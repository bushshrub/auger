use std::sync::Arc;
use std::sync::mpsc;
use either::Either;
use provider::{AnyThread, ClankerMessage, LlmProvider, LlmThread, ToolCallRequest, ToolResult, UserPrompt};
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use uuid::Uuid;
use crate::session::events::{UserAction, ToolCallResponse, SessionEvent, UserCommand, ToolCallEvent, ClankerEvent};
use crate::session::handle::SessionHandle;
use crate::session::SessionId;
use crate::system_prompt::SystemPrompt;
use futures::stream::StreamExt;
use tracing::{debug, error, info, trace, warn};
use agent_tools::Dummy;
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::tool_call_batch::{Resolving, ToolCallBatch};
use crate::tools::tool_registry::ToolRegistry;
use provider::thread::{UserTurn, ToolResultsPending, ClankerTurn};

enum RunState {
    Idle,
    AwaitingApproval {
        batch: ToolCallBatch<Resolving>,
        completed_results: Vec<ToolResult>,
    },
}

pub struct Session {
    id: SessionId,
    model: String,
    tools: ToolRegistry,
    provider: Arc<dyn LlmProvider>,
    events: broadcast::Sender<SessionEvent>,
    thread: AnyThread,
    auto_approve: AutoApprovalPolicy,
}

impl Session {
    pub fn spawn(prompt: SystemPrompt, provider: &Arc<impl LlmProvider + 'static>, model: String) -> SessionHandle {
        let (cmds_tx, cmds_rx) = mpsc::channel();
        let (events_tx, _) = broadcast::channel(32);

        let id = Uuid::new_v4();

        let auto_approved_defaults = vec!["read_file", "list_files", "grep", "glob", "todo_list", "web_search"];

        let session = Session {
            id,
            tools: ToolRegistry::new(),
            model: model.clone(),
            provider: provider.clone(),
            events: events_tx.clone(),
            thread: AnyThread::User(LlmThread::new(prompt.into())),
            auto_approve: AutoApprovalPolicy::new(auto_approved_defaults.iter().map(|s| s.to_string())),
        };

        let handle = Handle::current();
        std::thread::Builder::new()
            .name(format!("session-{id}"))
            .spawn(move || session.run(cmds_rx, handle))
            .expect("failed to spawn session thread");
        SessionHandle::new(id, model, cmds_tx, events_tx)
    }

