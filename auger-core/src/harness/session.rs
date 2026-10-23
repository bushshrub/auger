use crate::harness::builder::Origin;
use crate::harness::events::LoopMessage;
use crate::harness::events::SessionCommand;
use crate::harness::events::SessionEvent;
use crate::harness::states::HarnessState;
use crate::harness::tools_missing::missing_tools;
use crate::record::RecordBody;
use crate::record::RecordSink;
use crate::record::RestoreError;
use crate::record::SessionHeader;
use crate::record::SessionId;
use crate::record::StreamRecord;
use crate::record::fold::TRACE_VERSION;
use crate::record::writer::RecordWriter;
use auger_driver::ReadyToStream;
use auger_driver::RestoreState;
use auger_driver::RestoredAgent;
use auger_driver::StreamResult;
use auger_driver::TypedAgent;
use auger_driver::WaitingForToolResponses;
use auger_driver::restore;
use auger_toolengine::Phase;
use auger_toolengine::PolicyNode;
use auger_toolengine::Tool;
use auger_toolengine::ToolFact;
use auger_toolengine::ToolMessageSender;
use auger_toolengine::ToolPhaseEngine;
use auger_toolengine::ToolSetup;
use chrono::DateTime;
use chrono::Utc;
use getset::CopyGetters;
use mpsc::Receiver;
use provider::LlmModel;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use tokio::runtime::Handle;
use tracing::info;
use tracing::warn;

/// A handle to a running auger session
#[derive(Clone, CopyGetters)]
pub struct SessionHandle {
    #[get_copy = "pub"]
    id: SessionId,
    loop_event_tx: Sender<LoopMessage>,

    // TODO: This REALLY does not belong here.
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
    writer: RecordWriter,
}

impl Session {
    pub(super) fn spawn(
        rt: Handle,
        origin: Origin,
        sink: Box<dyn RecordSink>,
        model: LlmModel,
        tools: Vec<Box<dyn Tool>>,
        policy: Arc<dyn PolicyNode>,
    ) -> Result<(SessionHandle, Receiver<SessionEvent>), RestoreError> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let offered: Vec<String> = tools.iter().map(|tool| tool.details().name).collect();
        let missing = match &origin {
            Origin::Restored(restored) => missing_tools(&restored.turns, &offered),
            Origin::New { .. } => Vec::new(),
        };
        let (data, writer, system_prompt, state, cwd, batch_facts) = match origin {
            Origin::New {
                data,
                system_prompt,
            } => {
                let writer = RecordWriter::new(sink, 0, event_tx.clone(), &[]);
                writer.note(RecordBody::Session(SessionHeader {
                    version: TRACE_VERSION,
                    data: data.clone(),
                    system_prompt: system_prompt.clone(),
                }));
                let cwd = data.cwd().clone();
                (data, writer, system_prompt, RestoreState::new(Vec::new(), None), cwd, Vec::new())
            }
            Origin::Restored(restored) => {
                let writer =
                    RecordWriter::new(sink, restored.next_seq, event_tx.clone(), &restored.turns);
                (
                    restored.header.data,
                    writer,
                    restored.header.system_prompt,
                    RestoreState::new(restored.turns, restored.partial),
                    restored.cwd,
                    restored.batch_facts,
                )
            }
        };
        let tools = ToolSetup::new(tools, policy, data.cwd().clone(), cwd, writer.journal());
        let initial_agent = restore(model, system_prompt, tools.definitions(), state)?;

        if !missing.is_empty() {
            let _ = event_tx.send(SessionEvent::ToolsMissing { names: missing });
        }

        let session = Self {
            id: data.session_id(),
            cmd_rx,
            harness_internal_event_tx: cmd_tx.clone(),
            event_tx,
            tools,
            writer,
        };
        let handle = SessionHandle::new(session.id, cmd_tx, data.created_at());

        std::thread::Builder::new()
            .name(format!("auger-session-{}", session.id))
            .spawn(move || session.run(rt, initial_agent, batch_facts))
            .expect("failed to spawn session thread");

