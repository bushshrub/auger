pub mod types;

use async_trait::async_trait;
use futures::stream::BoxStream;
use thiserror::Error;

pub use types::*;

/// A provider which, when implemented, provides access to an LLM.
#[async_trait]
pub trait Provider {
    /// Sends a chat request and returns a complete response.
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError>;

    /// Sends a chat request and returns a stream of incremental events.
    fn stream_chat(
        &self,
        req: &ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError>;
}

/// Incremental events emitted during a streaming response.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A delta of assistant text content.
    Content(String),
    /// A complete tool call emitted during streaming.
    ToolCall(ToolCall),
    /// The final event containing the complete response.
    Done(ChatResponse),
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("Transport error: {0}")]
    Transport(String),

    #[error("API error: status={status}, body={body}")]
    Api { status: u16, body: String },

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}
