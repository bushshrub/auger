use crate::{LlmError, LlmProvider, LlmRequest, LlmResponse, LlmStream};
use std::sync::Arc;

/// A model selected from an [`LlmProvider`]
#[derive(Clone)]
pub struct LlmModel {
    provider: Arc<dyn LlmProvider>,
    name: String,
}

impl LlmModel {
    pub fn new(provider: Arc<dyn LlmProvider>, name: impl Into<String>) -> Self {
        Self {
            provider,
            name: name.into(),
        }
    }

    /// The model name this handle is bound to.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get a non-streaming response from the clanker
    pub async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        self.provider.complete(&self.name, request).await
    }

    /// Let the clanker yap over a stream
    pub async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        self.provider.stream(&self.name, request).await
    }
}