    fn run(mut self, rx: mpsc::Receiver<UserCommand>, handle: Handle) {
        self.tools.register(Box::new(Dummy {}));
        self.tools.register(Box::new(agent_tools::ReadFile {}));
        self.tools.register(Box::new(agent_tools::ListFiles {}));
        self.tools.register(Box::new(agent_tools::Grep {}));
        self.tools.register(Box::new(agent_tools::Glob {}));
        self.tools.register(Box::new(agent_tools::WriteFile {}));
        self.tools.register(Box::new(agent_tools::EditFile {}));
        self.tools.register(Box::new(agent_tools::Shell {}));
        self.tools.register(Box::new(agent_tools::WebSearch::new()));
        self.tools.register(Box::new(agent_tools::FetchContent::new()));
        self.tools.register(Box::new(agent_tools::WebFetch::new()));
        self.tools.register(Box::new(agent_tools::WebFetchText::new()));
        self.tools.register(Box::new(agent_tools::TodoList::new()));

        let mut run_state = RunState::Idle;

        info!(session_id = %self.id, "Starting session");
        while let Ok(cmd) = rx.recv() {
            match cmd {
                UserCommand::Action(action) => match action {
                    UserAction::SendMessage(content) => {
                        if matches!(run_state, RunState::AwaitingApproval { .. }) {
                            warn!(session_id = %self.id, "Ignoring SendMessage while awaiting tool call approval");
                            continue;
                        }
                        let AnyThread::User(user_thread) = std::mem::replace(&mut self.thread, AnyThread::User(LlmThread::new(String::new()))) else {
                            warn!(session_id = %self.id, "SendMessage received but thread is not in UserTurn state");
                            continue;
                        };
                        let clanker_thread = user_thread.add_user_message(UserPrompt::new(content.msg));
                        match self.stream_llm_turn(clanker_thread, handle.clone()) {
                            Either::Left(user_thread) => {
                                self.thread = AnyThread::User(user_thread);
                                run_state = RunState::Idle;
                            }
                            Either::Right(tools_thread) => {
                                run_state = self.resolve_tool_calls(tools_thread, handle.clone());
                            }
                        }
                    }
                    UserAction::RespondToToolCall { response, tool_call_id, .. } => {
                        let RunState::AwaitingApproval { batch, completed_results } = run_state else {
                            warn!(session_id = %self.id, "Received RespondToToolCall while not awaiting approval");
                            run_state = RunState::Idle;
                            continue;
                        };
                        let result = match response {
                            ToolCallResponse::Approve => {
                                handle.block_on(async {
                                    batch.approve_and_run(&tool_call_id, &self.tools).await
                                })
                            }
                            ToolCallResponse::Deny => batch.deny(&tool_call_id),
                        };
                        match result {
                            Ok(Either::Left(batch)) => {
                                run_state = RunState::AwaitingApproval { batch, completed_results };
                            }
                            Ok(Either::Right(complete)) => {
                                let mut all_results = complete.drain();
                                all_results.extend(completed_results);
                                run_state = self.submit_tool_results(all_results, handle.clone());
                            }
                            Err(e) => {
                                error!(tool_call_id = %tool_call_id, "Error processing tool call: {:?}", e);
                                let _ = self.events.send(ToolCallEvent::Error { id: tool_call_id, error: format!("{:?}", e) }.into());
                                run_state = RunState::Idle;
                            }
                        }
                    }
                }

                UserCommand::Snapshot { reply } => {
                    let messages = match &self.thread {
                        AnyThread::User(t) => t.messages().to_vec(),
                        AnyThread::ToolsPending(t) => t.messages().to_vec(),
                        AnyThread::Clanker(t) => t.messages().to_vec(),
                    };
                    let _ = reply.send(messages);
                }
            }
        }
        info!(session_id = %self.id, "Session has been closed");
    }

    fn submit_tool_results(&mut self, results: Vec<ToolResult>, handle: Handle) -> RunState {
        let AnyThread::ToolsPending(tools_thread) = std::mem::replace(&mut self.thread, AnyThread::User(LlmThread::new(String::new()))) else {
            error!(session_id = %self.id, "Expected ToolsPending state when submitting tool results");
            return RunState::Idle;
        };
        let clanker_thread = match tools_thread.add_tool_results(results) {
            Ok(t) => t,
            Err(e) => {
                error!(session_id = %self.id, "Failed to add tool results: {:?}", e);
                return RunState::Idle;
            }
        };
        match self.stream_llm_turn(clanker_thread, handle.clone()) {
            Either::Left(user_thread) => {
                self.thread = AnyThread::User(user_thread);
                RunState::Idle
            }
            Either::Right(tools_thread) => {
                self.resolve_tool_calls(tools_thread, handle)
            }
        }
    }

    fn run_auto_approved(&self, tool_calls: Vec<ToolCallRequest>, handle: Handle) -> (Vec<ToolCallRequest>, Vec<ToolResult>) {
        let (approved, deferred): (Vec<_>, Vec<_>) = tool_calls
            .into_iter()
            .partition(|tc| self.auto_approve.is_approved(&tc.name));

        let results = handle.block_on(async {
            futures::future::join_all(approved.iter().map(|tc| self.tools.invoke(tc.clone()))).await
        });

        let auto_results = approved.into_iter().zip(results).map(|(tc, result)| {
            let _ = self.events.send(ToolCallEvent::AutoApproved {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            }.into());
            let content = match result {
                Ok(r) => {
                    let _ = self.events.send(ToolCallEvent::Result { id: tc.id.clone(), result: r.to_string() }.into());
                    r.to_string()
                }
                Err(e) => {
                    let _ = self.events.send(ToolCallEvent::Error { id: tc.id.clone(), error: e.to_string() }.into());
                    format!("error: {e}")
                }
            };
            ToolResult::new(tc.id, content)
        }).collect();

        (deferred, auto_results)
    }

