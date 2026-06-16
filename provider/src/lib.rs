pub mod types;


pub use types::*;

#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Get a non-streaming response from the clanker
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    /// Let the clanker yap over a stream
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError>;
}