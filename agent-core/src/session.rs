use std::fmt;
use std::sync::mpsc;
use tracing::info;
use auger_driver::{Agent, WaitingForUserMessage};
use provider::LlmModel;
use crate::events::{SessionCommand, SessionEvent};
use crate::SystemPrompt;

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
    command_tx: mpsc::Sender<SessionCommand>,
    event_rx: mpsc::Receiver<SessionEvent>,
}

impl SessionHandle {
    fn new(
        id: SessionId,
        command_tx: mpsc::Sender<SessionCommand>,
        event_rx: mpsc::Receiver<SessionEvent>,
    ) -> Self {
        Self {
            id,
            command_tx,
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
    agent: AgentState,
    /// Receiver to receive session commands from.
    cmd_rx: mpsc::Receiver<SessionCommand>,
    /// Sender for the session to emit events through
    event_tx: mpsc::Sender<SessionEvent>,
}

pub(crate) enum AgentState {
    Idle(Agent<WaitingForUserMessage>)
}

impl Session {
    pub fn start(model: LlmModel, system_prompt: SystemPrompt) -> SessionHandle {
        // TODO: pass tools
        let tool_defs = Vec::new();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let session = Self {
            id: SessionId::new(),
            agent: AgentState::Idle(Agent::new(model, system_prompt.into(), tool_defs)),
            cmd_rx,
            event_tx,
        };
        let handle = SessionHandle::new(session.id, cmd_tx, event_rx);

        std::thread::Builder::new()
            .name(format!("auger-session-{}", session.id.0))
            .spawn(move || session.run())
            .expect("failed to spawn session thread");

        handle
    }

    fn run(self) {
        info!(session_id = %self.id, "Session started");
        while let Ok(command) = self.cmd_rx.recv() {
            match command {
                SessionCommand::SendMessage(_) => {}
                SessionCommand::Interrupt => {}
                SessionCommand::ApproveToolCall { .. } => {}
                SessionCommand::DenyToolCall { .. } => {}
            }
        }

        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }
}
