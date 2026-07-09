use crate::events::{LlmDelta, ModelTurnOutcome, SessionError, SessionEvent, SessionStatus};
use crate::session_state::{
    AwaitingHostFeedback, AwaitingInterruptedUserMessage, Idle, LlmTurnRunning, SessionState,
};
use either::Either;
use futures::StreamExt;
use provider::{LlmModel, StreamEvent, ToolDefinition, ToolResult, UserPrompt};
use std::sync::mpsc;
use std::time::SystemTime;
use std::{fmt, thread};
use tokio::runtime::Handle;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(Uuid);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({})", self.0)
    }
}

impl SessionId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug)]
pub struct Session {
    id: SessionId,
    model: LlmModel,
    runtime: Handle,
    state: SessionStateEnum,
    command_rx: mpsc::Receiver<SessionCommand>,
    event_tx: mpsc::Sender<SessionEvent>,
    // TODO: eventually this should just be on the thread.
    tools: Vec<ToolDefinition>,
}

/// A handle to a running session.
#[derive(Debug)]
pub struct SessionHandle {
    /// ID of the session this handle is a handle to
    id: SessionId,
    /// Channel to send commands through.
    command_tx: mpsc::Sender<SessionCommand>,
    /// Channel to receive events through.
    event_rx: mpsc::Receiver<SessionEvent>,
}

impl SessionHandle {
    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn command_channel(&self) -> &mpsc::Sender<SessionCommand> {
        &self.command_tx
    }

    pub fn event_channel(&self) -> &mpsc::Receiver<SessionEvent> {
        &self.event_rx
    }
}

/// Snapshot of the current session state.
pub struct SessionSnapshot {
    /// ID of the session
    session_id: SessionId,
    /// ID of the snapshot
    snapshot_id: Uuid,
    /// The time this snapshot was taken.
    snapshot_time: SystemTime,
    // TODO: the other data in the snapshot...
}

/// A command that can be sent to the session
pub enum SessionCommand {
    /// Add a user message. The host is expected to send this whenever
    /// the user sends a message while the session is idle.
    AddUserMessage(UserPrompt),
    /// Add a bunch of tool results.
    /// The host should send this when it has finished executing the tool calls
    /// requested by the model
    AddToolResults(Vec<ToolResult>),
    /// Adds a steering prompt. This prompt rides
    /// back with any tool results.
    AddSteeringPrompt(UserPrompt),
    /// Request a snapshot of the current session state
    Snapshot {
        /// Channel for the session loop to send the snapshot back on.
        reply: mpsc::SyncSender<SessionSnapshot>,
    },
    /// Request that the session interrupt its current work.
    Interrupt,
    /// Request to terminate the session.
    Shutdown,
}

impl Session {
    /// Start a new session with the given system prompt and tools.
    /// The session starts running in an OS thread.
    /// The tokio runtime handle must be passed in for the session to
    /// stream responses from the model using async.
    pub fn start(
        system_prompt: String,
        tools: Vec<ToolDefinition>,
        model: LlmModel,
        runtime: Handle,
    ) -> SessionHandle {
        let id = SessionId::new();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let sess_state = SessionState::<Idle>::new(system_prompt);
        let mut session = Session {
            id,
            model,
            runtime,
            command_rx,
            event_tx,
            tools,
            state: sess_state.into(),
        };

        thread::spawn(move || {
            session.run();
        });

        SessionHandle {
            id,
            command_tx,
            event_rx,
        }
    }

    /// Run the session. This will block until the session is complete.
    /// Commands should be sent via the sending half of the channel,
    /// which is in SessionHandle.
    fn run(&mut self) {
        info!(id = %self.id, "Starting session");
        let _ = self
            .event_tx
            .send(SessionEvent::StateChanged(SessionStatus::Idle));

        loop {
            let command = match self.command_rx.recv() {
                Ok(command) => command,
                Err(_) => break,
            };

            match command {
                SessionCommand::Shutdown => {
                    let _ = self.event_tx.send(SessionEvent::Shutdown);
                    break;
                }
                SessionCommand::Snapshot { reply } => {
                    let snapshot = self.snapshot();
                    let _ = reply.send(snapshot);
                }
                SessionCommand::Interrupt => {
                    let state = std::mem::replace(&mut self.state, SessionStateEnum::Poisoned);
                    self.state = handle_interrupt(state);
                    let _ = self.event_tx.send(SessionEvent::Interrupted);
                    match &self.state {
                        SessionStateEnum::Idle(_) => {
                            let _ = self
                                .event_tx
                                .send(SessionEvent::StateChanged(SessionStatus::Idle));
                        }
                        SessionStateEnum::AwaitingHostFeedback(_) => {
                            let _ = self.event_tx.send(SessionEvent::StateChanged(
                                SessionStatus::AwaitingHostFeedback,
                            ));
                        }
                        SessionStateEnum::AwaitingInterruptedUserMessage(_) => {
                            let _ = self.event_tx.send(SessionEvent::StateChanged(
                                SessionStatus::AwaitingInterruptedUserMessage,
                            ));
                        }
                        SessionStateEnum::LlmTurnRunning { .. } => {
                            unreachable!(
                                "interrupting an LLM turn should settle into idle or host feedback"
                            );
                        }
                        SessionStateEnum::Poisoned => {
                            let message = "session entered poisoned state while handling interrupt"
                                .to_string();
                            error!(message = %message);
                            let _ = self
                                .event_tx
                                .send(SessionEvent::Error(SessionError::Internal(message)));
                        }
                    }
                }
                // Any other commands require LLM response.
                command => {
                    let state = std::mem::replace(&mut self.state, SessionStateEnum::Poisoned);
                    self.state = handle_event(state, command, &self.event_tx);
                    self.stream_llm_response();
                }
            }
        }

        info!(id = %self.id, "Session stopped");
    }

