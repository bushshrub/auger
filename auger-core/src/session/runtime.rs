use crate::SystemPrompt;
use crate::events::LoopMessage;
use crate::events::SessionCommand;
use crate::events::SessionEvent;
use crate::ids::SessionId;
use crate::session::history::AssistantTurnOutcome;
use crate::session::history::AuthorizationSource;
use crate::session::history::StopReason;
use crate::session::history::SessionRecord;
use crate::session::recorder::SessionRecorder;
use crate::session::states::HarnessState;
use crate::tools::auto_approval::AutoApprovalPolicies;
use crate::tools::tool_decisions::ToolAuthorization;
use crate::tools::tool_decisions::UserToolDecisions;
use crate::tools::tool_execution::ToolExecution;
use crate::tools::tool_execution::ToolExecutionCompleted;
use crate::tools::tool_registry::ToolRegistry;
use agent_tools::Tool;
use auger_driver::RestoredAgent;
use auger_driver::StreamResult;
use auger_driver::restore;
use chrono::DateTime;
use chrono::Utc;
use either::Either;
use getset::CopyGetters;
use mpsc::Receiver;
use provider::LlmModel;
use provider::ToolDefinition;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use tokio::runtime::Handle;
use tracing::info;
use tracing::warn;

#[derive(Clone, Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("session is closed")]
    SessionClosed,
}

/// A handle to a running auger session
#[derive(Clone, CopyGetters)]
pub struct SessionHandle {
    #[get_copy = "pub"]
    id: SessionId,
    loop_event_tx: Sender<LoopMessage>,

    // TODO: This REALLY does not belong here.
    // But SessionRecord is owned by Session...
    #[get_copy = "pub"]
    created_at: DateTime<Utc>,
}

impl SessionHandle {
    fn new(id: SessionId, command_tx: Sender<LoopMessage>, created_at: DateTime<Utc>) -> Self {
        Self {
            id,
            loop_event_tx: command_tx,
            created_at,
        }
    }

    // TODO: Deal with error types
    pub fn send_command(&self, cmd: SessionCommand) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopMessage::Cmd(cmd))
            .map_err(|_| ())
    }
}

pub struct Session {
    id: SessionId,
    /// Receiver to receive session commands and agent events from
    cmd_rx: Receiver<LoopMessage>,
    harness_internal_event_tx: Sender<LoopMessage>,
    /// Sender for the session to emit events through
    event_tx: Sender<SessionEvent>,
    tool_registry: Arc<ToolRegistry>,
    auto_approval_policies: Arc<AutoApprovalPolicies>,
    recorder: SessionRecorder,
}

impl Session {
    fn create_initial_agent(
        system_prompt: SystemPrompt,
        record: &SessionRecord,
        model: LlmModel,
        tools: Vec<ToolDefinition>,
    ) -> RestoredAgent {
        restore(model, system_prompt.into(), tools, record.restore_state())
    }

    pub(super) fn spawn(
        rt: Handle,
        system_prompt: SystemPrompt,
        record: SessionRecorder,
        model: LlmModel,
        tools: Vec<Box<dyn Tool>>,
        auto_approval_policies: AutoApprovalPolicies,
    ) -> (SessionHandle, Receiver<SessionEvent>) {
        let id = record.record().data().session_id();
        let creation_time = record.record().data().created_at();
        let mut tool_registry = ToolRegistry::new();
        for tool in tools {
            tool_registry.register(tool);
        }
        let tool_registry = Arc::new(tool_registry);
        let llm_tools = tool_registry.list_for_clanker();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let session = Self {
            id,
            cmd_rx,
            harness_internal_event_tx: cmd_tx.clone(),
            event_tx,
            tool_registry,
            auto_approval_policies: Arc::new(auto_approval_policies),
            recorder: record,
        };
        let handle = SessionHandle::new(session.id, cmd_tx.clone(), creation_time);

        let initial_agent =
            Self::create_initial_agent(system_prompt, &session.recorder.record(), model, llm_tools);

        std::thread::Builder::new()
            .name(format!("auger-session-{}", session.id))
            .spawn(move || session.run(rt, initial_agent))
            .expect("failed to spawn session thread");

        (handle, event_rx)
    }

