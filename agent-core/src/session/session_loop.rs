use std::sync::Arc;
use std::sync::mpsc;
use provider::{LlmProvider, LlmRequest, LlmResponse, ToolCall};
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use uuid::Uuid;
use crate::session::events::{UserCmd, SessionEvent};
use crate::session::handle::SessionHandle;
use crate::session::SessionId;
use crate::system_prompt::SystemPrompt;
use futures::stream::StreamExt;
use tracing::{debug, error, info, trace, warn};
use agent_tools::Dummy;
use crate::session::session_history::SessionHistory;
use crate::session::tool_call_batch::{Resolving, ToolCallBatch};
use crate::session::tool_registry::ToolRegistry;

enum RunState {
    Idle,
    AwaitingApproval(ToolCallBatch<Resolving>),
}

pub struct Session {
    id: SessionId,
    model: String,
    tools: ToolRegistry,
    provider: Arc<dyn LlmProvider>,
    events: broadcast::Sender<SessionEvent>,
    history: SessionHistory,
}

impl Session {
    pub fn spawn(prompt: SystemPrompt, provider: &Arc<impl LlmProvider + 'static>, model: String) -> SessionHandle {
        let (cmds_tx, cmds_rx) = mpsc::channel();
        let (events_tx, _) = broadcast::channel(32);

        let id = Uuid::new_v4();

        let session = Session {
            id,
            tools: ToolRegistry::new(),
            model: model.clone(),
            provider: provider.clone(),
            events: events_tx.clone(),
            history: SessionHistory::new(id, prompt),
        };

        let handle = Handle::current();
        std::thread::Builder::new()
            .name(format!("session-{id}"))
            .spawn(move || session.run(cmds_rx, handle))
            .expect("failed to spawn session thread");
        SessionHandle::new(id, model, cmds_tx, events_tx)
    }

    fn run(mut self, rx: mpsc::Receiver<UserCmd>, handle: Handle) {
        self.tools.register(Box::new(Dummy {}));
        self.tools.register(Box::new(agent_tools::ReadFile {}));
        self.tools.register(Box::new(agent_tools::ListFiles {}));

        let mut run_state = RunState::Idle;

        info!(session_id = %self.id, "Starting session");
        while let Ok(cmd) = rx.recv() {
            match cmd {
                UserCmd::SendMessage(content) => {
                    if matches!(run_state, RunState::AwaitingApproval(_)) {
                        warn!(session_id = %self.id, "Ignoring SendMessage while awaiting tool call approval");
                        continue;
                    }
                    let _ = self.events.send(SessionEvent::UserMessage { content: content.clone() });
                    let request = self.history
                        .begin_user_turn(self.model.clone(), self.tools.list_for_clanker())
                        .with_user_message(content.msg)
                        .build();
                    let tool_calls = self.stream_llm_turn(request, handle.clone());
                    run_state = if tool_calls.is_empty() {
                        RunState::Idle
                    } else {
                        RunState::AwaitingApproval(ToolCallBatch::new_batch(tool_calls))
                    };
                }
                UserCmd::ApproveToolCall { tool_call_id } => {
                    let RunState::AwaitingApproval(mut batch) = run_state else {
                        warn!(session_id = %self.id, "Received ApproveToolCall while not awaiting approval");
                        run_state = RunState::Idle;
                        continue;
                    };
                    let tool_result = handle.block_on(async {
                        batch.approve_and_run(&tool_call_id, &self.tools).await
                    });
                    match tool_result {
                        Ok(result) => {
                            debug!(tool_call_id = %tool_call_id, "Tool executed successfully: {}", &result);
                            let _ = self.events.send(SessionEvent::ToolCallResult { id: tool_call_id, result: result.to_string() });
                        }
                        Err(e) => {
                            error!(tool_call_id = %tool_call_id, "Error executing tool: {:?}", e);
                            let _ = self.events.send(SessionEvent::ToolCallError { id: tool_call_id, error: format!("{:?}", e) });
                        }
                    }
                    match batch.try_complete() {
                        Ok(complete) => {
                            debug!("All pending tool calls processed. Resuming LLM turn.");
                            let tool_results = complete.drain();
                            let request = self.history
                                .begin_tool_turn(self.model.clone(), self.tools.list_for_clanker())
                                .with_tool_results(None, tool_results)
                                .build();
                            let tool_calls = self.stream_llm_turn(request, handle.clone());
                            run_state = if tool_calls.is_empty() {
                                RunState::Idle
                            } else {
                                RunState::AwaitingApproval(ToolCallBatch::new_batch(tool_calls))
                            };
                        }
                        Err(batch) => {
                            run_state = RunState::AwaitingApproval(batch);
                        }
                    }
                }
                UserCmd::DenyToolCall { tool_call_id } => {
                    let RunState::AwaitingApproval(mut batch) = run_state else {
                        warn!(session_id = %self.id, "Received DenyToolCall while not awaiting approval");
                        run_state = RunState::Idle;
                        continue;
                    };
                    match batch.deny(&tool_call_id) {
                        Ok(_) => {
                            debug!(tool_call_id = %tool_call_id, "Tool call denied by user.");
                            let _ = self.events.send(SessionEvent::ToolCallDenied { id: tool_call_id, reason: "User denied".to_string() });
                        }
                        Err(e) => {
                            error!(tool_call_id = %tool_call_id, "Error denying tool call: {:?}", e);
                            let _ = self.events.send(SessionEvent::ToolCallError { id: tool_call_id, error: format!("{:?}", e) });
                        }
                    }
                    match batch.try_complete() {
                        Ok(complete) => {
                            debug!("All pending tool calls processed. Resuming LLM turn.");
                            let tool_results = complete.drain();
                            let request = self.history
                                .begin_tool_turn(self.model.clone(), self.tools.list_for_clanker())
                                .with_tool_results(None, tool_results)
                                .build();
                            let tool_calls = self.stream_llm_turn(request, handle.clone());
                            run_state = if tool_calls.is_empty() {
                                RunState::Idle
                            } else {
                                RunState::AwaitingApproval(ToolCallBatch::new_batch(tool_calls))
                            };
                        }
                        Err(batch) => {
                            run_state = RunState::AwaitingApproval(batch);
                        }
                    }
                }
                UserCmd::Snapshot { reply } => {
                    let _ = reply.send(self.history.messages().to_vec());
                }
            }
        }
        info!(session_id = %self.id, "Session has been closed");
    }

