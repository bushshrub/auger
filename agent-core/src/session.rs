use crate::SystemPrompt;
use crate::events::{HarnessState, LoopMessage, SessionCommand, SessionEvent};
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::tool_decisions::{ToolAuthorization, UserToolDecisions};
use crate::tools::tool_execution::ToolExecution;
use crate::tools::tool_registry::ToolRegistry;
use agent_tools::Tool;
use auger_driver::{StreamResult, TypedAgent, WaitingForUserMessage};
use provider::{LlmModel, UserPrompt};
use std::fmt;
use std::sync::{mpsc, Arc};
use std::sync::mpsc::Sender;
use either::Either;
use tokio::runtime::Handle;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(uuid::Uuid);

/// A read-only copy of the committed messages in a session thread.
#[derive(Clone, Debug)]
pub struct ThreadSnapshot {
    messages: Vec<provider::Message>,
}

impl ThreadSnapshot {
    pub fn messages(&self) -> &[provider::Message] {
        &self.messages
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("session is closed")]
    SessionClosed,
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SessionId {
    pub(crate) fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub fn as_uuid(self) -> uuid::Uuid {
        self.0
    }

    pub fn from_uuid(id: uuid::Uuid) -> Self {
        Self(id)
    }
}

/// A handle to a running auger session
#[derive(Clone)]
pub struct SessionHandle {
    id: SessionId,
    loop_event_tx: mpsc::Sender<LoopMessage>,
}

/// The unique capability to stop a running session.
pub struct SessionOwner {
    loop_event_tx: mpsc::Sender<LoopMessage>,
}

/// The unique receiver for events emitted by a session.
pub struct SessionEventReceiver {
    event_rx: mpsc::Receiver<SessionEvent>,
}

impl SessionHandle {
    fn new(id: SessionId, command_tx: mpsc::Sender<LoopMessage>) -> Self {
        Self {
            id,
            loop_event_tx: command_tx,
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn send_message(&self, prompt: UserPrompt) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::SendMessage(prompt)))
            .map_err(|_| ())
    }

    pub fn interrupt(&self) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::Interrupt))
            .map_err(|_| ())
    }

    /// Clone the committed conversation thread without changing session state.
    pub fn snapshot(&self) -> Result<ThreadSnapshot, SnapshotError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::Snapshot { reply_tx }))
            .map_err(|_| SnapshotError::SessionClosed)?;
        reply_rx.recv().map_err(|_| SnapshotError::SessionClosed)
    }

    pub fn approve_tool_call(&self, id: impl Into<String>) -> Result<(), ()> {
        self.tool_decision(id, true, None)
    }

    pub fn deny_tool_call(&self, id: impl Into<String>) -> Result<(), ()> {
        self.tool_decision(id, false, Some("Denied by user".to_string()))
    }

    pub fn decide_tool_call(
        &self,
        id: impl Into<String>,
        approved: bool,
        message: Option<String>,
    ) -> Result<(), ()> {
        self.tool_decision(id, approved, message)
    }

    fn tool_decision(
        &self,
        id: impl Into<String>,
        approved: bool,
        message: Option<String>,
    ) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::ToolDecision {
                id: id.into(),
                approved,
                message,
            }))
            .map_err(|_| ())
    }

}

impl SessionOwner {
    /// Stop the session.
    pub fn stop(self) {
        let _ = self
            .loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::Stop));
    }
}

impl SessionEventReceiver {
    /// Receive the next event emitted by the session.
    pub fn recv_event(&self) -> Result<SessionEvent, mpsc::RecvError> {
        self.event_rx.recv()
    }
}

pub struct Session {
    id: SessionId,
    /// Receiver to receive session commands and agent events from
    cmd_rx: mpsc::Receiver<LoopMessage>,
    harness_internal_event_tx: Sender<LoopMessage>,
    /// Sender for the session to emit events through
    event_tx: mpsc::Sender<SessionEvent>,
    tool_registry: Arc<ToolRegistry>,
    auto_approval_policy: Arc<AutoApprovalPolicy>,
}

impl Session {
    pub fn start(
        model: LlmModel,
        system_prompt: SystemPrompt,
        rt: Handle,
    ) -> (SessionOwner, SessionHandle, SessionEventReceiver) {
        Self::start_with_tools(model, system_prompt, rt, Vec::new(), Vec::new())
    }

