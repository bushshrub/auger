use provider::TokenUsage;

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

#[derive(Debug, Clone)]
pub enum ModelTurnOutcome {
    AssistantMessageComplete {
        usage: Option<Usage>,
        stop_reason: Option<String>,
    },
    NeedsToolResults,
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
