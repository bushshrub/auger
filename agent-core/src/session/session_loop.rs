use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use provider::{LlmProvider, LlmRequest, ToolCall};
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use uuid::Uuid;
use crate::session::events::{UserCmd, SessionEvent};
use crate::session::handle::SessionHandle;
use crate::session::{SessionId, SessionStatus};
use crate::system_prompt::SystemPrompt;
use futures::stream::StreamExt;
use tracing::{debug, error, info, trace};
use agent_tools::{Dummy, Tool};
use crate::session::tool_registry::ToolRegistry;

/// The main loop for a session.
/// Receives user commands, talks to the clanker, executes tools and then sends events back to
/// the user.
pub struct Session {
    id: SessionId,
    model: String,
    tools: ToolRegistry,
    status: SessionStatus,
    provider: Arc<dyn LlmProvider>,
    events: broadcast::Sender<SessionEvent>,
}

impl Session {
    // TODO: no need to share LlmProvider. session should take care of spawning it.
    pub fn spawn(prompt: SystemPrompt, provider: &Arc<impl LlmProvider + 'static>, model: String) -> SessionHandle {
        let (cmds_tx, cmds_rx) = mpsc::channel();
        let (events_tx, _) = broadcast::channel(32);

        let id = Uuid::new_v4();

        let session = Session {
            id,
            tools: ToolRegistry::new(),
            model: model.clone(),
            status: SessionStatus::Idle,
            provider: provider.clone(),
            events: events_tx.clone(),
        };

        // Session owns an OS thread, not a tokio task. The tokio runtime is still
        // reachable via the captured `Handle` to drive async provider IO (and fan
        // out IO work) via `block_on`, but the session's lifetime is the thread's.
        let handle = Handle::current();
        std::thread::Builder::new()
            .name(format!("session-{id}"))
            .spawn(move || session.run(cmds_rx, handle))
            .expect("failed to spawn session thread");
        SessionHandle::new(id, model, cmds_tx, events_tx)
    }

