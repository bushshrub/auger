use crate::session_state::{
    AwaitingHostFeedback, AwaitingInterruptedUserMessage, Idle, LlmTurnRunning, SessionState,
};
use either::Either;
use futures::StreamExt;
use provider::{
    ClankerMessage, LlmError, LlmModel, LlmResponse, StreamEvent, ToolCallRequest, ToolDefinition,
    ToolResult, UserPrompt,
};
use std::sync::mpsc;
use std::time::SystemTime;
use std::{fmt, thread};
use tokio::runtime::Handle;
use tracing::{info, warn};
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
    // TODO: eventually this should just be on the thread.
    tools: Vec<ToolDefinition>,
}

/// A handle to a running session.
#[derive(Debug)]
pub struct SessionHandle {
    /// ID of the session this handle is a handle to
    id: SessionId,
    commands: mpsc::Sender<SessionCommand>,
    events: mpsc::Receiver<SessionEvent>,
}

impl SessionHandle {
    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn commands(&self) -> &mpsc::Sender<SessionCommand> {
        &self.commands
    }

    pub fn events(&self) -> &mpsc::Receiver<SessionEvent> {
        &self.events
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
    SubmitInput(UserPrompt),
    /// Add a bunch of tool results.
    /// The host should send this when it has finished executing the tool calls
    /// requested by the model
    SubmitHostFeedback(Vec<ToolResult>),
    /// Adds a steering prompt. This prompt rides
    /// back with any tool results.
    AddSteeringPrompt(UserPrompt),
    /// Retry the current response after a streaming failure.
    RetryResponse,
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

#[derive(Debug, Clone)]
pub enum SessionEvent {
    StreamingStarted,
    StreamEvent(StreamEvent),
    FinalAssistantResponse(ClankerMessage),
    ModelToolCallBatchReady(Vec<ToolCallRequest>),
    Interrupted,
    StreamFailed(LlmError),
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
            tools,
            state: sess_state.into(),
        };

        thread::spawn(move || {
            session.run(command_rx, event_tx);
        });

        SessionHandle {
            id,
            commands: command_tx,
            events: event_rx,
        }
    }

    /// Run the session. This will block until the session is complete.
    /// Commands should be sent via the sending half of the channel,
    /// which is in SessionHandle.
    fn run(
        &mut self,
        command_recv: mpsc::Receiver<SessionCommand>,
        event_tx: mpsc::Sender<SessionEvent>,
    ) {
        info!(id = %self.id, "Starting session");

        loop {
            let command = match command_recv.recv() {
                Ok(command) => command,
                Err(_) => break,
            };

            match command {
                SessionCommand::Shutdown => break,
                SessionCommand::Snapshot { reply } => {
                    let snapshot = self.snapshot();
                    let _ = reply.send(snapshot);
                }
                SessionCommand::Interrupt => {
                    let state = std::mem::replace(&mut self.state, SessionStateEnum::Poisoned);
                    self.state = handle_interrupt(state);
                    let _ = event_tx.send(SessionEvent::Interrupted);
                }
                // Any other commands require LLM response.
                command => {
                    let state = std::mem::replace(&mut self.state, SessionStateEnum::Poisoned);
                    self.state = handle_event(state, command);
                    self.stream_llm_response(&event_tx);
                }
            }
        }

        info!(id = %self.id, "Session stopped");
    }

    fn stream_llm_response(&mut self, event_tx: &mpsc::Sender<SessionEvent>) {
        let state = std::mem::replace(&mut self.state, SessionStateEnum::Poisoned);

        self.state = match state {
            SessionStateEnum::LlmTurnRunning {
                state,
                mut partial_response,
            } => {
                let request = state.create_request(self.tools.clone());
                let _ = event_tx.send(SessionEvent::StreamingStarted);
                let stream_result = self.runtime.block_on(async {
                    let mut stream = match self.model.stream(request).await {
                        Ok(stream) => stream,
                        Err(err) => return Err((partial_response, err)),
                    };

                    while let Some(event) = stream.next().await {
                        match event {
                            Ok(event) => {
                                let done = matches!(event, StreamEvent::Done { .. });
                                let _ = event_tx.send(SessionEvent::StreamEvent(event.clone()));
                                partial_response.push(event);
                                if done {
                                    break;
                                }
                            }
                            Err(err) => return Err((partial_response, err)),
                        }
                    }

                    Ok(partial_response)
                });

                match stream_result {
                    Ok(partial_response) => commit_llm_response(state, partial_response, event_tx),
                    Err((partial_response, err)) => {
                        let _ = event_tx.send(SessionEvent::StreamFailed(err));
                        SessionStateEnum::LlmTurnRunning {
                            state,
                            partial_response,
                        }
                    }
                }
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

fn handle_event(state: SessionStateEnum, event: SessionCommand) -> SessionStateEnum {
    match (state, event) {
        (SessionStateEnum::Idle(state), SessionCommand::SubmitInput(prompt)) => {
            state.add_user_message(prompt).into()
        }

        (
            SessionStateEnum::AwaitingInterruptedUserMessage(state),
            SessionCommand::SubmitInput(prompt),
        ) => state.add_user_message(prompt).into(),

        (
            SessionStateEnum::AwaitingHostFeedback(state),
            SessionCommand::SubmitHostFeedback(results),
        ) => {
            if let Err(err) = state.validate_tool_results(&results) {
                warn!(error = %err, "Invalid tool result");
                return state.into();
            }

            match state.add_tool_results(results) {
                Ok(Either::Left(state)) => state.into(),
                Ok(Either::Right(state)) => state.into(),
                Err(err) => {
                    unreachable!("tool results were prevalidated: {err}");
                }
            }
        }

        (
            SessionStateEnum::LlmTurnRunning {
                state,
                partial_response,
            },
            SessionCommand::RetryResponse,
        ) => SessionStateEnum::LlmTurnRunning {
            state,
            partial_response,
        },

        (SessionStateEnum::LlmTurnRunning { state, .. }, SessionCommand::SubmitInput(prompt)) => {
            state.abandon_and_add_user_message(prompt).into()
        }

        (
            SessionStateEnum::AwaitingHostFeedback(state),
            SessionCommand::AddSteeringPrompt(prompt),
        ) => state.add_steering_prompt(prompt).into(),

        (state, _event) => {
            // invalid event for current state; eventually emit/log an error
            state
        }
    }
}

fn commit_llm_response(
    state: SessionState<LlmTurnRunning>,
    partial_response: Vec<StreamEvent>,
    event_tx: &mpsc::Sender<SessionEvent>,
) -> SessionStateEnum {
    let message = ClankerMessage::from(LlmResponse::from(partial_response));
    match state.add_llm_response(message.clone()) {
        Either::Left(state) => {
            let _ = event_tx.send(SessionEvent::FinalAssistantResponse(message));
            state.into()
        }
        Either::Right(state) => {
            let _ = event_tx.send(SessionEvent::ModelToolCallBatchReady(
                state.pending_tool_calls(),
            ));
            state.into()
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