    fn run(mut self, rt: Handle, init_agent: RestoredAgent) {
        info!(session_id = %self.id, "Session started");
        let mut curr_state = init_agent.into();
        if let HarnessState::HasToolCalls { _agent: agent } = curr_state {
            let tool_calls = agent.get_requested_tools();
            let undecided = tool_calls.iter().map(|call| call.id.clone()).collect();
            let _ = self
                .event_tx
                .send(SessionEvent::ToolConsentRequired { tool_calls });
            curr_state = HarnessState::NeedToolConsent {
                agent,
                user_tool_decisions: UserToolDecisions::new_undecided(undecided),
            };
        }
        'session_loop: while let Ok(msg) = self.cmd_rx.recv() {
            match msg {
                LoopMessage::Cmd(cmd) => match cmd {
                    SessionCommand::Stop { reply_tx } => {
                        let _ = reply_tx.send(());
                        break 'session_loop;
                    }
                    SessionCommand::Snapshot { reply_tx } => {
                        let _ = reply_tx.send(self.recorder.record().clone());
                    }
                    SessionCommand::SendMessage(prompt) => {
                        info!(session_id = %self.id, "Received user message {:?}", prompt);
                        let new_agent = match curr_state {
                            HarnessState::WaitingForUserMessage { agent } => {
                                agent.add_message(prompt.clone())
                            }
                            // Keep whatever was streamed and carry on. A failed
                            // stream usually has no partial, so this just adds
                            // the message to the turn that never got answered.
                            HarnessState::StreamingInterrupted { agent }
                            | HarnessState::StreamingFailed { agent } => {
                                agent.seal_and_continue(prompt.clone())
                            }
                            HarnessState::InterruptingStream {
                                pending_message: None,
                            } => {
                                curr_state = HarnessState::InterruptingStream {
                                    pending_message: Some(prompt),
                                };
                                continue 'session_loop;
                            }
                            other => {
                                warn!(session_id = %self.id, command = "send_message", "Ignoring command in invalid harness state");
                                curr_state = other;
                                continue 'session_loop;
                            }
                        };

                        let event_tx = self.event_tx.clone();
                        let inbox_tx = self.harness_internal_event_tx.clone();
                        let stream_fut = new_agent.create_stream(move |event| {
                            let _ = event_tx.send(SessionEvent::StreamEvent(event));
                        });
                        let cancel = stream_fut.interrupt_handle();
                        let sess_id = self.id;

                        self.recorder.record_prompt(prompt);
                        rt.spawn(async move {
                            info!(session_id=%sess_id, "Starting stream");
                            let res = stream_fut.await;
                            inbox_tx
                                .send(LoopMessage::StreamResult(res))
                                .expect("inbox_rx was dropped");
                        });
                        curr_state = HarnessState::Streaming { cancel };
                    }
                    SessionCommand::Interrupt => {
                        curr_state = match curr_state {
                            HarnessState::Streaming { cancel } => {
                                info!(session_id = %self.id, "Interrupting LLM stream");
                                cancel.cancel();
                                HarnessState::InterruptingStream {
                                    pending_message: None,
                                }
                            }
                            HarnessState::ToolCallsAreRunning { agent, cancel } => {
                                info!(session_id = %self.id, "Interrupting tool execution");
                                cancel.cancel();
                                HarnessState::InterruptingToolExecution { agent }
                            }
                            _ => {
                                warn!(session_id = %self.id, command = "interrupt", "Ignoring command in invalid harness state");
                                curr_state
                            }
                        }
                    }
                    SessionCommand::ToolDecision {
                        id,
                        approved,
                        message,
                    } => {
                        curr_state = match curr_state {
                            HarnessState::NeedToolConsent {
                                agent,
                                user_tool_decisions,
                            } => {
                                let valid_decision = user_tool_decisions.is_undecided(&id);
                                if valid_decision {
                                    let prev_turn_id = self
                                        .recorder
                                        .previous_turn_id()
                                        .expect("there to be a previous turn");
                                    self.recorder
                                        .record_tool_decision(
                                            prev_turn_id,
                                            id.clone().into(),
                                            approved,
                                            AuthorizationSource::User,
                                            message.clone(),
                                        )
                                        .expect("previous turn to be assistant");
                                }
                                match user_tool_decisions.record_decision(id, approved, message) {
                                    Either::Left(not_all_decided) => {
                                        HarnessState::NeedToolConsent {
                                            agent,
                                            user_tool_decisions: not_all_decided,
                                        }
                                    }
                                    Either::Right(all_decided) => {
                                        let batch = agent.get_batch();
                                        let execution = ToolExecution::new(
                                            batch.requested().cloned().collect(),
                                            ToolAuthorization::PerTool(all_decided),
                                            self.tool_registry.clone(),
                                            self.event_tx.clone(),
                                        )
                                        .run();
                                        let cancel = execution.interrupt_handle();
                                        let inbox_tx = self.harness_internal_event_tx.clone();
                                        rt.spawn(async move {
                                            let results = execution.await;
                                            let _ = inbox_tx.send(
                                                LoopMessage::ToolBatchExecutionResult {
                                                    batch,
                                                    results,
                                                },
                                            );
                                        });
                                        HarnessState::ToolCallsAreRunning { agent, cancel }
                                    }
                                }
                            }
                            _ => {
                                warn!(session_id = %self.id, command = "tool_decision", "Ignoring command in invalid harness state");
                                curr_state
                            }
                        }
                    }
                },
                LoopMessage::StreamResult(res) => {
                    curr_state = match curr_state {
                        HarnessState::Streaming { cancel } => {
                            drop(cancel);
                            match res {
                                StreamResult::Interrupted(_) => {
                                    // invalid state - unrecoverable.
                                    panic!(
                                        "stream returned interrupted while harness was still \
                                         streaming"
                                    )
                                }
                                StreamResult::Failed { agent, error } => {
                                    warn!(
                                        session_id = %self.id,
                                        error = %error,
                                        "LLM stream failed; waiting for a new user message"
                                    );
                                    self.recorder
                                        .record_assistant(AssistantTurnOutcome::Incomplete {
                                            partial_response: agent.state().partial().clone(),
                                            reason: StopReason::Failed(error.clone()),
                                        })
                                        .expect("previous turn was user");
                                    let _ = self.event_tx.send(SessionEvent::StreamError {
                                        error: error.to_string(),
                                    });
                                    HarnessState::StreamingFailed { agent }
                                }
                                StreamResult::WaitingForToolResponses { agent, end } => {
                                    info!(session_id = %self.id, "stream finished: agent has called tools");

                                    let turn_id = self
                                        .recorder
                                        .record_assistant(AssistantTurnOutcome::Completed {
                                            response: agent.previous_message().clone(),
                                        })
                                        .expect("last turn to be user");
                                    let _ = self.event_tx.send(SessionEvent::TurnComplete {
                                        usage: end.usage,
                                        stop_reason: end.stop_reason,
                                    });

                                    let call_requests = agent.get_requested_tools();
                                    if self.auto_approval_policies.will_approve_all(&call_requests)
                                    {
                                        for call in &call_requests {
                                            self.recorder
                                                .record_tool_decision(
                                                    turn_id,
                                                    call.id.clone().into(),
                                                    true,
                                                    AuthorizationSource::Policy,
                                                    None,
                                                )
                                                .expect("turn to be assistant");
                                        }
                                        let batch = agent.get_batch();
                                        info!(session_id=%self.id, "automatically running all {} tools", call_requests.len());
                                        let execution = ToolExecution::new(
                                            call_requests,
                                            ToolAuthorization::AllAutoApproved,
                                            self.tool_registry.clone(),
                                            self.event_tx.clone(),
                                        )
                                        .run();
                                        let cancel = execution.interrupt_handle();
                                        let inbox_tx = self.harness_internal_event_tx.clone();
                                        rt.spawn(async move {
                                            let results = execution.await;
                                            let _ = inbox_tx.send(
                                                LoopMessage::ToolBatchExecutionResult {
                                                    batch,
                                                    results,
                                                },
                                            );
                                        });
                                        HarnessState::ToolCallsAreRunning { agent, cancel }
                                    } else {
                                        info!(session_id=%self.id, "Some tools require consent");
                                        for call in &call_requests {
                                            if self.auto_approval_policies.is_approved(call) {
                                                self.recorder
                                                    .record_tool_decision(
                                                        turn_id,
                                                        call.id.clone().into(),
                                                        true,
                                                        AuthorizationSource::Policy,
                                                        None,
                                                    )
                                                    .expect("turn to be assistant");
                                            }
                                        }
                                        let unapproved = self
                                            .auto_approval_policies
                                            .ids_needing_consent(&call_requests);
                                        let tool_calls = agent
                                            .get_requested_tools()
                                            .into_iter()
                                            .filter(|call| unapproved.contains(&call.id))
                                            .collect();
                                        info!(session_id=%self.id, "User consent needed for {} tools", unapproved.len());
                                        let _ = self
                                            .event_tx
                                            .send(SessionEvent::ToolConsentRequired { tool_calls });
                                        HarnessState::NeedToolConsent {
                                            agent,
                                            user_tool_decisions: UserToolDecisions::new_undecided(
                                                unapproved,
                                            ),
                                        }
                                    }
                                }
                                StreamResult::WaitingForUserMessage { agent, end } => {
                                    info!(session_id=%self.id, "Stream has returned: No tools called");
                                    self.recorder
                                        .record_assistant(AssistantTurnOutcome::Completed {
                                            response: agent
                                                .previous_message()
                                                .expect("a previous message to exist")
                                                .clone(),
                                        })
                                        .expect("last turn to be user");
                                    let _ = self.event_tx.send(SessionEvent::TurnComplete {
                                        usage: end.usage,
                                        stop_reason: end.stop_reason,
                                    });
                                    HarnessState::WaitingForUserMessage { agent }
                                }
                            }
                        }
                        HarnessState::InterruptingStream { pending_message } => match res {
                            StreamResult::Interrupted(agent) => match pending_message {
                                Some(prompt) => {
                                    let event_tx = self.event_tx.clone();
                                    let new_agent = agent.seal_and_continue(prompt.clone());
                                    let inbox_tx = self.harness_internal_event_tx.clone();
                                    let stream_fut = new_agent.create_stream(move |event| {
                                        let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                    });
                                    let cancel = stream_fut.interrupt_handle();
                                    self.recorder.record_prompt(prompt);
                                    rt.spawn(async move {
                                        let res = stream_fut.await;
                                        inbox_tx
                                            .send(LoopMessage::StreamResult(res))
                                            .expect("inbox_rx was dropped");
                                    });
                                    HarnessState::Streaming { cancel }
                                }
                                None => {
                                    info!(session_id=%self.id, "Stream successfully interrupted (no user msg)");
                                    let _ = self.event_tx.send(SessionEvent::Interrupted);
                                    HarnessState::StreamingInterrupted { agent }
                                }
                            },
                            // TODO: we must handle these
                            StreamResult::Failed { .. } => {
                                panic!("stream failed while harness was interrupting the stream")
                            }
                            StreamResult::WaitingForToolResponses { .. } => {
                                panic!(
                                    "stream requested tools while harness was interrupting the \
                                     stream"
                                )
                            }
                            StreamResult::WaitingForUserMessage { .. } => {
                                panic!("stream completed while harness was interrupting the stream")
                            }
                        },
                        _ => curr_state,
                    };
                }
                LoopMessage::ToolBatchExecutionResult { mut batch, results } => {
                    info!(session_id=%self.id, "tools have finished executing");
                    let agent = match curr_state {
                        HarnessState::ToolCallsAreRunning { agent, cancel } => {
                            drop(cancel);
                            agent
                        }
                        HarnessState::InterruptingToolExecution { agent } => agent,
                        other => {
                            curr_state = other;
                            continue 'session_loop;
                        }
                    };

                    // TODO: That enum is useless if we are just going to mark everything as
                    // interrupted?
                    let tool_results = match results {
                        ToolExecutionCompleted::Completed(results) => results,
                        ToolExecutionCompleted::Interrupted(interrupted_results) => {
                            interrupted_results
                        }
                    };
                    for result in tool_results {
                        self.recorder.record_tool_result(result.clone());
                        batch
                            .add_result(result.tool_call_id(), result.into())
                            .expect("result to be for a requested call");
                    }
                    let resolved_batch = batch.into_resolved().expect_right("there is a bug");
                    // TODO: allow steering message to ride along
                    info!(session_id=%self.id, "Sending {} tool results back to the model", resolved_batch.results().len());
                    let new_agent = agent.add_all_tool_responses(resolved_batch);
                    let event_tx = self.event_tx.clone();
                    let stream_fut = new_agent.create_stream(move |event| {
                        let _ = event_tx.send(SessionEvent::StreamEvent(event));
                    });
                    let cancel = stream_fut.interrupt_handle();
                    let inbox_tx = self.harness_internal_event_tx.clone();

                    rt.spawn(async move {
                        let res = stream_fut.await;
                        inbox_tx
                            .send(LoopMessage::StreamResult(res))
                            .expect("inbox_rx was dropped");
                    });
                    curr_state = HarnessState::Streaming { cancel };
                }
            }
        }

        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }
}
