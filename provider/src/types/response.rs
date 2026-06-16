use std::pin::Pin;
use futures_core::Stream;
use crate::tool::ToolCall;

#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
    pub cached_tokens: Option<i32>,
    pub cache_creation_tokens: Option<i32>,
}

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
    Done {
        usage: Option<TokenUsage>,
        stop_reason: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub usage: Option<TokenUsage>,
    pub stop_reason: Option<String>,
}

pub struct LlmError {}
pub type LlmStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, LlmError>> + Send>>;