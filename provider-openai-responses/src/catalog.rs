use crate::OpenAiResponsesProvider;
use provider::{LlmError, ModelCatalog, ModelId, ModelInfo};
use serde_json::Value;

impl OpenAiResponsesProvider {
    async fn fetch_models(&self) -> Result<Vec<Value>, LlmError> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.send().await.map_err(|e| LlmError {
            message: e.to_string(),
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError {
                message: format!("HTTP {}: {}", status, text),
            });
        }

        let data: Value = resp.json().await.map_err(|e| LlmError {
            message: format!("parse error: {}", e),
        })?;

        Ok(data["data"].as_array().cloned().unwrap_or_default())
    }
}

#[async_trait::async_trait]
impl ModelCatalog for OpenAiResponsesProvider {
    async fn list_models(&self) -> Result<Vec<ModelId>, LlmError> {
        Ok(self
            .fetch_models()
            .await?
            .iter()
            .filter_map(|m| m["id"].as_str())
            .map(ModelId::new)
            .collect())
    }

    async fn model_info(&self, model: &str) -> Result<ModelInfo, LlmError> {
        // Some OpenAI-compatible servers (llama.cpp) don't implement
        // GET /models/{id}, so resolve against the list instead.
        let models = self.fetch_models().await?;
        let entry = models
            .iter()
            .find(|m| m["id"].as_str() == Some(model))
            .ok_or_else(|| LlmError {
                message: format!("model '{model}' not found in provider catalog"),
            })?;

        let mut info = ModelInfo::new(ModelId::new(model));
        // llama.cpp reports the model's training context length in `meta`;
        // OpenAI reports no capability metadata here.
        info.context_window = entry["meta"]["n_ctx_train"].as_u64().map(|n| n as u32);
        Ok(info)
    }
}
