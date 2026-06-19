use std::pin::Pin;
use futures_core::Stream;
use crate::types::ToolCall;

/// Token usage details
#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
    pub cached_tokens: Option<i32>,
    pub cache_creation_tokens: Option<i32>,
}

/// Events that can be emitted while streaming a response from the clanker.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Text(String),
    /// Thinking output from clanker
    Reasoning(String),
    /// Tool call delta from clanker.
    ToolCall {
        id: String,
        name: String,
        /// Incomplete arguments
        arguments: String,
    },
    /// Clanker is done clanking.
    Done {
        usage: Option<TokenUsage>,
        stop_reason: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    // TODO: include thinking
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Token usage details after this response is complete.
    /// May be None if the provider doesn't expose token usage details
    pub usage: Option<TokenUsage>,
    pub stop_reason: Option<String>,
}

pub struct LlmError {}
/// Stream of events from the LLM. You can either get StreamEvents, or Errors.
pub type LlmStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, LlmError>> + Send>>;