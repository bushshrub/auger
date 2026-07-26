use async_openai::Client;
use async_openai::config::OpenAIConfig;
use futures::StreamExt;
use provider::Block;
use provider::BlockKind;
use provider::CompletedLlmResponse;
use provider::LlmError;
use provider::LlmProvider;
use provider::LlmRequest;
use provider::StreamEnd;
use provider::StreamEvent;
use provider::TokenUsage;
use provider::ToolCallRequest;
use reqwest::Client as HttpClient;
use serde_json::Value;
use serde_json::json;

const DEFAULT_USER_AGENT: &str = "auger-code/0.1.0";

mod catalog;
mod errors;

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
        Self::with_user_agent(api_key, base_url, "")
    }

    pub fn with_user_agent(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        user_agent: impl AsRef<str>,
    ) -> Self {
        let client = Client::with_config(
            OpenAIConfig::new()
                .with_api_key(api_key)
                .with_api_base(normalize_base_url(base_url)),
        )
        .with_http_client(
            HttpClient::builder()
                .user_agent(format_user_agent(user_agent.as_ref()))
                .build()
                .expect("auger user agent must be valid HTTP header text"),
        );
        Self { client }
    }
}

fn format_user_agent(user_agent: &str) -> String {
    if user_agent.is_empty() {
        DEFAULT_USER_AGENT.to_string()
    } else {
        user_agent.to_string()
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
                        "tool_call_id": tr.tool_call_id,
                        "content": tr.content,
                    }));
                }
                if !message.message.is_empty() || tool_call_results.is_empty() {
                    out.push(json!({
                        "role": "user",
                        "content": message.message,
                    }));
                }
            }
            provider::Message::Assistant { response } => {
                let blocks = response.blocks();
                let mut msg = json!({"role": "assistant"});
                let mut has_content = false;
                let mut tool_call_arr: Vec<Value> = Vec::new();
                for block in blocks {
                    match block {
                        Block::Text(text) => {
                            if !text.is_empty() {
                                msg["content"] = json!(text);
                                has_content = true;
                            }
                        }
                        Block::Reasoning { text } => {
                            if !text.is_empty() {
                                msg["reasoning_content"] = json!(text);
                                has_content = true;
                            }
                        }
                        Block::ToolCall(tc) => {
                            if !has_content {
                                msg["content"] = json!("");
                                has_content = true;
                            }
                            tool_call_arr.push(json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {"name": tc.name, "arguments": tc.arguments}
                            }));
                        }
                    }
                }
                if !tool_call_arr.is_empty() {
                    msg["tool_calls"] = json!(tool_call_arr);
                }
                if !has_content {
                    msg["content"] = json!("");
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
    async fn complete(
        &self,
        model: &str,
        request: LlmRequest,
    ) -> Result<CompletedLlmResponse, LlmError> {
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
            .map_err(errors::from_error)?;

        let msg = &resp["choices"][0]["message"];
        let tool_calls = extract_tool_calls(&msg["tool_calls"]);
        let finish_reason = resp["choices"][0]["finish_reason"]
            .as_str()
            .map(str::to_string);

        let mut blocks: Vec<Block> = Vec::new();
        let content = msg["content"].as_str().unwrap_or("");
        if !content.is_empty() {
            blocks.push(Block::Text(content.to_string()));
        }
        if let Some(rc) = extract_reasoning(msg) {
            blocks.push(Block::Reasoning { text: rc });
        }
        if let Some(tcs) = tool_calls {
            for tc in tcs {
                blocks.push(Block::ToolCall(tc));
            }
        }

        Ok(CompletedLlmResponse {
            response: provider::AssistantResponse::new(blocks).unwrap(),
            usage: extract_usage(&resp),
            stop_reason: finish_reason,
        })
    }

    async fn stream(
        &self,
        model: &str,
        request: LlmRequest,
        sink: provider::EventSink<'_>,
    ) -> Result<StreamEnd, LlmError> {
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
            .map_err(errors::from_error)?;

        struct TcAccum {
            id: String,
            name: String,
            arguments: String,
        }

        let mut accums: Vec<Option<TcAccum>> = Vec::new();
        let mut next_block_idx: usize = 0;
        let mut stop_reason: Option<String> = None;
        let mut final_usage: Option<TokenUsage> = None;
        let mut tool_calls_completed = false;

        let mut stream = sse_stream;
        while let Some(result) = stream.next().await {
            match result {
                Err(e) => return Err(errors::from_error(e)),
                Ok(chunk) => {
                    if let Some(error) = chunk["error"].as_object() {
                        let message = error["message"].as_str().unwrap_or("stream error");
                        return Err(errors::in_band_fields(
                            message.to_string(),
                            error.get("type").and_then(|v| v.as_str()),
                            error.get("code").and_then(|v| v.as_str()),
                        ));
                    }
                    if let Some(u) = extract_usage(&chunk) {
                        final_usage = Some(u);
                    }

                    let choice = &chunk["choices"][0];
                    let delta = &choice["delta"];

                    if let Some(reasoning) = extract_reasoning(delta) {
                        let idx = next_block_idx;
                        next_block_idx += 1;
                        sink(StreamEvent::BlockStart { index: idx, kind: BlockKind::Reasoning });
                        sink(StreamEvent::BlockDelta { index: idx, delta: reasoning });
                    }

                    if let Some(content) = delta["content"].as_str() {
                        if !content.is_empty() {
                            let idx = next_block_idx;
                            next_block_idx += 1;
                            sink(StreamEvent::BlockStart { index: idx, kind: BlockKind::Text });
                            sink(StreamEvent::BlockDelta { index: idx, delta: content.to_string() });
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
                            let tc_idx = next_block_idx;
                            next_block_idx += 1;
                            sink(StreamEvent::BlockStart {
                                index: tc_idx,
                                kind: BlockKind::ToolCall {
                                    id: acc.id.clone(),
                                    name: acc.name.clone(),
                                },
                            });
                            sink(StreamEvent::BlockDelta { index: tc_idx, delta: arg_delta.to_string() });
                        }
                    }

                    if choice["finish_reason"].as_str() == Some("error") {
                        return Ok(StreamEnd {
                            usage: None,
                            stop_reason: Some("stream finish_reason=error".to_string()),
                        });
                    }
                    if choice["finish_reason"].is_string() && !tool_calls_completed {
                        tool_calls_completed = true;
                        for _acc in accums.iter().flatten() {
                            sink(StreamEvent::BlockEnd { index: next_block_idx - 1 });
                        }
                        stop_reason = choice["finish_reason"].as_str().map(str::to_string);
                    }
                }
            }
        }

        Ok(StreamEnd {
            usage: final_usage,
            stop_reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::extract_reasoning;
    use super::messages_to_json;
    use super::normalize_base_url;
    use provider::Message;
    use provider::ToolResult;
    use provider::UserPrompt;
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
        assert_eq!(
            normalize_base_url("https://opencode.ai/zen/v1"),
            "https://opencode.ai/zen/v1"
        );
    }

    #[test]
    fn accepts_openai_reasoning_field_names() {
        assert_eq!(
            extract_reasoning(&json!({"reasoning": "think"})),
            Some("think".to_string())
        );
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
