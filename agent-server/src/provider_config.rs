use std::sync::Arc;

use provider::LlmProvider;
use provider_anthropic::AnthropicProvider;
use provider_openai_chatcompletions::OpenAiChatCompletionsProvider;
use provider_openai_responses::OpenAiResponsesProvider;
use tracing::{error, info};

use crate::config::Config;

const DEFAULT_OPENAI_BASE_URL: &str = "http://server-slop:8080/v1/";
const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderType {
    Anthropic,
    OpenAiChatCompletions,
    OpenAiResponses,
    DeepSeek,
}

impl ProviderType {
    fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "openai-chat-completions" | "openai_chat_completions" => {
                Ok(Self::OpenAiChatCompletions)
            }
            "openai-responses" | "openai_responses" => Ok(Self::OpenAiResponses),
            "deepseek" => Ok(Self::DeepSeek),
            _ => Err(format!(
                "unsupported PROVIDER_TYPE '{value}'; expected anthropic, openai-chat-completions, or openai-responses"
            )),
        }
    }
}

fn default_base_url(provider_type: ProviderType) -> String {
    match provider_type {
        ProviderType::Anthropic => String::new(),
        ProviderType::OpenAiChatCompletions | ProviderType::OpenAiResponses => {
            DEFAULT_OPENAI_BASE_URL.to_string()
        }
        ProviderType::DeepSeek => DEFAULT_DEEPSEEK_BASE_URL.to_string(),
    }
}

pub(crate) fn from_config(config: &Config) -> Arc<dyn LlmProvider> {
    let provider_type = config.provider_type();
    let provider_type = ProviderType::parse(&provider_type).unwrap_or_else(|error| {
        error!(
            configured_provider_type = %provider_type,
            error = %error,
            "invalid provider API type"
        );
        panic!("{error}");
    });
    let api_key = config.provider_api_key();
    let user_agent = config.user_agent();

    match provider_type {
        ProviderType::Anthropic => {
            let base_url = config.provider_base_url().unwrap_or_default();
            info!(provider_type = ?provider_type, base_url = %base_url, "configured LLM provider");
            Arc::new(AnthropicProvider::with_user_agent(
                api_key,
                base_url,
                user_agent.clone(),
            ))
        }
        // deepseek uses openai chat completion
        ProviderType::OpenAiChatCompletions => {
            let base_url = config
                .provider_base_url()
                .unwrap_or_else(|| default_base_url(provider_type));

            Arc::new(OpenAiChatCompletionsProvider::with_user_agent(
                api_key,
                base_url,
                user_agent.clone(),
            ))
        }

        ProviderType::DeepSeek => {
            let base_url = config
                .provider_base_url()
                .unwrap_or_else(|| default_base_url(provider_type));

            Arc::new(OpenAiChatCompletionsProvider::with_user_agent(
                api_key,
                base_url,
                user_agent.clone(),
            ))
        }

        ProviderType::OpenAiResponses => {
            let base_url = config
                .provider_base_url()
                .unwrap_or_else(|| default_base_url(provider_type));
            info!(provider_type = ?provider_type, base_url = %base_url, "configured LLM provider");
            Arc::new(OpenAiResponsesProvider::with_user_agent(
                api_key, base_url, user_agent,
            ))
        }
    }
}

#[cfg(test)]
pub(crate) fn from_env_for_test() -> Arc<dyn LlmProvider> {
    Arc::new(OpenAiResponsesProvider::new(
        "test",
        "http://127.0.0.1:1/v1/",
    ))
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_DEEPSEEK_BASE_URL, ProviderType, default_base_url};

    #[test]
    fn parses_supported_provider_types() {
        assert_eq!(
            ProviderType::parse("anthropic").unwrap(),
            ProviderType::Anthropic
        );
        assert_eq!(
            ProviderType::parse("openai-chat-completions").unwrap(),
            ProviderType::OpenAiChatCompletions
        );
        assert_eq!(
            ProviderType::parse("openai_responses").unwrap(),
            ProviderType::OpenAiResponses
        );
        assert_eq!(
            ProviderType::parse("deepseek").unwrap(),
            ProviderType::DeepSeek
        );
    }

    #[test]
    fn deepseek_uses_deepseek_base_url_by_default() {
        assert_eq!(
            default_base_url(ProviderType::DeepSeek),
            DEFAULT_DEEPSEEK_BASE_URL
        );
    }
}
