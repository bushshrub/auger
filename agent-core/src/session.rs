use std::fmt;
use std::sync::mpsc;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use auger_driver::{
    Agent, LlmStreamingFailed, LlmStreamingInterrupted, StreamResult, WaitingForToolResponses,
    WaitingForUserMessage,
};
use provider::LlmModel;
use provider::UserPrompt;
use crate::events::{LoopEvent, SessionCommand, SessionEvent};
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

    /// Send a user message to the session.
    pub fn send_message(
        &self,
        message: UserPrompt,
    ) -> Result<(), mpsc::SendError<SessionCommand>> {
        self.loop_event_tx
            .send(LoopEvent::Cmd(SessionCommand::SendMessage(message)))
            .map_err(|error| match error.0 {
                LoopEvent::Cmd(command) => mpsc::SendError(command),
                LoopEvent::StreamCompletion(_) => {
                    unreachable!("the session handle only sends commands")
                }
            })
    }

    /// Stop the session.
    pub fn stop(self) {
        todo!()
    }
}

pub struct Session {
    id: SessionId,
    agent: AgentState,
    /// Receiver to receive session commands and agent events from
    inbox: mpsc::Receiver<LoopEvent>,
    loop_event_tx: mpsc::Sender<LoopEvent>,
    /// Sender for the session to emit events through
    event_tx: mpsc::Sender<SessionEvent>,
}

pub(crate) enum AgentState {
    Idle(Agent<WaitingForUserMessage>),
    Streaming(CancellationToken),
    WaitingForToolResponses(Agent<WaitingForToolResponses>),
    Interrupted(Agent<LlmStreamingInterrupted>),
    Failed(Agent<LlmStreamingFailed>),
}

impl Session {
    pub fn start(model: LlmModel, system_prompt: SystemPrompt, rt: Handle) -> SessionHandle {
        // TODO: pass tools
        let tool_defs = Vec::new();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let session = Self {
            id: SessionId::new(),
            agent: AgentState::Idle(Agent::new(model, system_prompt.into(), tool_defs)),
            inbox: cmd_rx,
            loop_event_tx: cmd_tx.clone(),
            event_tx,
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
                LoopEvent::Cmd(command) => match command {
                    SessionCommand::SendMessage(message) => {
                        let agent = match std::mem::replace(
                            &mut self.agent,
                            AgentState::Streaming(CancellationToken::new()),
                        ) {
                            AgentState::Idle(agent) => agent,
                            other => {
                                self.agent = other;
                                continue;
                            }
                        };

                        let event_tx = self.event_tx.clone();
                        let stream = agent
                            .add_message(message)
                            .add_event_callback(move |event| {
                                let _ = event_tx.send(SessionEvent::StreamEvent(event));
                            })
                            .create_stream();
                        let cancellation = stream.interrupt_handle();
                        self.agent = AgentState::Streaming(cancellation);
                        let loop_event_tx = self.loop_event_tx.clone();
                        rt.spawn(async move {
                            info!(session_id = %self.id, "Starting stream");
                            let result = stream.await;
                            info!(session_id = %self.id, "Stream completed");
                            let _ = loop_event_tx.send(LoopEvent::StreamCompletion(result));
                        });
                    }
                    SessionCommand::Interrupt => {
                        if let AgentState::Streaming(cancellation) = &self.agent {
                            info!(session_id = %self.id, "Interrupting current stream");
                            cancellation.cancel();
                        } else {
                            warn!(session_id = %self.id, "Received interrupt command while not streaming");
                        }
                    }
                    // TODO: user approval pathway
                    SessionCommand::ApproveToolCall { .. } => {}
                    SessionCommand::DenyToolCall { .. } => {}
                },
                LoopEvent::StreamCompletion(result) => match result {
                    StreamResult::WaitingForUserMessage(agent) => {
                        self.agent = AgentState::Idle(agent);
                    }
                    StreamResult::WaitingForToolResponses(agent) => {
                        self.agent = AgentState::WaitingForToolResponses(agent);
                    }
                    StreamResult::Interrupted(agent) => {
                        self.agent = AgentState::Interrupted(agent);
                    }
                    StreamResult::Failed(agent) => {
                        self.agent = AgentState::Failed(agent);
                    }
                },
            }
        }

        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }
}
