use std::pin::Pin;
use futures_core::Stream;
use thiserror::Error;
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

    TextDelta(String),
    /// Thinking output from clanker
    ReasoningDelta(String),
    /// Tool call delta from clanker.
    ToolCall {
        id: String,
        name: String,
        /// Incomplete arguments
        arguments: String,
    },
    ToolCallComplete {
        id: String,
        name: String,
        /// Complete arguments
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
    pub content: String,
    /// Optional reasoning output, if the model supports reasoning.
    pub reasoning: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Token usage details after this response is complete.
    /// May be None if the provider doesn't expose token usage details
    pub usage: Option<TokenUsage>,
    pub stop_reason: Option<String>,
}

#[derive(Error, Debug, Clone)]
pub struct LlmError {
    pub message: String,
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Stream of events from the LLM. You can either get StreamEvents, or Errors.
pub type LlmStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, LlmError>> + Send>>;