use crate::SystemPrompt;
use crate::events::{HarnessState, LoopEvent, SessionCommand, SessionEvent};
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::tool_decisions::{ToolAuthorization, UserToolDecisions};
use crate::tools::tool_registry::ToolRegistry;
use agent_tools::Tool;
use auger_driver::TypedAgent;
use provider::{LlmModel, UserPrompt};
use std::fmt;
use std::sync::mpsc;
use tokio::runtime::Handle;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(uuid::Uuid);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SessionId {
    pub(crate) fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

/// A handle to a running auger session
pub struct SessionHandle {
    id: SessionId,
    loop_event_tx: mpsc::Sender<LoopEvent>,
    event_rx: mpsc::Receiver<SessionEvent>,
}

impl SessionHandle {
    fn new(
        id: SessionId,
        command_tx: mpsc::Sender<LoopEvent>,
        event_rx: mpsc::Receiver<SessionEvent>,
    ) -> Self {
        Self {
            id,
            loop_event_tx: command_tx,
            event_rx,
        }
    }

    /// Receive the next event emitted by the session.
    pub fn recv_event(&self) -> Result<SessionEvent, mpsc::RecvError> {
        self.event_rx.recv()
    }

    pub fn send_message(&self, prompt: UserPrompt) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopEvent::Cmd(SessionCommand::SendMessage(prompt)))
            .map_err(|_| ())
    }

    pub fn interrupt(&self) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopEvent::Cmd(SessionCommand::Interrupt))
            .map_err(|_| ())
    }

    pub fn approve_tool_call(&self, id: impl Into<String>) -> Result<(), ()> {
        self.tool_decision(id, true, None)
    }

    pub fn deny_tool_call(&self, id: impl Into<String>) -> Result<(), ()> {
        self.tool_decision(id, false, Some("Denied by user".to_string()))
    }

    fn tool_decision(
        &self,
        id: impl Into<String>,
        approved: bool,
        message: Option<String>,
    ) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopEvent::Cmd(SessionCommand::ToolDecision {
                id: id.into(),
                approved,
                message,
            }))
            .map_err(|_| ())
    }

    /// Stop the session.
    pub fn stop(self) {
        todo!()
    }
}

pub struct Session {
    id: SessionId,

    session_state: Option<HarnessState>,
    /// Receiver to receive session commands and agent events from
    inbox: mpsc::Receiver<LoopEvent>,
    loop_event_tx: mpsc::Sender<LoopEvent>,
    /// Sender for the session to emit events through
    event_tx: mpsc::Sender<SessionEvent>,
    tool_registry: std::sync::Arc<ToolRegistry>,
    auto_approval_policy: std::sync::Arc<AutoApprovalPolicy>,
}

impl Session {
    pub fn start(model: LlmModel, system_prompt: SystemPrompt, rt: Handle) -> SessionHandle {
        Self::start_with_tools(model, system_prompt, rt, Vec::new(), Vec::new())
    }

    pub fn start_with_tools(
        model: LlmModel,
        system_prompt: SystemPrompt,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approved_tools: Vec<String>,
    ) -> SessionHandle {
        let mut tool_registry = ToolRegistry::new();
        for tool in tools {
            tool_registry.register(tool);
        }
        let tool_registry = std::sync::Arc::new(tool_registry);
        let tool_defs = tool_registry.list_for_clanker();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let session = Self {
            id: SessionId::new(),
            inbox: cmd_rx,
            loop_event_tx: cmd_tx.clone(),
            event_tx,
            session_state: Some(HarnessState::WaitingForUserMessage {
                agent: TypedAgent::new(model, system_prompt.into(), tool_defs),
            }),
            tool_registry,
            auto_approval_policy: std::sync::Arc::new(AutoApprovalPolicy::new(auto_approved_tools)),
        };
        let handle = SessionHandle::new(session.id, cmd_tx, event_rx);

        std::thread::Builder::new()
            .name(format!("auger-session-{}", session.id.0))
            .spawn(move || session.run(rt))
            .expect("failed to spawn session thread");

        handle
    }

