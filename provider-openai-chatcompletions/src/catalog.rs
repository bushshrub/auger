use crate::OpenAiChatCompletionsProvider;
use provider::{LlmError, ModelCatalog, ModelId, ModelInfo};

#[async_trait::async_trait]
impl ModelCatalog for OpenAiChatCompletionsProvider {
    async fn list_models(&self) -> Result<Vec<ModelId>, LlmError> {
        let resp = self.client.models().list().await.map_err(|e| LlmError {
            message: e.to_string(),
        })?;
        Ok(resp.data.into_iter().map(|m| ModelId::new(m.id)).collect())
    }

    async fn model_info(&self, model: &str) -> Result<ModelInfo, LlmError> {
        // Some OpenAI-compatible servers (llama.cpp) don't implement
        // GET /models/{id}, so resolve against the list instead. The list
        // carries no capability metadata either way.
        let resp = self.client.models().list().await.map_err(|e| LlmError {
            message: e.to_string(),
        })?;

        if resp.data.iter().any(|m| m.id == model) {
            Ok(ModelInfo::new(ModelId::new(model)))
        } else {
            Err(LlmError {
                message: format!("model '{model}' not found in provider catalog"),
            })
        }
    }
}