        Ok((handle, event_rx))
    }

    /// Start streaming from `agent`, forwarding stream events to subscribers
    /// and the eventual result back into the loop's own inbox.
    fn start_stream(&self, agent: TypedAgent<ReadyToStream>, rt: &Handle) -> HarnessState {
        self.writer.sync_turns(agent.turns());
        self.writer.note(RecordBody::Stream(StreamRecord::Started));
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

    /// Run the tool calls of `agent`, resuming from `facts` journaled for
    /// this batch before a restart. Streams again at once if there is nothing
    /// to wait for.
    fn start_tool_phase(
        &mut self,
        agent: TypedAgent<WaitingForToolResponses>,
        facts: &[ToolFact],
        rt: &Handle,
    ) -> HarnessState {
        let inbox_tx = self.harness_internal_event_tx.clone();
        let send: ToolMessageSender = Arc::new(move |msg| {
            let _ = inbox_tx.send(LoopMessage::Tool(msg));
        });
        let phase = ToolPhaseEngine::resume(agent.get_batch(), self.tools.clone(), rt, send, facts);
        self.continue_tool_phase(agent, phase, rt)
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

    /// Record a stream that completed: its response is in `turns`.
    fn record_completed(&self, turns: &[auger_driver::Turn], end: provider::StreamEnd) {
        self.writer.sync_turns(turns);
        self.writer.note(RecordBody::Stream(StreamRecord::Completed {
            usage: end.usage,
            stop_reason: end.stop_reason,
        }));
    }

    fn run(mut self, rt: Handle, init_agent: RestoredAgent, batch_facts: Vec<ToolFact>) {
        info!(session_id = %self.id, "Session started");
        let mut curr_state = match init_agent {
            RestoredAgent::WaitingForUserMessage(agent) => {
                HarnessState::WaitingForUserMessage { agent }
            }
            RestoredAgent::WaitingForToolResponses(agent) => {
                self.start_tool_phase(agent, &batch_facts, &rt)
            }
            RestoredAgent::Interrupted(agent) => HarnessState::StreamingInterrupted { agent },
        };
        'session_loop: while let Ok(msg) = self.cmd_rx.recv() {
            match msg {
                LoopMessage::Cmd(cmd) => match cmd {
                    SessionCommand::Stop { reply_tx } => {
                        let _ = reply_tx.send(());
                        break 'session_loop;
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
                                let phase = engine.decide(&id, approved, message);
                                self.continue_tool_phase(agent, phase, &rt)
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
                                    self.writer.sync_turns(agent.turns());
                                    self.writer.note(RecordBody::Stream(StreamRecord::Failed {
                                        partial: agent.state().partial().clone(),
                                        error,
                                    }));
                                    HarnessState::StreamingFailed { agent }
                                }
                                StreamResult::WaitingForToolResponses { agent, end } => {
                                    info!(session_id = %self.id, "stream finished: agent has called tools");

                                    self.record_completed(agent.turns(), end);

                                    self.start_tool_phase(agent, &[], &rt)
                                }
                                StreamResult::WaitingForUserMessage { agent, end } => {
                                    info!(session_id=%self.id, "Stream has returned: No tools called");
                                    self.record_completed(agent.turns(), end);
                                    HarnessState::WaitingForUserMessage { agent }
                                }
                            }
                        }
                        HarnessState::InterruptingStream { pending_message } => match res {
                            StreamResult::Interrupted(agent) => {
                                self.writer.note(RecordBody::Stream(StreamRecord::Interrupted {
                                    partial: agent.state().partial().clone(),
                                }));
                                match pending_message {
                                    Some(prompt) => {
                                        self.start_stream(agent.seal_and_continue(prompt), &rt)
                                    }
                                    None => {
                                        info!(session_id=%self.id, "Stream successfully interrupted (no user msg)");
                                        HarnessState::StreamingInterrupted { agent }
                                    }
                                }
                            }
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
                            let phase = engine.receive(msg);
                            self.continue_tool_phase(agent, phase, &rt)
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
