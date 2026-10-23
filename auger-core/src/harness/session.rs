use crate::SystemPrompt;
use crate::harness::events::LoopMessage;
use crate::harness::events::SessionCommand;
use crate::harness::events::SessionEvent;
use crate::harness::states::HarnessState;
use crate::harness::tool_engine::Phase;
use crate::harness::tool_engine::Step;
use crate::harness::tool_engine::ToolMessageSender;
use crate::harness::tool_engine::ToolPhaseEngine;
use crate::harness::tool_engine::check::ToolSetup;
use crate::harness::tool_engine::registry::ToolRegistry;
use crate::record::recorder::SessionRecorder;
use crate::record::session::SessionId;
use crate::record::session::SessionRecord;
use crate::record::turn::AssistantTurnOutcome;
use crate::record::turn::StopReason;
use crate::policy::PolicyNode;
use crate::tool::Tool;
use crate::tool::context::WorkDir;
use auger_driver::ReadyToStream;
use auger_driver::RestoredAgent;
use auger_driver::StreamResult;
use auger_driver::TypedAgent;
use auger_driver::WaitingForToolResponses;
use auger_driver::restore;
use chrono::DateTime;
use chrono::Utc;
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
    tools: ToolSetup,
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
        policy: Arc<dyn PolicyNode>,
    ) -> (SessionHandle, Receiver<SessionEvent>) {
        let id = record.record().data().session_id();
        let creation_time = record.record().data().created_at();
        let mut tool_registry = ToolRegistry::new();
        for tool in tools {
            tool_registry.register(tool);
        }
        let tool_registry = Arc::new(tool_registry);
        let llm_tools = tool_registry.list_for_clanker();
        let workspace = record.record().data().cwd().clone();
        let tools = ToolSetup {
            registry: tool_registry,
            policy,
            workdir: WorkDir::new(workspace.clone()),
            workspace,
        };
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let session = Self {
            id,
            cmd_rx,
            harness_internal_event_tx: cmd_tx.clone(),
            event_tx,
            tools,
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

    /// Start streaming from `agent`, forwarding stream events to subscribers
    /// and the eventual result back into the loop's own inbox.
    fn start_stream(&self, agent: TypedAgent<ReadyToStream>, rt: &Handle) -> HarnessState {
        let event_tx = self.event_tx.clone();
        let inbox_tx = self.harness_internal_event_tx.clone();
        let stream_fut = agent.create_stream(move |event| {
            let _ = event_tx.send(SessionEvent::StreamEvent(event));
        });
        let cancel = stream_fut.interrupt_handle();
        let sess_id = self.id;
        rt.spawn(async move {
            info!(session_id = %sess_id, "Starting stream");
            let res = stream_fut.await;
            inbox_tx
                .send(LoopMessage::StreamResult(res))
                .expect("inbox_rx was dropped");
        });
        HarnessState::Streaming { cancel }
    }

    /// Run the tool calls of `agent`. Streams again at once if there is
    /// nothing to wait for.
    fn start_tool_phase(
        &mut self,
        agent: TypedAgent<WaitingForToolResponses>,
        rt: &Handle,
    ) -> HarnessState {
        let inbox_tx = self.harness_internal_event_tx.clone();
        let send: ToolMessageSender = Arc::new(move |msg| {
            let _ = inbox_tx.send(LoopMessage::Tool(msg));
        });
        let started = ToolPhaseEngine::start(agent.get_batch(), self.tools.clone(), rt, send);
        for decision in started.decisions {
            let _ = self.event_tx.send(SessionEvent::ToolPolicyDecision {
                tool_call_id: decision.tool_call_id,
                verdict: decision.verdict,
            });
        }
        for result in started.finished {
            self.recorder.record_tool_result(result.clone());
            let _ = self.event_tx.send(SessionEvent::ToolCallResult(result));
        }
        self.continue_tool_phase(agent, started.phase, rt)
    }

    fn continue_tool_phase(
        &self,
        agent: TypedAgent<WaitingForToolResponses>,
        phase: Phase,
        rt: &Handle,
    ) -> HarnessState {
        match phase {
            Phase::Pending {
                engine,
                needs_consent,
            } => {
                if !needs_consent.is_empty() {
                    let _ = self.event_tx.send(SessionEvent::ToolConsentRequired {
                        tool_calls: needs_consent,
                    });
                }
                HarnessState::ToolPhase { agent, engine }
            }
            Phase::Done(batch) => {
                info!(session_id = %self.id, "Sending tool results back to the model");
                self.start_stream(agent.add_all_tool_responses(batch), rt)
            }
            Phase::Withheld(batch) => {
                info!(session_id = %self.id, "Tool results withheld until the next user message");
                let _ = self.event_tx.send(SessionEvent::ToolResultsWithheld);
                HarnessState::ToolResultsWithheld { agent, batch }
            }
        }
    }

    /// Apply a step of the tool phase: report the finished calls, then move on.
    fn apply_tool_step(
        &mut self,
        agent: TypedAgent<WaitingForToolResponses>,
        step: Step,
        rt: &Handle,
    ) -> HarnessState {
        for result in step.finished {
            self.recorder.record_tool_result(result.clone());
            let _ = self.event_tx.send(SessionEvent::ToolCallResult(result));
        }
        self.continue_tool_phase(agent, step.phase, rt)
    }

    fn run(mut self, rt: Handle, init_agent: RestoredAgent) {
        info!(session_id = %self.id, "Session started");
        let mut curr_state = match init_agent {
            RestoredAgent::WaitingForUserMessage(agent) => {
                HarnessState::WaitingForUserMessage { agent }
            }
            RestoredAgent::WaitingForToolResponses(agent) => self.start_tool_phase(agent, &rt),
            RestoredAgent::Interrupted(agent) => HarnessState::StreamingInterrupted { agent },
        };
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
                            HarnessState::ToolResultsWithheld { agent, batch } => {
                                agent.inject(prompt.clone()).add_all_tool_responses(batch)
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

                        self.recorder.record_prompt(prompt);
                        curr_state = self.start_stream(new_agent, &rt);
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
                            HarnessState::ToolPhase { agent, engine } => {
                                let step = engine.decide(&id, approved, message);
                                self.apply_tool_step(agent, step, &rt)
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

                                    self.recorder
                                        .record_assistant(AssistantTurnOutcome::Completed {
                                            response: agent.previous_message().clone(),
                                        })
                                        .expect("last turn to be user");
                                    let _ = self.event_tx.send(SessionEvent::TurnComplete {
                                        usage: end.usage,
                                        stop_reason: end.stop_reason,
                                    });

                                    self.start_tool_phase(agent, &rt)
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
                                    let new_agent = agent.seal_and_continue(prompt.clone());
                                    self.recorder.record_prompt(prompt);
                                    self.start_stream(new_agent, &rt)
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
                LoopMessage::Tool(msg) => {
                    curr_state = match curr_state {
                        HarnessState::ToolPhase { agent, engine } => {
                            let step = engine.receive(msg);
                            self.apply_tool_step(agent, step, &rt)
                        }
                        // The phase is over: a late result, drop it.
                        other => other,
                    };
                }
            }
        }

        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }
}