    fn resolve_tool_calls(&mut self, tools_thread: LlmThread<ToolResultsPending>, handle: Handle) -> RunState {
        let tool_calls = tools_thread.get_pending_tool_calls();
        let (deferred, auto_results) = self.run_auto_approved(tool_calls, handle.clone());

        if deferred.is_empty() {
            if auto_results.is_empty() {
                self.thread = AnyThread::ToolsPending(tools_thread);
                return RunState::Idle;
            }
            self.thread = AnyThread::ToolsPending(tools_thread);
            self.submit_tool_results(auto_results, handle)
        } else {
            self.thread = AnyThread::ToolsPending(tools_thread);
            RunState::AwaitingApproval {
                batch: ToolCallBatch::new_batch(deferred),
                completed_results: auto_results,
            }
        }
    }

    fn stream_llm_turn(&self, thread: LlmThread<ClankerTurn>, handle: Handle) -> Either<LlmThread<UserTurn>, LlmThread<ToolResultsPending>> {
        let request = thread.create_request(self.model.clone(), self.tools.list_for_clanker());
        let events = &self.events;
        let provider = &self.provider;

        let response = handle.block_on(async {
            trace!("Making request to provider: {:?}", request);
            let mut stream = match provider.stream(request).await {
                Ok(stream) => stream,
                Err(e) => {
                    error!("Error opening provider stream: {:?}", e);
                    return None;
                }
            };

            let mut full_response = Vec::new();
            while let Some(event_result) = stream.next().await {
                match &event_result {
                    Ok(evt) => full_response.push(evt.clone()),
                    Err(_) => {}
                }
                match event_result {
                    Ok(provider::StreamEvent::TextDelta(text)) => {
                        trace!("text delta: {}", text);
                        let _ = events.send(ClankerEvent::ContentDelta { delta: text }.into());
                    }
                    Ok(provider::StreamEvent::ReasoningDelta(text)) => {
                        trace!("reasoning delta: {}", text);
                        let _ = events.send(ClankerEvent::ReasoningDelta { delta: text }.into());
                    }
                    Ok(provider::StreamEvent::ToolCall { id, name, arguments }) => {
                        trace!(tool_call_id = %id, tool = %name, "tool call delta: {}", arguments);
                    }
                    Ok(provider::StreamEvent::ToolCallComplete { id, name, arguments }) => {
                        debug!(tool_call_id = %id, tool = %name, "tool call complete: {}", arguments);
                        let _ = events.send(ClankerEvent::ToolCallRequest { id, name, arguments }.into());
                    }
                    Ok(provider::StreamEvent::Done { usage, stop_reason }) => {
                        debug!("Response finished. Usage: {:?}, stop reason: {:?}", usage, stop_reason);
                        let _ = events.send(ClankerEvent::Done { usage, stop_reason }.into());
                    }
                    Err(e) => {
                        error!("Error while streaming response from provider: {:?}", e);
                    }
                }
            }
            Some(provider::LlmResponse::from(full_response))
        });

        let clanker_msg = match response {
            Some(r) => ClankerMessage::from(r),
            None => ClankerMessage::from(provider::LlmResponse {
                content: String::new(),
                reasoning: None,
                tool_calls: None,
                usage: None,
                stop_reason: Some("error".to_string()),
            }),
        };

        thread.add_clanker_reply(clanker_msg)
    }
}
