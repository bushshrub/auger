use crate::CompletedLlmResponse;
use crate::LlmError;
use crate::LlmProvider;
use crate::LlmRequest;
use crate::LlmStream;
use std::fmt;
use std::sync::Arc;

/// A model selected from an [`LlmProvider`]
#[derive(Clone)]
pub struct LlmModel {
    provider: Arc<dyn LlmProvider>,
    name: String,
}

impl fmt::Debug for LlmModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmModel")
            .field("name", &self.name)
            .finish()
    }
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
    pub async fn complete(&self, request: LlmRequest) -> Result<CompletedLlmResponse, LlmError> {
        self.provider.complete(&self.name, request).await
    }

    /// Let the clanker yap over a stream
    pub async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        self.provider.stream(&self.name, request).await
    }
}
