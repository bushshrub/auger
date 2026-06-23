use std::pin::Pin;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use crate::types::ToolCall;

/// Token usage details
#[derive(Debug, Clone, Deserialize, Serialize)]
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

impl From<Vec<StreamEvent>> for LlmResponse {
    fn from(events: Vec<StreamEvent>) -> Self {
        let mut content = String::new();
        let mut reasoning = None;
        let mut tool_calls = Vec::new();
        let mut usage = None;
        let mut stop_reason = None;

        for event in events {
            match event {
                StreamEvent::TextDelta(delta) => content.push_str(&delta),
                StreamEvent::ReasoningDelta(delta) => {
                    reasoning.get_or_insert(String::new()).push_str(&delta)
                }
                // discard tool call deltas.
                StreamEvent::ToolCall { id, name, arguments } => {
                }
                StreamEvent::ToolCallComplete { id, name, arguments } => {
                    tool_calls.push(ToolCall { id, name, arguments })
                }
                StreamEvent::Done { usage: u, stop_reason: sr } => {
                    usage = u;
                    stop_reason = sr;
                }
            }
        }

        Self {
            content,
            reasoning,
            tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
            usage,
            stop_reason,
        }
    }
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