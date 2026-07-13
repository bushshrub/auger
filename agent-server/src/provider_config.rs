use std::sync::Arc;

use provider::LlmProvider;
use provider_anthropic::AnthropicProvider;
use provider_openai_chatcompletions::OpenAiChatCompletionsProvider;
use provider_openai_responses::OpenAiResponsesProvider;

use crate::config::Config;

const DEFAULT_OPENAI_BASE_URL: &str = "http://server-slop:8080/v1/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderType {
    Anthropic,
    OpenAiChatCompletions,
    OpenAiResponses,
}

impl ProviderType {
    fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "openai-chat-completions" | "openai_chat_completions" => Ok(Self::OpenAiChatCompletions),
            "openai-responses" | "openai_responses" => Ok(Self::OpenAiResponses),
            _ => Err(format!("unsupported PROVIDER_TYPE '{value}'; expected anthropic, openai-chat-completions, or openai-responses")),
        }
    }
}

pub(crate) fn from_config(config: &Config) -> Arc<dyn LlmProvider> {
    let provider_type = config.provider_type();
    let provider_type = ProviderType::parse(&provider_type).unwrap_or_else(|error| panic!("{error}"));
    let api_key = config.provider_api_key();

    match provider_type {
        ProviderType::Anthropic => {
            let base_url = config.provider_base_url().unwrap_or_default();
            Arc::new(AnthropicProvider::new(api_key, base_url))
        }
        ProviderType::OpenAiChatCompletions => {
            let base_url = config.provider_base_url().unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.to_string());
            Arc::new(OpenAiChatCompletionsProvider::new(api_key, base_url))
        }
        ProviderType::OpenAiResponses => {
            let base_url = config.provider_base_url().unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.to_string());
            Arc::new(OpenAiResponsesProvider::new(api_key, base_url))
        }
    }
}

#[cfg(test)]
pub(crate) fn from_env_for_test() -> Arc<dyn LlmProvider> {
    Arc::new(OpenAiResponsesProvider::new("test", "http://127.0.0.1:1/v1/"))
}

#[cfg(test)]
mod tests {
    use super::ProviderType;

    #[test]
    fn parses_supported_provider_types() {
        assert_eq!(ProviderType::parse("anthropic").unwrap(), ProviderType::Anthropic);
        assert_eq!(ProviderType::parse("openai-chat-completions").unwrap(), ProviderType::OpenAiChatCompletions);
        assert_eq!(ProviderType::parse("openai_responses").unwrap(), ProviderType::OpenAiResponses);
    }
}
