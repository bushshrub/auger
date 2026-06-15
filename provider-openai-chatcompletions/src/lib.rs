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
use provider::{
    ChatRequest, ChatResponse, FinishReason as ProviderFinishReason, Provider, ProviderError,
    StreamEvent, ToolCall, ToolDefinition, Usage,
};

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

// ── Request adaptation ───────────────────────────────────────────────────────

fn map_message(msg: &provider::Message) -> ChatCompletionRequestMessage {
    match msg.role {
        provider::Role::System => {
            ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                content: ChatCompletionRequestSystemMessageContent::Text(msg.content.clone()),
                name: None,
            })
        }
        provider::Role::User => {
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content: ChatCompletionRequestUserMessageContent::Text(msg.content.clone()),
                name: None,
            })
        }
        provider::Role::Assistant => {
            let tool_calls = msg.tool_calls.as_ref().map(|tc| {
                tc.iter()
                    .map(|tc| {
                        async_openai::types::chat::ChatCompletionMessageToolCalls::Function(
                            async_openai::types::chat::ChatCompletionMessageToolCall {
                                id: tc.id.clone(),
                                function: FunctionCall {
                                    name: tc.name.clone(),
                                    arguments: tc.arguments.clone(),
                                },
                            },
                        )
                    })
                    .collect()
            });
            ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
                content: if msg.content.is_empty() {
                    None
                } else {
                    Some(ChatCompletionRequestAssistantMessageContent::Text(
                        msg.content.clone(),
                    ))
                },
                refusal: None,
                name: None,
                audio: None,
                tool_calls,
                #[allow(deprecated)]
                function_call: None,
            })
        }
        provider::Role::Tool => {
            ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
                content: ChatCompletionRequestToolMessageContent::Text(msg.content.clone()),
                tool_call_id: msg.tool_call_id.clone().unwrap_or_default(),
            })
        }
    }
}

fn map_tool(def: &ToolDefinition) -> ChatCompletionTools {
    ChatCompletionTools::Function(async_openai::types::chat::ChatCompletionTool {
        function: FunctionObject {
            name: def.name.clone(),
            description: def.description.clone(),
            parameters: Some(def.parameters.clone()),
            strict: None,
        },
    })
}

fn to_oai_request(req: &ChatRequest) -> CreateChatCompletionRequest {
    CreateChatCompletionRequest {
        messages: req.messages.iter().map(map_message).collect(),
        model: req.model.clone(),
        modalities: None,
        verbosity: None,
        reasoning_effort: None,
        max_completion_tokens: req.max_tokens.map(|n| n as u32),
        frequency_penalty: None,
        presence_penalty: None,
        web_search_options: None,
        top_logprobs: None,
        response_format: None,
        audio: None,
        store: None,
        stream: None,
        stop: None,
        logit_bias: None,
        logprobs: None,
        #[allow(deprecated)]
        max_tokens: None,
        n: None,
        prediction: None,
        #[allow(deprecated)]
        seed: None,
        stream_options: None,
        service_tier: None,
        temperature: req.temperature.map(|t| t as f32),
        top_p: None,
        tools: req.tools.as_ref().map(|ts| ts.iter().map(map_tool).collect()),
        tool_choice: None,
        parallel_tool_calls: None,
        #[allow(deprecated)]
        user: None,
        safety_identifier: None,
        prompt_cache_key: None,
        #[allow(deprecated)]
        function_call: None,
        #[allow(deprecated)]
        functions: None,
        metadata: None,
    }
}

// ── Response adaptation ──────────────────────────────────────────────────────

fn map_finish_reason(fr: FinishReason) -> ProviderFinishReason {
    match fr {
        FinishReason::Stop => ProviderFinishReason::Stop,
        FinishReason::Length => ProviderFinishReason::Length,
        FinishReason::ToolCalls => ProviderFinishReason::ToolCalls,
        FinishReason::ContentFilter => ProviderFinishReason::Error,
        FinishReason::FunctionCall => ProviderFinishReason::ToolCalls,
    }
}

