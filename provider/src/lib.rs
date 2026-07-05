pub mod types;
pub mod thread;
pub mod model;

pub use types::*;
pub use thread::{LlmThread, LlmThreadState, AnyThread};
pub use model::LlmModel;

/// A connection to a language model API: endpoint, credentials, wire format.
///
/// Consumers shouldn't use the trait methods directly - rather they should
/// utilize [`LlmModel`].
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Get a non-streaming response from the clanker
    async fn complete(&self, model: &str, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    /// Let the clanker yap over a stream
    async fn stream(&self, model: &str, request: LlmRequest) -> Result<LlmStream, LlmError>;
}
