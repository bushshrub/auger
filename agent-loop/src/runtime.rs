use crate::events::{ModelTurnOutcome, SessionError, SessionEvent, SessionStatus};
use crate::model_stream::{ModelStreamOutcome, ModelStreamTerminal, stream_model};
use crate::session_state::{
    AwaitingHostFeedback, AwaitingInterruptedUserMessage, Idle, LlmTurnRunning, ResponseError,
    SessionState,
};
use either::Either;
use provider::{LlmModel, Message, StreamEvent, ToolDefinition, ToolResult, UserPrompt};
use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::SystemTime;
use std::{fmt, thread};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
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
    inbox_tx: mpsc::Sender<LoopMessage>,
    inbox_rx: mpsc::Receiver<LoopMessage>,
    deferred_commands: VecDeque<SessionCommand>,
    shutdown_requested: bool,
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
    command_tx: SessionCommandSender,
    /// Channel to receive events through.
    event_rx: mpsc::Receiver<SessionEvent>,
}

/// Sender for commands handled by a running session.
#[derive(Clone, Debug)]
pub struct SessionCommandSender {
    inbox_tx: mpsc::Sender<LoopMessage>,
}

impl SessionCommandSender {
    /// Send a command to the session loop.
    pub fn send(&self, command: SessionCommand) -> Result<(), mpsc::SendError<SessionCommand>> {
        match self.inbox_tx.send(LoopMessage::HostCommand(command)) {
            Ok(()) => Ok(()),
            Err(mpsc::SendError(LoopMessage::HostCommand(command))) => {
                Err(mpsc::SendError(command))
            }
            Err(mpsc::SendError(LoopMessage::ModelStreamFinished(_))) => {
                unreachable!("command sender returned an internal loop message")
            }
        }
    }
}

impl SessionHandle {
    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn command_channel(&self) -> &SessionCommandSender {
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
    /// The current coarse session status.
    status: SessionStatus,
    /// The provider conversation state owned by the loop.
    messages: Vec<Message>,
}

impl SessionSnapshot {
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn snapshot_id(&self) -> Uuid {
        self.snapshot_id
    }

    pub fn snapshot_time(&self) -> SystemTime {
        self.snapshot_time
    }

