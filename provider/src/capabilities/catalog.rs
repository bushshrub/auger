use crate::{LlmError, LlmModel, LlmProvider};
use std::sync::Arc;

/// The identifier of a model available from a provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelId(String);

impl ModelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Descriptive metadata about a model.
///
/// Fields a provider does not report are `None`. Note that
/// these are not necessarily the same as the model's actual capabilities,
/// which may be publicly known (e.g. openai models do not report this in their
/// API, but their capabilities can be looked up elsewhere.)
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: ModelId,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub supports_tools: Option<bool>,
    pub supports_reasoning: Option<bool>,
}

impl ModelInfo {
    /// Info with a known id and everything else unreported.
    pub fn new(id: ModelId) -> Self {
        Self {
            id,
            context_window: None,
            max_output_tokens: None,
            supports_tools: None,
            supports_reasoning: None,
        }
    }
}

/// Capability: enumerating and describing the models a provider serves.
#[async_trait::async_trait]
pub trait ModelCatalog: LlmProvider {
    /// List the models available from this provider.
    async fn list_models(&self) -> Result<Vec<ModelId>, LlmError>;

    /// Describe a single model.
    async fn model_info(&self, model: &str) -> Result<ModelInfo, LlmError>;
}

/// Checked model selection: verify `name` against the provider's catalog
/// before binding an [`LlmModel`] to it.
///
/// This should be used instead of `LlmModel::new` whenever the
/// provider exposes the `ModelCatalog` capability,
/// to avoid runtime errors from selecting a
/// model that doesn't exist.
pub async fn resolve_model<C>(catalog: &Arc<C>, name: &str) -> Result<LlmModel, LlmError>
where
    C: ModelCatalog + 'static,
{
    let models = catalog.list_models().await?;
    if models.iter().any(|m| m.as_str() == name) {
        Ok(LlmModel::new(catalog.clone(), name))
    } else {
        Err(LlmError {
            message: format!("model '{name}' not found in provider catalog"),
        })
    }
}