    fn run(mut self, rt: Handle) {
        info!(session_id = %self.id, "Session started");
        while let Ok(event) = self.inbox.recv() {
            match event {
                LoopEvent::Cmd(cmd) => match cmd {
                    SessionCommand::SendMessage(prompt) => match self.session_state.take() {
                        Some(HarnessState::WaitingForUserMessage { agent }) => {
                            self.emit_state_transition(HarnessState::ReadyToStream {
                                agent: agent.add_message(prompt),
                            });
                        }
                        Some(HarnessState::StreamingInterrupted { agent }) => {
                            self.emit_state_transition(HarnessState::ReadyToStream {
                                agent: agent.add_message_to_continue(prompt, true),
                            });
                        }
                        Some(HarnessState::StreamingFailed { agent }) => {
                            self.emit_state_transition(HarnessState::ReadyToStream {
                                agent: agent.add_message_to_continue(prompt),
                            });
                        }
                        Some(HarnessState::Streaming { cancel }) => {
                            // TODO: add a message queue that lets the user steer the current turn
                            // or enqueue the message for the next turn.
                            self.session_state = Some(HarnessState::Streaming { cancel });
                            warn!(session_id = %self.id, "Rejected SendMessage while streaming");
                        }
                        state => {
                            self.session_state = state;
                            warn!(session_id = %self.id, "Received SendMessage command while not ready for a message");
                        }
                    },
                    SessionCommand::Interrupt => {
                        if let Some(HarnessState::Streaming { cancel, .. }) =
                            self.session_state.as_ref()
                        {
                            cancel.cancel();
                        }
                    }
                    SessionCommand::ToolDecision {
                        id,
                        approved,
                        message,
                    } => {
                        if let Some(HarnessState::NeedToolConsent {
                            agent,
                            user_tool_decisions,
                        }) = self
                            .session_state
                            .take_if(|state| matches!(state, HarnessState::NeedToolConsent { .. }))
                        {
                            if !user_tool_decisions.decision_needed(&id) {
                                warn!(session_id = %self.id, "Received ToolDecision for tool id {}, but it does not need a decision", id);
                                self.emit_state_transition(HarnessState::NeedToolConsent {
                                    agent,
                                    user_tool_decisions,
                                });
                                continue;
                            }

                            match user_tool_decisions.record_decision(id, approved, message) {
                                either::Either::Left(user_tool_decisions) => {
                                    self.emit_state_transition(HarnessState::NeedToolConsent {
                                        agent,
                                        user_tool_decisions,
                                    });
                                }
                                either::Either::Right(user_tool_decisions) => {
                                    info!(session_id = %self.id, "All tool decisions have been made, transitioning to ReadyToRunTools");
                                    self.emit_state_transition(HarnessState::ReadyToRunTools {
                                        agent,
                                        authorization: ToolAuthorization::PerTool(
                                            user_tool_decisions,
                                        ),
                                    });
                                }
                            }
                        } else {
                            warn!(session_id = %self.id, "Received ToolDecision but session isn't waiting for tool decisions");
                        }
                    }
                },
                LoopEvent::StreamResult(result) => {
                    let state: HarnessState = result.into();
                    self.emit_state_transition(state);
                }
                LoopEvent::StateTransition(new_state) => {
                    match new_state {
                        // idle state: no need to do anything
                        HarnessState::WaitingForUserMessage { agent } => {
                            self.session_state =
                                Some(HarnessState::WaitingForUserMessage { agent });
                        }
                        HarnessState::ReadyToStream { agent } => {
                            let event_tx = self.event_tx.clone();
                            let agent = agent.add_event_callback(move |event| {
                                let _ = event_tx.send(SessionEvent::StreamEvent(event));
                            });
                            let streaming_fut = agent.create_stream();
                            let cancel_token = streaming_fut.interrupt_handle();
                            let loop_event_tx = self.loop_event_tx.clone();
                            rt.spawn(async move {
                                let _ = loop_event_tx
                                    .send(LoopEvent::StreamResult(streaming_fut.await));
                            });
                            self.session_state = Some(HarnessState::Streaming {
                                cancel: cancel_token,
                            });
                        }
                        HarnessState::Streaming { cancel } => {
                            self.session_state = Some(HarnessState::Streaming { cancel });
                        }
                        HarnessState::StreamingInterrupted { agent } => {
                            self.session_state = Some(HarnessState::StreamingInterrupted { agent });
                        }
                        HarnessState::StreamingFailed { agent } => {
                            self.session_state = Some(HarnessState::StreamingFailed { agent });
                        }
                        HarnessState::HasToolCalls { agent } => {
                            if self
                                .auto_approval_policy
                                .will_approve_all(agent.tool_names_requested())
                            {
                                self.emit_state_transition(HarnessState::ReadyToRunTools {
                                    agent,
                                    authorization: ToolAuthorization::AllAutoApproved,
                                });
                            } else {
                                let ids_needing_consent = self
                                    .auto_approval_policy
                                    .ids_needing_consent(agent.get_requested_tools());
                                self.emit_state_transition(HarnessState::NeedToolConsent {
                                    agent,
                                    user_tool_decisions: UserToolDecisions::new_undecided(
                                        ids_needing_consent,
                                    ),
                                });
                            }
                        }
                        HarnessState::ReadyToRunTools {
                            agent,
                            authorization,
                        } => {
                            // TODO: enqueue task on async executor to run all tools, transition to WaitingForToolResults
                            // the future that runs the tools will emit a state transition into HarnessState::ReadyToStream
                        }
                        // "idle" state: the future that runs the tools will emit a state transition into HarnessState::ReadyToStream
                        HarnessState::WaitingForToolResults { cancel } => {
                            self.session_state =
                                Some(HarnessState::WaitingForToolResults { cancel });
                        }
                        // idle state: user has to provide a response.
                        HarnessState::NeedToolConsent {
                            agent,
                            user_tool_decisions,
                        } => {
                            self.session_state = Some(HarnessState::NeedToolConsent {
                                agent,
                                user_tool_decisions,
                            });
                        }
                    }
                }
            }
        }

        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }

    /// Send an internal event signalling that the state has transitioned.
    fn emit_state_transition(&self, transition: HarnessState) {
        self.loop_event_tx
            .send(LoopEvent::StateTransition(transition))
            .expect("failed to send state transition");
    }
}
