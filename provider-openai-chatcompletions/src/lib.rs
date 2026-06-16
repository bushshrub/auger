use std::collections::HashMap;

use async_openai::{
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestAssistantMessage, ChatCompletionRequestAssistantMessageContent,
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
        ChatCompletionRequestSystemMessageContent, ChatCompletionRequestToolMessage,
        ChatCompletionRequestToolMessageContent, ChatCompletionRequestUserMessage,
        ChatCompletionRequestUserMessageContent, ChatCompletionTools, CreateChatCompletionRequest,
        FinishReason, FunctionCall, FunctionObject,
    },
    Client,
};
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};

pub struct OpenAiChatCompletionsProvider {
    client: Client<OpenAIConfig>,
}

impl OpenAiChatCompletionsProvider {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }

    /// Point at a custom base URL (e.g. a local llama.cpp server).
    pub fn with_config(base_url: &str, api_key: &str) -> Self {
        let config = OpenAIConfig::new()
            .with_api_base(base_url)
            .with_api_key(api_key);
        Self { client: Client::with_config(config) }
    }
}