    fn stream_llm_response(&mut self) {
        let state = std::mem::replace(&mut self.state, SessionStateEnum::Poisoned);

        self.state = match state {
            SessionStateEnum::LlmTurnRunning {
                state,
                mut partial_response,
            } => {
                let request = state.create_request(self.tools.clone());
                let partial_response = self.runtime.block_on(async {
                    let mut stream = match self.model.stream(request).await {
                        Ok(stream) => stream,
                        Err(err) => {
                            let _ = self
                                .event_tx
                                .send(SessionEvent::Error(SessionError::Model(err.to_string())));
                            return partial_response;
                        }
                    };

                    while let Some(event) = stream.next().await {
                        match event {
                            Ok(event) => {
                                let done = matches!(event, StreamEvent::Done { .. });
                                match &event {
                                    StreamEvent::TextDelta(delta) => {
                                        let _ = self.event_tx.send(SessionEvent::LlmDelta(
                                            LlmDelta::AssistantContent(delta.clone()),
                                        ));
                                    }
                                    StreamEvent::ReasoningDelta(delta) => {
                                        let _ = self.event_tx.send(SessionEvent::LlmDelta(
                                            LlmDelta::AssistantReasoning(delta.clone()),
                                        ));
                                    }
                                    StreamEvent::ToolCall {
                                        id,
                                        name,
                                        arguments,
                                    }
                                    | StreamEvent::ToolCallComplete {
                                        id,
                                        name,
                                        arguments,
                                    } => {
                                        let _ = self.event_tx.send(SessionEvent::LlmDelta(
                                            LlmDelta::ToolCall {
                                                id: id.clone(),
                                                name: name.clone(),
                                                arguments: arguments.clone(),
                                            },
                                        ));
                                    }
                                    StreamEvent::Done { .. } => {}
                                }
                                partial_response.push(event);
                                if done {
                                    break;
                                }
                            }
                            Err(err) => {
                                let _ = self.event_tx.send(SessionEvent::Error(
                                    SessionError::Model(err.to_string()),
                                ));
                                return partial_response;
                            }
                        }
                    }

                    partial_response
                });

                let mut usage = None;
                let mut stop_reason = None;
                for event in &partial_response {
                    if let StreamEvent::Done {
                        usage: event_usage,
                        stop_reason: event_stop_reason,
                    } = event
                    {
                        usage = event_usage.clone();
                        stop_reason = event_stop_reason.clone();
                    }
                }

                let next_state: SessionStateEnum = match state.abandon_llm_turn(partial_response) {
                    Either::Left(state) => state.into(),
                    Either::Right(state) => state.into(),
                };
                match &next_state {
                    SessionStateEnum::AwaitingHostFeedback(state) => {
                        let _ = self.event_tx.send(SessionEvent::ModelTurnDone(
                            ModelTurnOutcome::NeedsToolResults {
                                tool_calls: state.requested_tool_calls(),
                            },
                        ));
                        let _ = self.event_tx.send(SessionEvent::StateChanged(
                            SessionStatus::AwaitingHostFeedback,
                        ));
                    }
                    _ => {
                        let _ = self.event_tx.send(SessionEvent::ModelTurnDone(
                            ModelTurnOutcome::AssistantMessageComplete { usage, stop_reason },
                        ));
                        let _ = self
                            .event_tx
                            .send(SessionEvent::StateChanged(SessionStatus::Idle));
                    }
                }
                next_state
            }
            state => state,
        };
    }

    /// Take a snapshot of the current session state.
    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            session_id: self.id,
            snapshot_id: Uuid::new_v4(),
            snapshot_time: SystemTime::now(),
        }
    }
}

