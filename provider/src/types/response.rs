use crate::Message;
use crate::types::ToolCallRequest;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::task::{Context, Poll};
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

pub struct ClankerMessage {
    reasoning: Option<String>,
    pub(crate) tool_calls: Vec<ToolCallRequest>,
    content: String,
}

impl From<CompletedLlmResponse> for ClankerMessage {
    fn from(response: CompletedLlmResponse) -> Self {
        Self {
            reasoning: response.reasoning,
            tool_calls: response.tool_calls.unwrap_or_default(),
            content: response.content,
        }
    }
}

impl From<PartialLlmResponse> for ClankerMessage {
    fn from(response: PartialLlmResponse) -> Self {
        Self {
            reasoning: response.reasoning,
            tool_calls: response.tool_calls.unwrap_or_default(),
            content: response.content,
        }
    }
}

impl From<ClankerMessage> for Message {
    fn from(msg: ClankerMessage) -> Self {
        Message::Assistant {
            reasoning: msg.reasoning,
            tool_calls: msg.tool_calls,
            content: msg.content,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PartialLlmResponse {
    pub content: String,
    /// Optional reasoning output, if the model supports reasoning.
    pub reasoning: Option<String>,
    /// Tool calls requested by the model.
    /// These are not guaranteed to be complete or valid.
    pub tool_calls: Option<Vec<ToolCallRequest>>,
    /// Token usage details after this response is complete.
    /// May be None if the provider doesn't expose token usage details
    pub usage: Option<TokenUsage>,
    /// The reason why the model stopped generating output.
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompletedLlmResponse {
    pub content: String,
    /// Optional reasoning output, if the model supports reasoning.
    pub reasoning: Option<String>,
    /// Tool calls requested by the model.
    pub tool_calls: Option<Vec<ToolCallRequest>>,
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
        let mut content = String::new();
        let mut reasoning = None;
        let mut tool_calls = Vec::new();
        let mut usage = None;
        let mut stop_reason = None;
        let mut completed = false;

        for event in events {
            match event {
                StreamEvent::TextDelta(delta) => content.push_str(&delta),
                StreamEvent::ReasoningDelta(delta) => {
                    reasoning.get_or_insert(String::new()).push_str(&delta)
                }
                // discard tool call deltas.
                StreamEvent::ToolCall { .. } => {}
                StreamEvent::ToolCallComplete {
                    id,
                    name,
                    arguments,
                } => tool_calls.push(ToolCallRequest {
                    id,
                    name,
                    arguments,
                }),
                StreamEvent::Done {
                    usage: u,
                    stop_reason: sr,
                } => {
                    usage = u;
                    stop_reason = sr;
                    completed = true;
                }
            }
        }

        let tool_calls = if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        };

        if completed {
            Self::Completed(CompletedLlmResponse {
                content,
                reasoning,
                tool_calls,
                usage,
                stop_reason,
            })
        } else {
            Self::Partial(PartialLlmResponse {
                content,
                reasoning,
                tool_calls,
                usage,
                stop_reason,
            })
        }
    }
}

impl From<LlmResponse> for ClankerMessage {
    fn from(response: LlmResponse) -> Self {
        match response {
            LlmResponse::Partial(response) => response.into(),
            LlmResponse::Completed(response) => response.into(),
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
pub struct LlmStream {
    inner: Option<Pin<Box<dyn Stream<Item = Result<StreamEvent, LlmError>> + Send>>>,
}

impl LlmStream {
    pub fn new(stream: impl Stream<Item = Result<StreamEvent, LlmError>> + Send + 'static) -> Self {
        Self {
            inner: Some(Box::pin(stream)),
        }
    }

    /// Abort the stream and release the provider's underlying stream.
    pub fn abort(&mut self) {
        self.inner = None;
    }
}

impl Stream for LlmStream {
    type Item = Result<StreamEvent, LlmError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut() {
            Some(stream) => stream.as_mut().poll_next(cx),
            None => Poll::Ready(None),
        }
    }
}

impl Unpin for LlmStream {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_without_done_is_partial() {
        let response = LlmResponse::from_events(vec![StreamEvent::TextDelta("partial".into())]);
        assert!(matches!(response, LlmResponse::Partial(_)));
    }

    #[test]
    fn response_with_done_is_completed() {
        let response = LlmResponse::from_events(vec![StreamEvent::Done {
            usage: None,
            stop_reason: None,
        }]);
        assert!(matches!(response, LlmResponse::Completed(_)));
    }
}
