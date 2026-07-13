use async_openai::Client;
use async_openai::config::OpenAIConfig;
use futures::StreamExt;
use provider::{
    LlmError, LlmProvider, LlmRequest, LlmResponse, LlmStream, StreamEvent, TokenUsage,
    ToolCallRequest,
};
use serde_json::{Value, json};

mod catalog;

pub struct OpenAiChatCompletionsProvider {
    client: Client<OpenAIConfig>,
}

fn normalize_base_url(base_url: impl Into<String>) -> String {
    let base_url = base_url.into();
    let base_url = base_url.trim_end_matches('/');
    base_url
        .strip_suffix("/chat/completions")
        .unwrap_or(base_url)
        .to_string()
}

impl OpenAiChatCompletionsProvider {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let client = Client::with_config(
            OpenAIConfig::new()
                .with_api_key(api_key)
                .with_api_base(normalize_base_url(base_url)),
        );
        Self { client }
    }
}

fn messages_to_json(messages: &[provider::Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m {
            provider::Message::System(content) => {
                out.push(json!({"role": "system", "content": content}));
            }
            provider::Message::User {
                message,
                tool_call_results,
            } => {
                for tr in tool_call_results {
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": tr.id(),
                        "content": tr.content(),
                    }));
                }
                if !message.message().is_empty() || tool_call_results.is_empty() {
                    out.push(json!({
                        "role": "user",
                        "content": message.message(),
                    }));
                }
            }
            provider::Message::Assistant {
                reasoning,
                content,
                tool_calls,
            } => {
                let mut msg = json!({"role": "assistant"});
                if !content.is_empty() {
                    msg["content"] = json!(content);
                }
                if let Some(rc) = reasoning {
                    msg["reasoning_content"] = json!(rc);
                }
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = json!(
                        tool_calls
                            .iter()
                            .map(|tc| json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {"name": tc.name, "arguments": tc.arguments}
                            }))
                            .collect::<Vec<_>>()
                    );
                }
                out.push(msg);
            }
        }
    }
    out
}

