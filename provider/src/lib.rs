pub mod capabilities;
pub mod model;
pub mod thread;
pub mod types;

pub use capabilities::{ModelCatalog, ModelId, ModelInfo};
pub use model::LlmModel;
pub use thread::{AnyThread, LlmThread, LlmThreadState};
pub use types::*;

/// A connection to a language model API: endpoint, credentials, wire format.
///
/// Consumers shouldn't use the trait methods directly - rather they should
/// utilize [`LlmModel`].
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Get a non-streaming response from the clanker
    async fn complete(&self, model: &str, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    /// Let the clanker yap over an abortable stream.
    async fn stream(&self, model: &str, request: LlmRequest) -> Result<LlmStream, LlmError>;
}
