use tokio_util::sync::CancellationToken;
use provider::{LlmModel, LlmThread, UserPrompt};
use provider::thread::{ClankerTurn, UserTurn};
use crate::states::streaming::{LlmStreaming, StreamResult};
/// Synchronous state machine for the auger driver.
pub struct Agent<S: State> {
    pub(crate) model: LlmModel,
    pub(crate) state: S,
}

/// A state that the driver can be in.
pub trait State {}

/// The driver is waiting for a user message.
/// Providing a message will begin the LLM stream and
/// transition it to the [`LlmStreaming`] state.
pub struct WaitingForUserMessage {
    pub(crate) thread: LlmThread<UserTurn>,
}

impl State for WaitingForUserMessage {}

impl Agent<WaitingForUserMessage> {

    /// Create a new agent with the given system prompt and model.
    pub fn new(model: LlmModel, system_prompt: String) -> Self {
        let thread = LlmThread::new(system_prompt);
        let state = WaitingForUserMessage { thread };
        Self { model, state }
    }

    /// Add a user message to the driver and transition it to the [`ReadyToStream`] state.
    pub fn add_message(self, msg: UserPrompt) -> Agent<ReadyToStream> {
        let thread = self.state.thread.add_user_message(msg);
        let state = ReadyToStream { thread, event_callback: Box::new(|_| {}) };
        Agent { model: self.model, state }
    }
}

/// The driver is ready to begin streaming the LLM response.
pub struct ReadyToStream {
    thread: LlmThread<ClankerTurn>,
    event_callback: Box<dyn Fn(provider::StreamEvent) + Send + Sync>,
}

impl State for ReadyToStream {}

impl Agent<ReadyToStream> {
    pub fn add_event_callback(
        self,
        cb: impl Fn(provider::StreamEvent) + Send + Sync + 'static,
    ) -> Self {
        let state = ReadyToStream { thread: self.state.thread, event_callback: Box::new(cb) };
        Agent { model: self.model, state }
    }

    /// Creates an interruptible LLM stream future.
    ///
    pub fn create_stream(self) -> LlmStreaming {
        let cancellation = CancellationToken::new();

        LlmStreaming::new(
            self.model,
            self.state.thread,
            self.state.event_callback,
            cancellation,
        )
    }
}

