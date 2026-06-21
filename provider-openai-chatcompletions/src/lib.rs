use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessage,
    ChatCompletionRequestUserMessageArgs, ChatCompletionTool, ChatCompletionTools,
    CompletionUsage, CreateChatCompletionRequestArgs, FinishReason, FunctionCall, FunctionObject,
};
use async_openai::Client;
use futures::StreamExt;
use provider::{
    LlmError, LlmProvider, LlmRequest, LlmResponse, LlmStream, StreamEvent, TokenUsage, ToolCall,
};

pub struct OpenAiChatCompletionsProvider {
    client: Client<OpenAIConfig>,
}

impl OpenAiChatCompletionsProvider {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let client = Client::with_config(
            OpenAIConfig::new()
                .with_api_key(api_key)
                .with_api_base(base_url),
        );
        Self { client }
    }
}

fn to_openai_messages(messages: Vec<provider::Message>) -> Vec<ChatCompletionRequestMessage> {
    messages
        .into_iter()
        .map(|m| match m {
            provider::Message::System(content) => ChatCompletionRequestMessage::System(
                ChatCompletionRequestSystemMessageArgs::default()
                    .content(content)
                    .build()
                    .unwrap(),
            ),
            provider::Message::User(content) => ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content(content)
                    .build()
                    .unwrap(),
            ),
            provider::Message::Assistant { content, tool_calls } => {
                let mut builder = ChatCompletionRequestAssistantMessageArgs::default();
                if !content.is_empty() {
                    builder.content(content);
                }
                if !tool_calls.is_empty() {
                    let oai_tool_calls: Vec<ChatCompletionMessageToolCalls> = tool_calls
                        .into_iter()
                        .map(|tc| {
                            ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                                id: tc.id,
                                function: FunctionCall {
                                    name: tc.name,
                                    arguments: tc.arguments,
                                },
                            })
                        })
                        .collect();
                    builder.tool_calls(oai_tool_calls);
                }
                ChatCompletionRequestMessage::Assistant(builder.build().unwrap())
            }
            provider::Message::Tool { tool_call_id, content } => {
                ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
                    content: content.into(),
                    tool_call_id,
                })
            }
        })
        .collect()
}

fn to_openai_tools(tools: Vec<provider::ToolDefinition>) -> Vec<ChatCompletionTools> {
    tools
        .into_iter()
        .map(|t| {
            ChatCompletionTools::Function(ChatCompletionTool {
                function: FunctionObject {
                    name: t.name,
                    description: t.description,
                    parameters: Some(t.parameters),
                    strict: None,
                },
            })
        })
        .collect()
}

fn map_usage(usage: CompletionUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: Some(usage.prompt_tokens as i32),
        completion_tokens: Some(usage.completion_tokens as i32),
        total_tokens: Some(usage.total_tokens as i32),
        cached_tokens: None,
        cache_creation_tokens: None,
    }
}

fn finish_reason_string(fr: &FinishReason) -> String {
    match fr {
        FinishReason::Stop => "stop".into(),
        FinishReason::Length => "length".into(),
        FinishReason::ToolCalls => "tool_calls".into(),
        FinishReason::ContentFilter => "content_filter".into(),
        FinishReason::FunctionCall => "function_call".into(),
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiChatCompletionsProvider {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let req = CreateChatCompletionRequestArgs::default()
            .model(request.model)
            .messages(to_openai_messages(request.messages))
            .tools(to_openai_tools(request.tools))
            .build()
            .map_err(|e| LlmError { message: e.to_string() })?;

        let response = self
            .client
            .chat()
            .create(req)
            .await
            .map_err(|e| LlmError { message: e.to_string() })?;

        let choice = response.choices.first().ok_or_else(|| LlmError {
            message: "No choices in response".into(),
        })?;

        let tool_calls = choice.message.tool_calls.as_ref().and_then(|tcs| {
            let mapped: Vec<ToolCall> = tcs
                .iter()
                .filter_map(|tc| match tc {
                    ChatCompletionMessageToolCalls::Function(fc) => Some(ToolCall {
                        id: fc.id.clone(),
                        name: fc.function.name.clone(),
                        arguments: fc.function.arguments.clone(),
                    }),
                    _ => None,
                })
                .collect();
            if mapped.is_empty() { None } else { Some(mapped) }
        });

        Ok(LlmResponse {
            content: choice.message.content.clone().unwrap_or_default(),
            reasoning: None,
            tool_calls,
            usage: response.usage.map(map_usage),
            stop_reason: choice.finish_reason.as_ref().map(finish_reason_string),
        })
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let req = CreateChatCompletionRequestArgs::default()
            .model(request.model)
            .messages(to_openai_messages(request.messages))
            .tools(to_openai_tools(request.tools))
            .build()
            .map_err(|e| LlmError { message: e.to_string() })?;

        let sse_stream = self
            .client
            .chat()
            .create_stream(req)
            .await
            .map_err(|e| LlmError { message: e.to_string() })?;

        struct Accumulator {
            id: String,
            name: String,
            arguments: String,
        }

        let mut accums: Vec<Option<Accumulator>> = Vec::new();

        let stream = async_stream::stream! {
            let mut stream = sse_stream;
            while let Some(result) = stream.next().await {
                match result {
                    Ok(response) => {
                        for choice in &response.choices {
                            if let Some(ref content) = choice.delta.content {
                                if !content.is_empty() {
                                    yield Ok(StreamEvent::Text(content.clone()));
                                }
                            }

                            if let Some(ref tool_calls) = choice.delta.tool_calls {
                                for tc in tool_calls {
                                    let idx = tc.index as usize;
                                    while accums.len() <= idx {
                                        accums.push(None);
                                    }
                                    let acc = accums[idx].get_or_insert_with(|| Accumulator {
                                        id: String::new(),
                                        name: String::new(),
                                        arguments: String::new(),
                                    });

                                    if let Some(ref id) = tc.id {
                                        acc.id.clone_from(id);
                                    }
                                    if let Some(ref func) = tc.function {
                                        if let Some(ref name) = func.name {
                                            acc.name.clone_from(name);
                                        }
                                        if let Some(ref args) = func.arguments {
                                            acc.arguments.clone_from(args);
                                        }
                                    }

                                    yield Ok(StreamEvent::ToolCall {
                                        id: acc.id.clone(),
                                        name: acc.name.clone(),
                                        arguments: acc.arguments.clone(),
                                    });
                                }
                            }

                            if choice.finish_reason.is_some() {
                                yield Ok(StreamEvent::Done {
                                    usage: response.usage.clone().map(map_usage),
                                    stop_reason: choice.finish_reason.as_ref().map(finish_reason_string),
                                });
                            }
                        }
                    }
                    Err(e) => yield Err(LlmError { message: e.to_string() }),
                }
            }
        }
        .boxed();

        Ok(stream)
    }
}
