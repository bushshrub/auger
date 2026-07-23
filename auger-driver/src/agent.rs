use crate::streaming::LlmStreaming as LlmStreamingFuture;
use getset::Getters;
use provider::AssistantResponse;
use provider::LlmModel;
use provider::Message;
use provider::ToolDefinition;
use provider::UserPrompt;
use tokio_util::sync::CancellationToken;
/// Synchronous state machine for the auger driver.
/// This is the main state machine.
/// State enforced through typestates.
#[derive(Getters)]
pub struct TypedAgent<S: State> {
    pub(crate) model: LlmModel,
    /// The messages in the agent's current session so far.
    /// Note that this crate guarantees the messages in here are aligned with
    /// the state. It is a bug if that is untrue.
    #[get = "pub"]
    pub(crate) messages: Vec<Message>,
    pub(crate) tools: Vec<ToolDefinition>,
    #[get = "pub"]
    pub(crate) state: S,
}

/// A state that the driver can be in.
pub trait State {}

/// The driver is waiting for a user message.
/// Providing a message will begin the LLM stream and
/// transition it to the [`LlmStreaming`] state.
pub struct WaitingForUserMessage;
impl State for WaitingForUserMessage {}

impl TypedAgent<WaitingForUserMessage> {
    /// Create a new agent with the given system prompt and model.
    pub fn new(model: LlmModel, system_prompt: String, tools: Vec<ToolDefinition>) -> Self {
        let mut messages = Vec::new();
        messages.push(Message::System(system_prompt));
        let state = WaitingForUserMessage {};
        Self {
            messages,
            model,
            tools,
            state,
        }
    }

    /// Get the previous assistant message that occurred before this state.
    /// May be `None` if this is the first turn in the session.
    pub fn previous_message(&self) -> Option<&AssistantResponse> {
        let assistant_message = self.messages().last()?;
        match assistant_message {
            Message::Assistant { response } => Some(response),
            _ => panic!(
                "auger driver state invariant violation: last message should be an assistant \
                 message when in WaitingForUserMessage state"
            ),
        }
    }

    /// Add a user message to the driver and transition it to the
    /// [`ReadyToStream`] state.
    pub fn add_message(mut self, msg: UserPrompt) -> TypedAgent<ReadyToStream> {
        self.messages.push(msg.into());
        let state = ReadyToStream {};
        TypedAgent {
            model: self.model,
            tools: self.tools,
            state,
            messages: self.messages,
        }
    }
}

/// The driver is ready to begin streaming the LLM response.
pub struct ReadyToStream {}

impl State for ReadyToStream {}

impl TypedAgent<ReadyToStream> {
    /// Creates an interruptible LLM stream future.
    pub fn create_stream(
        self,
        cb: impl Fn(provider::StreamEvent) + Send + Sync + 'static,
    ) -> LlmStreamingFuture {
        let cancellation = CancellationToken::new();

        LlmStreamingFuture::new(
            self.model,
            self.tools,
            self.messages,
            Box::new(cb),
            cancellation,
        )
    }
}
