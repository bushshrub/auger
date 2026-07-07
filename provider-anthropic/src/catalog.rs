use crate::{API_VERSION, AnthropicProvider};
use provider::{LlmError, ModelCatalog, ModelId, ModelInfo};
use serde_json::Value;

impl AnthropicProvider {
    async fn get_json(&self, url: &str) -> Result<Value, LlmError> {
        let resp = self
            .client
            .get(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .send()
            .await
            .map_err(|e| LlmError {
                message: e.to_string(),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError {
                message: format!("HTTP {}: {}", status, text),
            });
        }

        resp.json().await.map_err(|e| LlmError {
            message: format!("parse error: {}", e),
        })
    }
}

#[async_trait::async_trait]
impl ModelCatalog for AnthropicProvider {
    async fn list_models(&self) -> Result<Vec<ModelId>, LlmError> {
        // Default page size is 20; 1000 is the API maximum and comfortably
        // covers the full model list in one request.
        let url = format!("{}?limit=1000", self.models_url);
        let data = self.get_json(&url).await?;
        Ok(data["data"]
            .as_array()
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m["id"].as_str())
                    .map(ModelId::new)
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn model_info(&self, model: &str) -> Result<ModelInfo, LlmError> {
        let url = format!("{}/{}", self.models_url, model);
        let data = self.get_json(&url).await?;
        let id = data["id"].as_str().unwrap_or(model);
        // The Anthropic models API reports only id/display_name/created_at,
        // no capability metadata.
        Ok(ModelInfo::new(ModelId::new(id)))
    }
}