    fn stream_llm_turn(&mut self, request: LlmRequest, handle: Handle) -> Vec<ToolCall> {
        let mut pending_tool_calls: Vec<ToolCall> = Vec::new();
        let events = &self.events;
        let provider = &self.provider;
        handle.block_on(async {
            let mut stream = match provider.stream(request).await {
                Ok(stream) => stream,
                Err(e) => {
                    error!("Error opening provider stream: {:?}", e);
                    return;
                }
            };

            let mut full_response = Vec::new();
            while let Some(event_result) = stream.next().await {
                let evt_clone = event_result.clone();
                match evt_clone {
                    Ok(evt) => full_response.push(evt),
                    Err(_) => {}
                }
                match event_result {
                    Ok(provider::StreamEvent::TextDelta(text)) => {
                        trace!("text delta: {}", text);
                        let _ = events.send(SessionEvent::Content { delta: text });
                    }
                    Ok(provider::StreamEvent::ReasoningDelta(text)) => {
                        trace!("reasoning delta: {}", text);
                        let _ = events.send(SessionEvent::Reasoning { delta: text });
                    }
                    Ok(provider::StreamEvent::ToolCall { id, name, arguments }) => {
                        trace!(tool_call_id = %id, tool = %name, "tool call delta: {}", arguments)
                    }
                    Ok(provider::StreamEvent::ToolCallComplete { id, name, arguments }) => {
                        debug!(tool_call_id = %id, tool = %name, "tool call complete: {}", arguments);
                        pending_tool_calls.push(ToolCall { id: id.clone(), name: name.clone(), arguments: arguments.clone() });
                        let _ = events.send(SessionEvent::ToolCallRequest { id, name, arguments });
                    }
                    Ok(provider::StreamEvent::Done { usage, stop_reason }) => {
                        debug!("Response finished. Usage: {:?}, stop reason: {:?}", usage, stop_reason);
                        let _ = events.send(SessionEvent::Done { usage, stop_reason });
                    }
                    Err(e) => {
                        error!("Error while streaming response from provider: {:?}", e);
                    }
                }
            }
            let complete_response = LlmResponse::from(full_response);
            self.history.push_llm_response(complete_response);
        });
        pending_tool_calls
    }
}