    pub fn start_with_tools(
        model: LlmModel,
        system_prompt: SystemPrompt,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approved_tools: Vec<String>,
    ) -> (SessionOwner, SessionHandle, SessionEventReceiver) {
        Self::spawn(
            SessionId::new(),
            model,
            system_prompt,
            None,
            rt,
            tools,
            auto_approved_tools,
        ).expect("new session history is valid")
    }

    /// Restore a session from committed history at a user-input boundary.
    pub fn restore(
        id: SessionId,
        model: LlmModel,
        messages: Vec<provider::Message>,
        rt: Handle,
    ) -> Result<(SessionOwner, SessionHandle, SessionEventReceiver), provider::RestoreThreadError> {
        Self::restore_with_tools(id, model, messages, rt, Vec::new(), Vec::new())
    }

    pub fn restore_with_tools(
        id: SessionId,
        model: LlmModel,
        messages: Vec<provider::Message>,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approved_tools: Vec<String>,
    ) -> Result<(SessionOwner, SessionHandle, SessionEventReceiver), provider::RestoreThreadError> {
        Self::spawn(
            id,
            model,
            SystemPrompt::new(String::new()),
            Some(messages),
            rt,
            tools,
            auto_approved_tools,
        )
    }

    fn spawn(
        id: SessionId,
        model: LlmModel,
        system_prompt: SystemPrompt,
        messages: Option<Vec<provider::Message>>,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approved_tools: Vec<String>,
    ) -> Result<(SessionOwner, SessionHandle, SessionEventReceiver), provider::RestoreThreadError> {
        let mut tool_registry = ToolRegistry::new();
        for tool in tools {
            tool_registry.register(tool);
        }
        let tool_registry = Arc::new(tool_registry);
        let llm_tools = tool_registry.list_for_clanker();
        let init_agent = match messages {
            Some(messages) => TypedAgent::<WaitingForUserMessage>::restore(
                model,
                messages,
                llm_tools,
            )?,
            None => TypedAgent::<WaitingForUserMessage>::new(
                model,
                system_prompt.into(),
                llm_tools,
            ),
        };
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let session = Self {
            id,
            cmd_rx,
            harness_internal_event_tx: cmd_tx.clone(),
            event_tx,
            tool_registry,
            auto_approval_policy: Arc::new(AutoApprovalPolicy::new(auto_approved_tools)),
        };
        let handle = SessionHandle::new(session.id, cmd_tx.clone());
        let owner = SessionOwner {
            loop_event_tx: cmd_tx,
        };
        let events = SessionEventReceiver { event_rx };

        std::thread::Builder::new()
            .name(format!("auger-session-{}", session.id.0))
            .spawn(move || session.run(rt, init_agent))
            .expect("failed to spawn session thread");

        Ok((owner, handle, events))
    }

