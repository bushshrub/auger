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
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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
    BlockEnd { index: usize },
    Usage(TokenUsage),
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

/// Fold stream events into blocks, in index order, plus any reported usage.
/// Truncated blocks just come out shorter; the caller decides what that means.
pub fn fold_events(events: &[StreamEvent]) -> (Vec<Block>, Option<TokenUsage>) {
    let mut open: BTreeMap<usize, (BlockKind, String)> = BTreeMap::new();
    let mut usage: Option<TokenUsage> = None;

    for event in events {
        match event {
            StreamEvent::BlockStart { index, kind } => {
                open.insert(*index, (kind.clone(), String::new()));
            }
            StreamEvent::BlockDelta { index, delta } => {
                if let Some((_, text)) = open.get_mut(index) {
                    text.push_str(delta);
                }
            }
            StreamEvent::BlockEnd { .. } => {}
            StreamEvent::Usage(reported) => {
                // Usage arrives in pieces, so merge per field rather than replacing.
                let acc = usage.get_or_insert_with(TokenUsage::default);
                acc.prompt_tokens = reported.prompt_tokens.or(acc.prompt_tokens);
                acc.completion_tokens = reported.completion_tokens.or(acc.completion_tokens);
                acc.total_tokens = reported.total_tokens.or(acc.total_tokens);
                acc.cached_tokens = reported.cached_tokens.or(acc.cached_tokens);
                acc.cache_creation_tokens =
                    reported.cache_creation_tokens.or(acc.cache_creation_tokens);
            }
        }
    }

    let blocks = open
        .into_values()
        .filter_map(|(kind, text)| match kind {
            BlockKind::Text => (!text.is_empty()).then(|| Block::Text(text)),
            BlockKind::Reasoning => (!text.is_empty()).then(|| Block::Reasoning { text }),
            BlockKind::ToolCall { id, name } => Some(Block::ToolCall(ToolCallRequest {
                id,
                name,
                arguments: text,
            })),
        })
        .collect();

    (blocks, usage)
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

