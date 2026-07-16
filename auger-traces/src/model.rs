use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ProviderType {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModelInfo {
    provider_type: ProviderType,
}

impl ModelInfo {
    pub fn new(provider_type: ProviderType) -> Self {
        Self { provider_type }
    }
}