    pub fn status(&self) -> SessionStatus {
        self.status
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}

/// A command that can be sent to the session
#[derive(Debug)]
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
    /// stream responses from the model using async. The runtime must remain
    /// active for the lifetime of the session.
    pub fn start(
        system_prompt: String,
        tools: Vec<ToolDefinition>,
        model: LlmModel,
        runtime: Handle,
    ) -> SessionHandle {
        let id = SessionId::new();
        let (inbox_tx, inbox_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let sess_state = SessionState::<Idle>::new(system_prompt);
        let mut session = Session {
            id,
            model,
            runtime,
            inbox_tx: inbox_tx.clone(),
            inbox_rx,
            deferred_commands: VecDeque::new(),
            shutdown_requested: false,
            event_tx,
            tools,
            state: sess_state.into(),
        };

        thread::spawn(move || {
            session.run();
        });

        SessionHandle {
            id,
            command_tx: SessionCommandSender { inbox_tx },
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
            let message = match self.next_message() {
                Ok(message) => message,
                Err(_) => break,
            };

            match message {
                LoopMessage::ModelStreamFinished(outcome) => {
                    if self.shutdown_requested {
                        let _ = self.event_tx.send(SessionEvent::Shutdown);
                        break;
                    }
                    self.finish_model_stream(outcome);
                }
                LoopMessage::HostCommand(command) if self.should_defer(&command) => {
                    self.deferred_commands.push_back(command);
                }
                LoopMessage::HostCommand(SessionCommand::Shutdown) => {
                    if self.model_stream_active() {
                        self.shutdown_requested = true;
                        self.active_model_turn_mut()
                            .expect("active model turn disappeared")
                            .cancellation_token
                            .cancel();
                    } else {
                        let _ = self.event_tx.send(SessionEvent::Shutdown);
                        break;
                    }
                }
                LoopMessage::HostCommand(SessionCommand::Snapshot { reply }) => {
                    let snapshot = self.snapshot();
                    let _ = reply.send(snapshot);
                }
                LoopMessage::HostCommand(SessionCommand::Interrupt) => {
                    if let Some(active_turn) = self.active_model_turn_mut() {
                        if !active_turn.interrupt_requested {
                            active_turn.interrupt_requested = true;
                            active_turn.cancellation_token.cancel();
                        }
                        continue;
                    }

                    let state = std::mem::replace(&mut self.state, SessionStateEnum::Poisoned);
                    self.state = handle_interrupt(state);
                    let _ = self.event_tx.send(SessionEvent::Interrupted);
                    self.emit_current_status();
                }
                LoopMessage::HostCommand(command) => {
                    let state = std::mem::replace(&mut self.state, SessionStateEnum::Poisoned);
                    self.state = handle_event(state, command, &self.event_tx);
                    self.start_model_stream();
                }
            }
        }

        info!(id = %self.id, "Session stopped");
    }

    fn next_message(&mut self) -> Result<LoopMessage, mpsc::RecvError> {
        if !self.model_stream_active()
            && let Some(command) = self.deferred_commands.pop_front()
        {
            return Ok(LoopMessage::HostCommand(command));
        }

        self.inbox_rx.recv()
    }

    /// Whether the given session command should wait for the current model
    /// stream to finish before processing.
    fn should_defer(&self, command: &SessionCommand) -> bool {
        self.model_stream_active()
            && !matches!(
                command,
                SessionCommand::Shutdown
                    | SessionCommand::Interrupt
                    | SessionCommand::Snapshot { .. }
            )
    }

    /// Start the stream from the model async.
    fn start_model_stream(&mut self) {
        let SessionStateEnum::LlmTurnRunning { state, active_turn } = &mut self.state else {
            return;
        };
        if active_turn.is_some() {
            return;
        }

        let request = state.create_request(self.tools.clone());
        let cancellation_token = CancellationToken::new();
        *active_turn = Some(ActiveModelTurn {
            cancellation_token: cancellation_token.clone(),
            interrupt_requested: false,
        });

        let model = self.model.clone();
        let event_tx = self.event_tx.clone();
        let inbox_tx = self.inbox_tx.clone();
        self.runtime.spawn(async move {
            let outcome = stream_model(model, request, cancellation_token, event_tx).await;
            let _ = inbox_tx.send(LoopMessage::ModelStreamFinished(outcome));
        });
    }

    fn finish_model_stream(&mut self, outcome: ModelStreamOutcome) {
        let state = std::mem::replace(&mut self.state, SessionStateEnum::Poisoned);
        let (state, interrupted) = match state {
            SessionStateEnum::LlmTurnRunning { state, active_turn } => {
                let active_turn =
                    active_turn.expect("model stream finished without an active turn");
                let ModelStreamOutcome {
                    terminal,
                    partial_response,
                } = outcome;
                let interrupted = active_turn.interrupt_requested
                    || matches!(terminal, ModelStreamTerminal::Cancelled);

                let state = if interrupted {
                    match state.user_interrupt_llm_turn(partial_response) {
                        Either::Left(state) => state.into(),
                        Either::Right(state) => state.into(),
                    }
                } else {
                    match terminal {
                        ModelStreamTerminal::Complete => {
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

                            let next_state: SessionStateEnum =
                                match state.abandon_llm_turn(partial_response) {
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
                                        ModelTurnOutcome::AssistantMessageComplete {
                                            usage,
                                            stop_reason,
                                        },
                                    ));
                                    let _ = self
                                        .event_tx
                                        .send(SessionEvent::StateChanged(SessionStatus::Idle));
                                }
                            }
                            next_state
                        }
                        ModelStreamTerminal::Failed(error) => {
                            let (next_state, partial) = state.interrupt_llm_turn(partial_response);
                            if partial.is_empty() {
                                let _ = self.event_tx.send(SessionEvent::Error(error));
                            } else {
                                let _ = self.event_tx.send(SessionEvent::ModelTurnInterrupted {
                                    partial_response: partial,
                                    error,
                                });
                            }
                            let next_state: SessionStateEnum = next_state.into();
                            let _ = self
                                .event_tx
                                .send(SessionEvent::StateChanged(SessionStatus::ResponseError));
                            next_state
                        }
                        ModelStreamTerminal::Cancelled => unreachable!(
                            "cancelled streams should settle through the interrupt path"
                        ),
                    }
                };
                (state, interrupted)
            }
            state => (state, false),
        };
        self.state = state;