    fn run(self, rt: Handle, init_agent: TypedAgent<WaitingForUserMessage>) {
        info!(session_id = %self.id, "Session started");
        let mut thread_snapshot = ThreadSnapshot {
            messages: init_agent.snapshot(),
        };
        let mut curr_state = HarnessState::WaitingForUserMessage { agent: init_agent };
        'session_loop: for msg in self.cmd_rx.iter() {
            match msg {
                LoopMessage::Cmd(cmd) => {
                    match cmd {
                        SessionCommand::Stop => break 'session_loop,
                        SessionCommand::Snapshot { reply_tx } => {
                            let _ = reply_tx.send(thread_snapshot.clone());
                        }
                        SessionCommand::SendMessage(prompt) => {
                            info!(session_id = %self.id, "Received user message {:?}", prompt);
                            curr_state = match curr_state {
                                HarnessState::WaitingForUserMessage { agent } => {
                                    let event_tx = self.event_tx.clone();
                                    let new_agent = agent.add_message(prompt).add_event_callback(move |event| {
                                        let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                    });
                                    thread_snapshot = ThreadSnapshot { messages: new_agent.snapshot() };
                                    let inbox_tx = self.harness_internal_event_tx.clone();
                                    let stream_fut = new_agent.create_stream();
                                    let cancel = stream_fut.interrupt_handle();
                                    let sess_id = self.id;
                                    rt.spawn(async move {
                                        info!(session_id=%sess_id, "Starting stream");
                                        let res = stream_fut.await;
                                        inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                    });
                                    HarnessState::Streaming { cancel }}
                                HarnessState::StreamingInterrupted { agent } => {
                                    let event_tx = self.event_tx.clone();
                                    let new_agent = agent
                                        .add_message_to_continue(prompt, true)
                                        .add_event_callback(move |event| {
                                            let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                        });
                                    thread_snapshot = ThreadSnapshot { messages: new_agent.snapshot() };
                                    let inbox_tx = self.harness_internal_event_tx.clone();
                                    let stream_fut = new_agent.create_stream();
                                    let cancel = stream_fut.interrupt_handle();
                                    rt.spawn(async move {
                                        let res = stream_fut.await;
                                        inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                    });
                                    HarnessState::Streaming { cancel }
                                }
                                HarnessState::StreamingFailed { agent } => {
                                    let event_tx = self.event_tx.clone();
                                    let new_agent = agent
                                        .add_message_to_continue(prompt)
                                        .add_event_callback(move |event| {
                                            let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                        });
                                    thread_snapshot = ThreadSnapshot { messages: new_agent.snapshot() };
                                    let inbox_tx = self.harness_internal_event_tx.clone();
                                    let stream_fut = new_agent.create_stream();
                                    let cancel = stream_fut.interrupt_handle();
                                    rt.spawn(async move {
                                        let res = stream_fut.await;
                                        inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                    });
                                    HarnessState::Streaming { cancel }
                                }
                                HarnessState::InterruptingStream { pending_message: None } => {
                                    HarnessState::InterruptingStream {
                                        pending_message: Some(prompt),
                                    }
                                }
                                _ => {
                                    warn!(session_id = %self.id, command = "send_message", "Ignoring command in invalid harness state");
                                    curr_state
                                }
                            }

                        }
                        SessionCommand::Interrupt => {
                            curr_state = match curr_state {
                                HarnessState::Streaming { cancel } => {
                                    cancel.cancel();
                                    HarnessState::InterruptingStream { pending_message: None }
                                }
                                HarnessState::ToolCallsAreRunning { agent, cancel } => {
                                    cancel.cancel();
                                    HarnessState::InterruptingToolExecution { agent }
                                }
                                _ => {
                                    warn!(session_id = %self.id, command = "interrupt", "Ignoring command in invalid harness state");
                                    curr_state
                                }
                            }
                        }
                        SessionCommand::ToolDecision { id, approved, message } => {
                            curr_state = match curr_state {
                                HarnessState::NeedToolConsent { agent, user_tool_decisions } => {
                                    match user_tool_decisions.record_decision(id, approved, message) {
                                        Either::Left(not_all_decided) => {
                                            HarnessState::NeedToolConsent {
                                                agent,
                                                user_tool_decisions: not_all_decided
                                            }
                                        }
                                        Either::Right(all_decided) => {
                                            let execution = ToolExecution::new(
                                                agent.get_batch(),
                                                ToolAuthorization::PerTool(all_decided),
                                                self.tool_registry.clone(),
                                                self.event_tx.clone(),
                                            ).run();
                                            let cancel = execution.interrupt_handle();
                                            let inbox_tx = self.harness_internal_event_tx.clone();
                                            rt.spawn(async move {
                                                let result = execution.await.resolve();
                                                let _ = inbox_tx.send(LoopMessage::ToolBatchExecutionResult(result));
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
                    }
                }
                LoopMessage::StreamResult(res) => {
                    curr_state = match curr_state {
                        HarnessState::Streaming { cancel } => {
                            drop(cancel);
                            match res {
                                StreamResult::Interrupted(_) => {
                                    panic!("stream returned interrupted while harness was still streaming")
                                }
                                StreamResult::Failed(agent) => {
                                    thread_snapshot = ThreadSnapshot { messages: agent.snapshot() };
                                    HarnessState::StreamingFailed { agent }
                                }
                                StreamResult::WaitingForToolResponses(agent) => {
                                    debug!(session_id = %self.id, "agent has called tools");
                                    thread_snapshot = ThreadSnapshot { messages: agent.snapshot() };
                                    let tool_batch = agent.get_requested_tools();
                                    if self.auto_approval_policy.will_approve_all(tool_batch.iter().map(|t| t.name.clone())) {
                                        info!(session_id=%self.id, "automatically running all tools");
                                        let execution = ToolExecution::new(
                                            agent.get_batch(),
                                            ToolAuthorization::AllAutoApproved,
                                            self.tool_registry.clone(),
                                            self.event_tx.clone(),
                                        ).run();
                                        let cancel = execution.interrupt_handle();
                                        let inbox_tx = self.harness_internal_event_tx.clone();
                                        rt.spawn(async move {
                                            let result = execution.await.resolve();
                                            let _ = inbox_tx.send(LoopMessage::ToolBatchExecutionResult(result));
                                        });
                                        HarnessState::ToolCallsAreRunning { agent, cancel }
                                    } else {
                                        info!(session_id=%self.id, "Some tools require consent");
                                        let unapproved = self.auto_approval_policy.ids_needing_consent(tool_batch);
                                        let tool_calls = agent
                                            .get_requested_tools()
                                            .into_iter()
                                            .filter(|call| unapproved.contains(&call.id))
                                            .collect();
                                        let _ = self.event_tx.send(SessionEvent::ToolConsentRequired {
                                            tool_calls,
                                        });
                                        HarnessState::NeedToolConsent { agent, user_tool_decisions: UserToolDecisions::new_undecided(unapproved) }
                                    }
                                }
                                StreamResult::WaitingForUserMessage(agent) => {
                                    info!(session_id=%self.id, "No tools called");
                                    thread_snapshot = ThreadSnapshot { messages: agent.snapshot() };
                                    HarnessState::WaitingForUserMessage { agent }
                                }
                            }
                        }
                        HarnessState::InterruptingStream { pending_message } => match res {
                            StreamResult::Interrupted(agent) => {
                                match pending_message {
                                    Some(prompt) => {
                                        let event_tx = self.event_tx.clone();
                                        let new_agent = agent
                                            .add_message_to_continue(prompt, true)
                                            .add_event_callback(move |event| {
                                                let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                            });
                                        thread_snapshot = ThreadSnapshot { messages: new_agent.snapshot() };
                                        let inbox_tx = self.harness_internal_event_tx.clone();
                                        let stream_fut = new_agent.create_stream();
                                        let cancel = stream_fut.interrupt_handle();
                                        rt.spawn(async move {
                                            let res = stream_fut.await;
                                            inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                        });
                                        HarnessState::Streaming { cancel }
                                    }
                                    None => {
                                        thread_snapshot = ThreadSnapshot { messages: agent.snapshot() };
                                        let _ = self.event_tx.send(SessionEvent::Interrupted);
                                        HarnessState::StreamingInterrupted { agent }
                                    }
                                }
                            }
                            // TODO: we must handle these
                            StreamResult::Failed(_) => {
                                panic!("stream failed while harness was interrupting the stream")
                            }
                            StreamResult::WaitingForToolResponses(_) => {
                                panic!("stream requested tools while harness was interrupting the stream")
                            }
                            StreamResult::WaitingForUserMessage(_) => {
                                panic!("stream completed while harness was interrupting the stream")
                            }
                        },
                        _ => curr_state
                    };
                }
                LoopMessage::ToolBatchExecutionResult(tool_batch) => {
                    curr_state = match curr_state {
                        HarnessState::ToolCallsAreRunning { agent, cancel } => {
                            drop(cancel);
                            let new_agent = agent.add_all_tool_responses(tool_batch);
                            thread_snapshot = ThreadSnapshot { messages: new_agent.snapshot() };
                            let event_tx = self.event_tx.clone();
                            let stream_fut = new_agent.add_event_callback(move |event| {
                                let _ = event_tx.send(SessionEvent::StreamEvent(event));
                            }).create_stream();
                            let cancel = stream_fut.interrupt_handle();
                            let inbox_tx = self.harness_internal_event_tx.clone();
                            rt.spawn(async move {
                                let res = stream_fut.await;
                                inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                            });
                            HarnessState::Streaming { cancel }
                        }
                        HarnessState::InterruptingToolExecution { agent } => {
                            let new_agent = agent.add_all_tool_responses(tool_batch);
                            thread_snapshot = ThreadSnapshot { messages: new_agent.snapshot() };
                            let event_tx = self.event_tx.clone();
                            let stream_fut = new_agent.add_event_callback(move |event| {
                                let _ = event_tx.send(SessionEvent::StreamEvent(event));
                            }).create_stream();
                            let cancel = stream_fut.interrupt_handle();
                            let inbox_tx = self.harness_internal_event_tx.clone();
                            rt.spawn(async move {
                                let res = stream_fut.await;
                                inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                            });
                            HarnessState::Streaming { cancel }
                        }
                        _ => curr_state
                    }
                }
            }
        }


        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }

}
