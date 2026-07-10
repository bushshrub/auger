use provider::{TokenUsage, ToolCallRequest as ProviderToolCallRequest};

/// Partial assistant output from a model turn that did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialModelResponse {
    content: String,
    reasoning: Option<String>,
}

impl PartialModelResponse {
    pub(crate) fn new(content: String, reasoning: Option<String>) -> Self {
        Self { content, reasoning }
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn reasoning(&self) -> Option<&str> {
        self.reasoning.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty() && self.reasoning.as_deref().unwrap_or_default().is_empty()
    }
}

/// A complete tool call request emitted by the minimal loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRequest {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallRequest {
    pub(crate) fn from_provider(request: ProviderToolCallRequest) -> Self {
        Self {
            id: request.id,
            name: request.name,
            arguments: request.arguments,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn arguments(&self) -> &str {
        &self.arguments
    }
}

/// Events that can be emitted by the minimal loop during
/// a session.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Emitted anytime there is a state change
    StateChanged(SessionStatus),
    /// Delta of response from clanker
    LlmDelta(LlmDelta),
    /// Emitted when the LLM has finished streaming
    ModelTurnDone(ModelTurnOutcome),
    /// Emitted when the LLM streamed partial output but failed before completion.
    ModelTurnInterrupted {
        partial_response: PartialModelResponse,
        error: SessionError,
    },
    /// Indicates that the session was interrupted by the user
    Interrupted,
    /// Indicates the session encountered some kind of error
    Error(SessionError),
    Shutdown,
}

/// LLM Delta emitted during streaming.
#[derive(Debug, Clone)]
pub enum LlmDelta {
    AssistantContent(String),
    AssistantReasoning(String),
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
}

/// Event data emitted when model has finished streaming
#[derive(Debug, Clone)]
pub enum ModelTurnOutcome {
    /// The model has finished its message
    AssistantMessageComplete {
        usage: Option<Usage>,
        stop_reason: Option<String>,
    },
    /// The model has finished the message and wants tool calls
    NeedsToolResults {
        // TODO: technically we can expose usage here as well? idk.
        tool_calls: Vec<ToolCallRequest>,
    },
}

/// Token accounting returned by the provider when it is available.
pub type Usage = TokenUsage;

/// Coarse state exposed to session hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Session is idle and awaiting user response
    Idle,
    /// Session is streaming model information
    LlmTurnRunning,
    /// Session is waiting for tool calls to come back
    AwaitingHostFeedback,
    /// Session was interrupted and is waiting for a user message to continue
    AwaitingInterruptedUserMessage,
    /// Session saw a model response error and is waiting for a user message to continue
    ResponseError,
}

/// Errors emitted by the minimal loop.
#[derive(Debug, Clone)]
pub enum SessionError {
    /// Error while opening the stream from the model or mid-stream error.
    Model(String),
    /// The host provided an invalid tool result.
    InvalidToolResult(String),
    /// Some internal error
    // TODO: not descriptive - should figure out WHEN this occurs.
    Internal(String),
}