        if interrupted {
            let _ = self.event_tx.send(SessionEvent::Interrupted);
            self.emit_current_status();
        }
    }

    fn model_stream_active(&self) -> bool {
        matches!(
            self.state,
            SessionStateEnum::LlmTurnRunning {
                active_turn: Some(_),
                ..
            }
        )
    }

    fn active_model_turn_mut(&mut self) -> Option<&mut ActiveModelTurn> {
        match &mut self.state {
            SessionStateEnum::LlmTurnRunning { active_turn, .. } => active_turn.as_mut(),
            _ => None,
        }
    }

    /// Emit the current state as an event
    fn emit_current_status(&self) {
        match self.state {
            SessionStateEnum::Poisoned => {
                let message = "session entered poisoned state while handling interrupt".to_string();
                error!(message = %message);
                let _ = self
                    .event_tx
                    .send(SessionEvent::Error(SessionError::Internal(message)));
            }
            _ => {
                let _ = self
                    .event_tx
                    .send(SessionEvent::StateChanged(self.state.status()));
            }
        }
    }

    /// Take a snapshot of the current session state.
    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            session_id: self.id,
            snapshot_id: Uuid::new_v4(),
            snapshot_time: SystemTime::now(),
            status: self.state.status(),
            messages: self.state.messages(),
        }
    }
}

// forced type erasure unfortunately...
#[derive(Debug)]
enum SessionStateEnum {
    Idle(SessionState<Idle>),
    LlmTurnRunning {
        state: SessionState<LlmTurnRunning>,
        active_turn: Option<ActiveModelTurn>,
    },
    AwaitingHostFeedback(SessionState<AwaitingHostFeedback>),
    AwaitingInterruptedUserMessage(SessionState<AwaitingInterruptedUserMessage>),
    ResponseError(SessionState<ResponseError>),
    /// This is a bad state. This happens if the event or interrupt handling fails.
    Poisoned,
}

impl SessionStateEnum {
    fn status(&self) -> SessionStatus {
        match self {
            SessionStateEnum::Idle(_) => SessionStatus::Idle,
            SessionStateEnum::LlmTurnRunning { .. } => SessionStatus::LlmTurnRunning,
            SessionStateEnum::AwaitingHostFeedback(_) => SessionStatus::AwaitingHostFeedback,
            SessionStateEnum::AwaitingInterruptedUserMessage(_) => {
                SessionStatus::AwaitingInterruptedUserMessage
            }
            SessionStateEnum::ResponseError(_) => SessionStatus::ResponseError,
            SessionStateEnum::Poisoned => SessionStatus::AwaitingInterruptedUserMessage,
        }
    }

    fn messages(&self) -> Vec<Message> {
        match self {
            SessionStateEnum::Idle(state) => state.messages(),
            SessionStateEnum::LlmTurnRunning { state, .. } => state.messages(),
            SessionStateEnum::AwaitingHostFeedback(state) => state.messages(),
            SessionStateEnum::AwaitingInterruptedUserMessage(state) => state.messages(),
            SessionStateEnum::ResponseError(state) => state.messages(),
            SessionStateEnum::Poisoned => Vec::new(),
        }
    }
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
            active_turn: None,
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

impl From<SessionState<ResponseError>> for SessionStateEnum {
    fn from(state: SessionState<ResponseError>) -> Self {
        SessionStateEnum::ResponseError(state)
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

        (SessionStateEnum::ResponseError(state), SessionCommand::AddUserMessage(prompt)) => {
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
            active_turn: None,
        } => match state.user_interrupt_llm_turn(Vec::new()) {
            Either::Left(state) => state.into(),
            Either::Right(state) => state.into(),
        },

        SessionStateEnum::LlmTurnRunning {
            active_turn: Some(_),
            ..
        } => unreachable!("active model turns must be cancelled before settling an interrupt"),

        SessionStateEnum::AwaitingHostFeedback(state) => {
            state.interrupt_pending_tool_calls().into()
        }

        SessionStateEnum::AwaitingInterruptedUserMessage(state) => state.into(),
        SessionStateEnum::ResponseError(state) => state.into(),

        SessionStateEnum::Poisoned => SessionStateEnum::Poisoned,
    }
}

#[derive(Debug)]
struct ActiveModelTurn {
    cancellation_token: CancellationToken,
    interrupt_requested: bool,
}

enum LoopMessage {
    HostCommand(SessionCommand),
    ModelStreamFinished(ModelStreamOutcome),
}
