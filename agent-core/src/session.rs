use crate::SystemPrompt;
use crate::events::{LoopEvent, SessionCommand, SessionEvent, HarnessState, NewState};
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::tool_registry::ToolRegistry;
use crate::tools::tool_decisions::{ToolAuthorization, UserToolDecisions};
use agent_tools::Tool;
use auger_driver::{Resolved, Resolving, ToolBatch, TypedAgent};
use provider::{LlmModel, LlmThread};
use provider::UserPrompt;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::mpsc;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

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
            session_state: Some(HarnessState::WaitingForUserMessage { agent: TypedAgent::new(model, system_prompt.into(), tool_defs) }),
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
                LoopEvent::Cmd(cmd) => {
                    match cmd {
                        SessionCommand::SendMessage(prompt) => {
                            if let Some(HarnessState::WaitingForUserMessage { agent }) = self.session_state.take_if(|state| matches!(state, HarnessState::WaitingForUserMessage { .. })) {
                                let agent = agent.add_message(prompt);
                                self.emit_state_transition(HarnessState::ReadyToStream { agent });
                            } else {
                                // TODO: handle interrupted and failed states if user sends message during then.
                                warn!(session_id = %self.id, "Received SendMessage command while not in WaitingForUserMessage state");
                            }
                        }
                        SessionCommand::Interrupt => {}
                        SessionCommand::ToolDecision { id, approved, message } => {
                            if let Some(HarnessState::NeedToolConsent { agent, user_tool_decisions }) = self.session_state.take_if(|state| matches!(state, HarnessState::NeedToolConsent { .. })) {
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
                                            authorization: ToolAuthorization::PerTool(user_tool_decisions),
                                        });
                                    }
                                }
                            } else {
                                warn!(session_id = %self.id, "Received ToolDecision but session isn't waiting for tool decisions");
                            }
                        }
                    }
                }
                LoopEvent::StateTransition(new_state) => {
                    match new_state {
                        // idle state: no need to do anything
                        HarnessState::WaitingForUserMessage { .. } => {}
                        HarnessState::ReadyToStream { agent } => {
                            let streaming_fut = agent.create_stream();
                            let cancel_token = streaming_fut.interrupt_handle();
                            let event_tx = self.event_tx.clone();
                            // TODO: enqueue streaming task, also need to move the event_tx in so the streaming deltas get emitted
                            self.emit_state_transition(HarnessState::Streaming { cancel: cancel_token });
                        }
                        // "idle" state, the future will emit events, we do nothing
                        HarnessState::Streaming { .. } => {}
                        HarnessState::HasToolCalls { agent } => {
                            if self.auto_approval_policy.will_approve_all(agent.tool_names_requested()) {
                                self.emit_state_transition(HarnessState::ReadyToRunTools { agent, authorization: ToolAuthorization::AllAutoApproved });
                            } else {
                                let ids_needing_consent = self.auto_approval_policy.ids_needing_consent(agent.get_requested_tools());
                                self.emit_state_transition(HarnessState::NeedToolConsent { agent, user_tool_decisions: UserToolDecisions::new_undecided(ids_needing_consent) });
                            }
                        }
                        HarnessState::ReadyToRunTools { agent, authorization } => {
                            // TODO: enqueue task on async executor to run all tools, transition to WaitingForToolResults
                            // the future that runs the tools will emit a state transition into HarnessState::ReadyToStream
                        }
                        // "idle" state: the future that runs the tools will emit a state transition into HarnessState::ReadyToStream
                        HarnessState::WaitingForToolResults { .. } => {}
                        // idle state: user has to provide a response.
                        HarnessState::NeedToolConsent { .. } => {}
                    }
                }
            }
        }

        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }

    /// Send an internal event signalling that the state has transitioned.
    fn emit_state_transition(&self, transition: HarnessState) {
        self.loop_event_tx.send(LoopEvent::StateTransition(transition)).expect("failed to send state transition");
    }

}