    fn stream_llm_turn(&mut self, request: LlmRequest, handle: Handle) -> HashMap<String, ToolCall> {
        let mut pending_tool_calls: HashMap<String, ToolCall> = HashMap::new();
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

            while let Some(event_result) = stream.next().await {
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
                        // TODO: handle tool call deltas
                    }
                    // clanker has finished generating tool call request
                    Ok(provider::StreamEvent::ToolCallComplete {id, name, arguments}) => {
                        debug!(tool_call_id = %id, tool = %name, "tool call complete: {}", arguments);
                        let tc = ToolCall { id: id.clone(), name: name.clone(), arguments: arguments.clone() };
                        pending_tool_calls.insert(id.clone(), tc);
                        let _ = events.send(SessionEvent::ToolCallRequest { id, name, arguments });

                    }
                    Ok(provider::StreamEvent::Done { usage, stop_reason }) => {
                        debug!("Response has finished generating. Usage: {:?}, stop reason: {:?}", usage, stop_reason);
                        self.status = SessionStatus::Idle;
                        let _ = events.send(SessionEvent::Done);
                    },
                    // TODO: Handle errors while streaming e.g. rate limit, connection drops.
                    Err(e) => {
                        error!("Error while streaming response from provider: {:?}", e);

                    },
                }
            }
        });
        pending_tool_calls
    }

    /// Runs the session. The user will send commands via `rx`.
    fn run(mut self, rx: mpsc::Receiver<UserCmd>, handle: Handle) {
        self.tools.register(Box::new(Dummy {}));

        // todo: refactor this garbage collection of state into a single struct
        let mut pending_tool_calls: HashMap<String, ToolCall> = HashMap::new();
        let mut tool_call_results: HashMap<String, String> = HashMap::new();

        info!(session_id = %self.id, "Starting session");
        while let Ok(cmd) = rx.recv() {
            match cmd {
                UserCmd::SendMessage(content) => {
                    self.status = SessionStatus::Running;
                    let _ = self.events.send(SessionEvent::UserMessage { content: content.clone() });

                    let user_text = content.msg;

                    let request = LlmRequest::new(
                        self.model.clone(),
                        vec![provider::Message::User(user_text)],
                        self.tools.list_for_clanker(),
                    );

                    let new_tool_calls = self.stream_llm_turn(request, handle.clone());
                    pending_tool_calls.extend(new_tool_calls);
                }
                UserCmd::ApproveToolCall { tool_call_id } => {
                    let tool_call = match pending_tool_calls.remove(&tool_call_id) {
                        Some(tc) => tc,
                        None => {
                            error!(tool_call_id = %tool_call_id, "No tool call found with this ID");
                            continue;
                        }
                    };
                    debug!(tool_call_id = %tool_call_id, "Tool call approved by user");
                    let tool_result = handle.block_on(async {
                        self.tools.call_tool(&tool_call.name, tool_call.arguments).await
                    });
                    match tool_result {
                        Ok(result) => {
                            debug!(tool_call_id = %tool_call_id, "Tool executed successfully: {}", &result);
                            tool_call_results.insert(tool_call_id.clone(), result.clone());
                            let _ = self.events.send(SessionEvent::ToolCallResult { id: tool_call_id, result });

                        }
                        Err(e) => {
                            error!(tool_call_id = %tool_call_id, "Error executing tool: {:?}", e);
                            tool_call_results.insert(tool_call_id.clone(), format!("Error executing tool: {:?}", e));
                            let _ = self.events.send(SessionEvent::ToolCallError { id: tool_call_id, error: format!("{:?}", e) });
                        }
                    }
                    if pending_tool_calls.is_empty() {
                        debug!("All pending tool calls have been processed. Resuming LLM turn.");
                        // send back to LLM
                        let tool_results: Vec<provider::ToolResult> = tool_call_results.iter().map(|(id, result)| {
                            provider::ToolResult {
                                tool_call_id: id.clone(),
                                content: result.clone(),
                            }
                        }).collect();
                        let request = LlmRequest::new_with_tool_results(
                            self.model.clone(),
                            Vec::new(), // TODO: this is where we will do steering
                            self.tools.list_for_clanker(),
                            tool_results,
                        );
                        let new_tool_calls = self.stream_llm_turn(request, handle.clone());
                        pending_tool_calls.extend(new_tool_calls);
                    }
                }
                UserCmd::DenyToolCall { tool_call_id } => {
                    let tool_call = match pending_tool_calls.remove(&tool_call_id) {
                        Some(tc) => tc,
                        None => {
                            error!(tool_call_id = %tool_call_id, "No tool call found with this ID");
                            continue;
                        }
                    };
                    debug!(tool_call_id = %tool_call_id, "Tool call denied by user");
                    // TODO: bad error
                    tool_call_results.insert(tool_call_id.clone(), format!("Tool call to {} denied by user", tool_call.name));
                    let _ = self.events.send(SessionEvent::ToolCallError { id: tool_call_id, error: "Tool call denied by user".into() });
                    if pending_tool_calls.is_empty() {
                        debug!("All pending tool calls have been processed. Resuming LLM turn.");
                        // send back to LLM
                        let tool_results: Vec<provider::ToolResult> = tool_call_results.iter().map(|(id, result)| {
                            provider::ToolResult {
                                tool_call_id: id.clone(),
                                content: result.clone(),
                            }
                        }).collect();
                        let request = LlmRequest::new_with_tool_results(
                            self.model.clone(),
                            Vec::new(), // TODO: this is where we will do steering
                            self.tools.list_for_clanker(),
                            tool_results,
                        );
                        let new_tool_calls = self.stream_llm_turn(request, handle.clone());
                        pending_tool_calls.extend(new_tool_calls);
                    }
                }
            }
        }
        info!(session_id = %self.id, "Session has been closed");
    }
}