// forced type erasure unfortunately...
#[derive(Debug)]
pub(crate) enum SessionStateEnum {
    Idle(SessionState<Idle>),
    LlmTurnRunning {
        state: SessionState<LlmTurnRunning>,
        partial_response: Vec<StreamEvent>,
    },
    AwaitingHostFeedback(SessionState<AwaitingHostFeedback>),
    AwaitingInterruptedUserMessage(SessionState<AwaitingInterruptedUserMessage>),
    /// This is a bad state. This happens if the event or interrupt handling fails.
    Poisoned,
}

impl From<SessionState<Idle>> for SessionStateEnum {
    fn from(state: SessionState<Idle>) -> Self {
        SessionStateEnum::Idle(state)
    }
}

impl From<SessionState<LlmTurnRunning>> for SessionStateEnum {
    fn from(state: SessionState<LlmTurnRunning>) -> Self {
        SessionStateEnum::LlmTurnRunning {
            state,
            partial_response: Vec::new(),
        }
    }
}

impl From<SessionState<AwaitingHostFeedback>> for SessionStateEnum {
    fn from(state: SessionState<AwaitingHostFeedback>) -> Self {
        SessionStateEnum::AwaitingHostFeedback(state)
    }
}

impl From<SessionState<AwaitingInterruptedUserMessage>> for SessionStateEnum {
    fn from(state: SessionState<AwaitingInterruptedUserMessage>) -> Self {
        SessionStateEnum::AwaitingInterruptedUserMessage(state)
    }
}

fn handle_event(
    state: SessionStateEnum,
    event: SessionCommand,
    event_tx: &mpsc::Sender<SessionEvent>,
) -> SessionStateEnum {
    match (state, event) {
        (SessionStateEnum::Idle(state), SessionCommand::AddUserMessage(prompt)) => {
            let _ = event_tx.send(SessionEvent::StateChanged(SessionStatus::LlmTurnRunning));
            state.add_user_message(prompt).into()
        }

        (
            SessionStateEnum::AwaitingInterruptedUserMessage(state),
            SessionCommand::AddUserMessage(prompt),
        ) => {
            let _ = event_tx.send(SessionEvent::StateChanged(SessionStatus::LlmTurnRunning));
            state.add_user_message(prompt).into()
        }

        (
            SessionStateEnum::AwaitingHostFeedback(state),
            SessionCommand::AddToolResults(results),
        ) => match state.add_tool_results(results) {
            Ok(Either::Left(state)) => {
                let _ = event_tx.send(SessionEvent::StateChanged(
                    SessionStatus::AwaitingHostFeedback,
                ));
                state.into()
            }
            Ok(Either::Right(state)) => {
                let _ = event_tx.send(SessionEvent::StateChanged(SessionStatus::LlmTurnRunning));
                state.into()
            }
            Err((state, err)) => {
                warn!(error = %err, "Invalid tool result");
                let _ = event_tx.send(SessionEvent::Error(SessionError::InvalidToolResult(
                    err.to_string(),
                )));
                state.into()
            }
        },

        (
            SessionStateEnum::AwaitingHostFeedback(state),
            SessionCommand::AddSteeringPrompt(prompt),
        ) => {
            let _ = event_tx.send(SessionEvent::StateChanged(
                SessionStatus::AwaitingHostFeedback,
            ));
            state.add_steering_prompt(prompt).into()
        }

        (state, _event) => {
            // invalid event for current state; eventually emit/log an error
            state
        }
    }
}

/// Interrupt handler for the session.
/// Attempts to interrupt ongoing work.
///
/// The behaviour of this function is dependent on the session's state:
/// - If the session is idle and waiting for the user to chat,
/// this does nothing.
/// - If the LLM is streaming, then this interrupts the LLM's response
/// halfway and moves back into an idle state
/// - If the session is waiting for the host to provide tool call responses,
/// all tool calls are marked as interrupted, and the session waits for
/// the next user message to ride back with those interrupted tool calls.
fn handle_interrupt(state: SessionStateEnum) -> SessionStateEnum {
    match state {
        SessionStateEnum::Idle(state) => state.into(),

        SessionStateEnum::LlmTurnRunning {
            state,
            partial_response,
        } => match state.abandon_llm_turn(partial_response) {
            Either::Left(state) => state.into(),
            Either::Right(state) => state.into(),
        },

        SessionStateEnum::AwaitingHostFeedback(state) => {
            state.interrupt_pending_tool_calls().into()
        }

        SessionStateEnum::AwaitingInterruptedUserMessage(state) => state.into(),

        SessionStateEnum::Poisoned => SessionStateEnum::Poisoned,
    }
}