fn tools_to_json(tools: &[provider::ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect()
}

fn extract_usage(v: &Value) -> Option<TokenUsage> {
    let u = v.get("usage")?;
    Some(TokenUsage {
        prompt_tokens: u["prompt_tokens"].as_i64().map(|n| n as i32),
        completion_tokens: u["completion_tokens"].as_i64().map(|n| n as i32),
        total_tokens: u["total_tokens"].as_i64().map(|n| n as i32),
        cached_tokens: u["prompt_tokens_details"]["cached_tokens"]
            .as_i64()
            .map(|n| n as i32),
        cache_creation_tokens: None,
    })
}

fn extract_tool_calls(v: &Value) -> Option<Vec<ToolCallRequest>> {
    let tcs = v.as_array()?;
    let calls: Vec<ToolCallRequest> = tcs
        .iter()
        .filter_map(|tc| {
            Some(ToolCallRequest {
                id: tc["id"].as_str()?.to_string(),
                name: tc["function"]["name"].as_str()?.to_string(),
                arguments: tc["function"]["arguments"].as_str()?.to_string(),
            })
        })
        .collect();
    if calls.is_empty() { None } else { Some(calls) }
}

fn extract_reasoning(v: &Value) -> Option<String> {
    v["reasoning_content"]
        .as_str()
        .or_else(|| v["reasoning"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiChatCompletionsProvider {
    async fn complete(&self, model: &str, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let body = json!({
            "model": model,
            "messages": messages_to_json(request.messages()),
            "tools": tools_to_json(request.tools()),
        });

        let resp: Value = self
            .client
            .chat()
            .create_byot(body)
            .await
            .map_err(|e| LlmError {
                message: e.to_string(),
            })?;

        let msg = &resp["choices"][0]["message"];
        let tool_calls = extract_tool_calls(&msg["tool_calls"]);
        let finish_reason = resp["choices"][0]["finish_reason"]
            .as_str()
            .map(str::to_string);

        Ok(LlmResponse {
            content: msg["content"].as_str().unwrap_or("").to_string(),
            reasoning: extract_reasoning(msg),
            tool_calls,
            usage: extract_usage(&resp),
            stop_reason: finish_reason,
        })
    }

    async fn stream(&self, model: &str, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let body = json!({
            "model": model,
            "messages": messages_to_json(request.messages()),
            "tools": tools_to_json(request.tools()),
            "stream": true,
            "stream_options": {"include_usage": true},
        });

        let sse_stream = self
            .client
            .chat()
            .create_stream_byot::<Value, Value>(body)
            .await
            .map_err(|e| LlmError {
                message: e.to_string(),
            })?;

        struct TcAccum {
            id: String,
            name: String,
            arguments: String,
        }

        let mut accums: Vec<Option<TcAccum>> = Vec::new();

        let stream = async_stream::stream! {
            let mut stream = sse_stream;
            let mut stop_reason: Option<String> = None;
            let mut final_usage: Option<TokenUsage> = None;
            let mut tool_calls_completed = false;

            while let Some(result) = stream.next().await {
                match result {
                    Err(e) => {
                        yield Err(LlmError { message: e.to_string() });
                        return;
                    }
                    Ok(chunk) => {
                        // Usage may arrive in the finish_reason chunk or in a trailing
                        // chunk with empty choices (OpenAI stream_options include_usage).
                        if let Some(u) = extract_usage(&chunk) {
                            final_usage = Some(u);
                        }

                        let choice = &chunk["choices"][0];
                        let delta = &choice["delta"];

                        if let Some(reasoning) = extract_reasoning(delta) {
                            yield Ok(StreamEvent::ReasoningDelta(reasoning));
                        }

                        if let Some(content) = delta["content"].as_str() {
                            if !content.is_empty() {
                                yield Ok(StreamEvent::TextDelta(content.to_string()));
                            }
                        }

                        if let Some(tcs) = delta["tool_calls"].as_array() {
                            for tc in tcs {
                                let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                                while accums.len() <= idx {
                                    accums.push(None);
                                }
                                let acc = accums[idx].get_or_insert_with(|| TcAccum {
                                    id: String::new(),
                                    name: String::new(),
                                    arguments: String::new(),
                                });
                                if let Some(id) = tc["id"].as_str() {
                                    acc.id = id.to_string();
                                }
                                if let Some(name) = tc["function"]["name"].as_str() {
                                    acc.name = name.to_string();
                                }
                                let mut arg_delta = "";
                                if let Some(args) = tc["function"]["arguments"].as_str() {
                                    acc.arguments.push_str(args);
                                    arg_delta = args;
                                }
                                yield Ok(StreamEvent::ToolCall {
                                    id: acc.id.clone(),
                                    name: acc.name.clone(),
                                    arguments: arg_delta.to_string(),
                                });
                            }
                        }

                        if choice["finish_reason"].is_string() && !tool_calls_completed {
                            tool_calls_completed = true;
                            for acc in accums.iter().flatten() {
                                yield Ok(StreamEvent::ToolCallComplete {
                                    id: acc.id.clone(),
                                    name: acc.name.clone(),
                                    arguments: acc.arguments.clone(),
                                });
                            }
                            stop_reason = choice["finish_reason"].as_str().map(str::to_string);
                        }
                    }
                }
            }

            yield Ok(StreamEvent::Done { usage: final_usage, stop_reason });
        }
        .boxed();

        Ok(LlmStream::new(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_reasoning, messages_to_json, normalize_base_url};
    use provider::{Message, ToolResult, UserPrompt};
    use serde_json::json;

    #[test]
    fn normalizes_chat_completions_endpoint() {
        assert_eq!(
            normalize_base_url("https://opencode.ai/zen/v1/chat/completions"),
            "https://opencode.ai/zen/v1"
        );
        assert_eq!(
            normalize_base_url("https://opencode.ai/zen/v1/chat/completions/"),
            "https://opencode.ai/zen/v1"
        );
        assert_eq!(normalize_base_url("https://opencode.ai/zen/v1"), "https://opencode.ai/zen/v1");
    }

    #[test]
    fn accepts_openai_reasoning_field_names() {
        assert_eq!(extract_reasoning(&json!({"reasoning": "think"})), Some("think".to_string()));
        assert_eq!(
            extract_reasoning(&json!({"reasoning_content": "think"})),
            Some("think".to_string())
        );
    }

    #[test]
    fn serializes_tool_results_as_tool_messages() {
        let messages = messages_to_json(&[Message::User {
            message: UserPrompt::new("continue".to_string()),
            tool_call_results: vec![ToolResult::new("call_1".to_string(), "result".to_string())],
        }]);

        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["tool_call_id"], "call_1");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "continue");
    }
}
