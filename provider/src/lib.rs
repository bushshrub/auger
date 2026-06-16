pub mod types;


use serde::Serialize;


pub use types::*;

#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// 
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError>;
}