fn map_tool_call(
    tc: async_openai::types::chat::ChatCompletionMessageToolCalls,
) -> ToolCall {
    match tc {
        async_openai::types::chat::ChatCompletionMessageToolCalls::Function(fc) => ToolCall {
            id: fc.id,
            name: fc.function.name,
            arguments: fc.function.arguments,
        },
        async_openai::types::chat::ChatCompletionMessageToolCalls::Custom(ct) => ToolCall {
            id: ct.id,
            name: ct.custom_tool.name,
            arguments: ct.custom_tool.input,
        },
    }
}

fn to_provider_response(
    resp: async_openai::types::chat::CreateChatCompletionResponse,
) -> Result<ChatResponse, ProviderError> {
    let choice = resp.choices.into_iter().next().ok_or_else(|| {
        ProviderError::InvalidResponse("no choices in response".into())
    })?;

    let content = choice
        .message
        .refusal
        .clone()
        .unwrap_or_else(|| choice.message.content.unwrap_or_default());

    let tool_calls = choice
        .message
        .tool_calls
        .map(|tc| tc.into_iter().map(map_tool_call).collect());

    let finish_reason = choice
        .finish_reason
        .map(map_finish_reason)
        .unwrap_or(ProviderFinishReason::Stop);

    let usage = resp.usage.map(|u| Usage {
        prompt_tokens: u.prompt_tokens as usize,
        completion_tokens: u.completion_tokens as usize,
        total_tokens: u.total_tokens as usize,
    });

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
        usage,
    })
}

// ── Provider trait ───────────────────────────────────────────────────────────

#[async_trait]
impl Provider for OpenAiChatCompletionsProvider {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let oai_req = to_oai_request(req);
        let resp = self.client.chat().create(oai_req).await.map_err(|e| match e {
            async_openai::error::OpenAIError::ApiError(api) => ProviderError::Api {
                status: api.status_code.as_u16(),
                body: api.api_error.message,
            },
            other => ProviderError::Transport(other.to_string()),
        })?;
        to_provider_response(resp)
    }

    fn stream_chat(
        &self,
        req: &ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
        let oai_req = to_oai_request(req);
        let client = self.client.clone();

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamEvent, ProviderError>>(64);

        tokio::spawn(async move {
            let mut stream = match client.chat().create_stream(oai_req).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.send(Err(ProviderError::Transport(e.to_string()))).await;
                    return;
                }
            };

            let mut full_content = String::new();
            // index → (id, name, accumulated_args)
            let mut tc_acc: HashMap<u32, (String, String, String)> = HashMap::new();
            let mut finish_reason = ProviderFinishReason::Stop;

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(resp) => {
                        for choice in resp.choices {
                            if let Some(text) = choice.delta.content {
                                full_content.push_str(&text);
                                if tx.send(Ok(StreamEvent::Content(text))).await.is_err() {
                                    return;
                                }
                            }
                            if let Some(chunks) = choice.delta.tool_calls {
                                for tc in chunks {
                                    let entry = tc_acc.entry(tc.index).or_default();
                                    if let Some(id) = tc.id {
                                        entry.0 = id;
                                    }
                                    if let Some(f) = tc.function {
                                        if let Some(name) = f.name {
                                            entry.1 = name;
                                        }
                                        if let Some(args) = f.arguments {
                                            entry.2.push_str(&args);
                                        }
                                    }
                                }
                            }
                            if let Some(fr) = choice.finish_reason {
                                finish_reason = map_finish_reason(fr);
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(ProviderError::Transport(e.to_string()))).await;
                        return;
                    }
                }
            }

            let mut sorted: Vec<_> = tc_acc.into_iter().collect();
            sorted.sort_by_key(|(idx, _)| *idx);
            let tool_calls: Vec<ToolCall> = sorted
                .into_iter()
                .map(|(_, (id, name, arguments))| ToolCall { id, name, arguments })
                .collect();

            let response = ChatResponse {
                content: full_content,
                tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
                finish_reason,
                usage: None,
            };

            let _ = tx.send(Ok(StreamEvent::Done(response))).await;
        });

        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }
}
