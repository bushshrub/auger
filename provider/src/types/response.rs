use std::collections::{BTreeMap, HashMap};
use crate::AssistantResponse;
use crate::Message;
use crate::types::ToolCallRequest;
use futures_core::Stream;
use getset::Getters;
use serde::Deserialize;
use serde::Serialize;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use thiserror::Error;

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
///
/// Some clankers are more advanced and can interleave reasoning and yap.
/// Example:
/// <some reasoning>
/// <text yap>
/// <more reasoning>
/// <text yap>
/// <call a tool>
/// <text yap>
/// <call another tool>
/// etc. etc.
///
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum StreamEvent {
    BlockStart { index: usize, kind: BlockKind },
    BlockDelta { index: usize, delta: String },
    BlockEnd { index: usize }
}

/// The type of block being emitted
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum BlockKind {
    Reasoning,
    Text,
    ToolCall { id: String, name: String },
}

/// The LLM stream has successfully returned
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEnd {
    pub usage: Option<TokenUsage>,
    pub stop_reason: Option<String>,
}

/// A block of response from the clanker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Block {
    Text (String),
    Reasoning {
        text: String,
    },
    ToolCall(ToolCallRequest)
}

/// A partial LLM response. Note that this may not have complete blocks since
/// the user could have cut it off, or it could have failed midway.
#[derive(Debug, Clone)]
pub struct PartialLlmResponse {
    /// The raw events from the clanker provider.
    pub raw_events: Vec<StreamEvent>,
    /// Token usage details after this response is complete.
    /// May be None if the provider doesn't expose token usage details
    pub usage: Option<TokenUsage>,
    /// The reason why the model stopped generating output.
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompletedLlmResponse {
    /// The final response from the clanker.
    pub response: AssistantResponse,
    /// Token usage details after this response is complete.
    /// May be None if the provider doesn't expose token usage details
    pub usage: Option<TokenUsage>,
    /// The reason why the model stopped generating output.
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub enum LlmResponse {
    Partial(PartialLlmResponse),
    Completed(CompletedLlmResponse),
}

impl LlmResponse {
    /// Collect stream events and select a partial or completed response.
    pub fn from_events(events: Vec<StreamEvent>) -> Self {
        todo!()
    }
}

/// The kind of LLM error received.
/// There are 2 kinds really, one in which we can just retry the request unchanged
/// and one in which we cannot.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LlmErrorKind {
    /// Resending the request unchanged may work
    Transient { retry_after: Option<Duration> },
    /// Resending the request unchanged will not work
    Fatal
}

#[derive(Error, Serialize, Deserialize, Debug, Clone)]
pub struct LlmError {
    /// What kind of error it is.
    pub kind: LlmErrorKind,
    /// The error message
    pub message: String,
    /// HTTP status code, if any
    pub status: Option<u16>,
    /// Request ID if any
    pub request_id: Option<String>
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

