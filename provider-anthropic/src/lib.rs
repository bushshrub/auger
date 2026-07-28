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
use provider::types::Message;
use reqwest::Client;
use serde_json::Value;
use serde_json::json;

mod catalog;
mod errors;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8096;
const DEFAULT_USER_AGENT: &str = "auger-code/0.1.0";

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    messages_url: String,
    models_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self::with_user_agent(api_key, base_url, "")
    }

    pub fn with_user_agent(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        user_agent: impl AsRef<str>,
    ) -> Self {
        let base = base_url.into();
        let base = if base.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            base
        };
        let base = base.trim_end_matches('/');
        let user_agent = format_user_agent(user_agent.as_ref());
        Self {
            client: Client::builder()
                .user_agent(user_agent)
                .build()
                .expect("auger user agent must be valid HTTP header text"),
            api_key: api_key.into(),
            messages_url: format!("{base}/v1/messages"),
            models_url: format!("{base}/v1/models"),
        }
    }
}

fn format_user_agent(user_agent: &str) -> String {
    if user_agent.is_empty() {
        DEFAULT_USER_AGENT.to_string()
    } else {
        user_agent.to_string()
    }
}

fn convert_messages(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut system: Option<String> = None;
    let mut out: Vec<Value> = Vec::new();

    for msg in messages {
        match msg {
            Message::System(content) => {
                system = Some(content.clone());
            }
            Message::User {
                message,
                tool_call_results,
            } => {
                let mut blocks: Vec<Value> = Vec::new();
                let msg_text = &message.message;
                if !msg_text.is_empty() {
                    blocks.push(json!({"type": "text", "text": msg_text}));
                }
                for tr in tool_call_results {
                    blocks.push(json!({
                        "type": "tool_result",
                        "tool_use_id": tr.tool_call_id,
                        "content": tr.content,
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(json!({"type": "text", "text": ""}));
                }
                out.push(json!({"role": "user", "content": blocks}));
            }
            Message::Assistant { response } => {
                let blocks = response.blocks();
                let mut msg_blocks: Vec<Value> = Vec::new();
                for block in blocks {
                    match block {
                        Block::Text(text) => {
                            if !text.is_empty() {
                                msg_blocks.push(json!({"type": "text", "text": text}));
                            }
                        }
                        Block::Reasoning { text } => {
                            if !text.is_empty() {
                                msg_blocks.push(json!({"type": "thinking", "thinking": text}));
                            }
                        }
                        Block::ToolCall(tc) => {
                            let input: Value = serde_json::from_str(&tc.arguments)
                                .unwrap_or(Value::Object(Default::default()));
                            msg_blocks.push(json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": input,
                            }));
                        }
                    }
                }
                if msg_blocks.is_empty() {
                    msg_blocks.push(json!({"type": "text", "text": ""}));
                }
                out.push(json!({"role": "assistant", "content": msg_blocks}));
            }
        }
    }
    (system, out)
}

fn convert_tools(tools: Vec<provider::ToolDefinition>) -> Vec<Value> {
    tools
        .into_iter()
        .map(|t| {
            let mut spec = json!({
                "name": t.name,
                "input_schema": t.parameters,
            });
            if let Some(desc) = t.description {
                spec["description"] = json!(desc);
            }
            spec
        })
        .collect()
}

fn build_body(model: &str, request: LlmRequest, do_stream: bool) -> Value {
    let (system, messages) = convert_messages(request.messages());
    let tools = convert_tools(request.tools().to_vec());

    let mut body = json!({
        "model": model,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "messages": messages,
    });
    if let Some(sys) = system {
        body["system"] = json!(sys);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    if do_stream {
        body["stream"] = json!(true);
    }
    body
}

fn parse_usage(u: &Value) -> Option<TokenUsage> {
    if !u.is_object() {
        return None;
    }
    Some(TokenUsage {
        prompt_tokens: u["input_tokens"].as_i64().map(|n| n as i32),
        completion_tokens: u["output_tokens"].as_i64().map(|n| n as i32),
        total_tokens: u["input_tokens"]
            .as_i64()
            .zip(u["output_tokens"].as_i64())
            .map(|(i, o)| (i + o) as i32),
        cached_tokens: u["cache_read_input_tokens"].as_i64().map(|n| n as i32),
        cache_creation_tokens: u["cache_creation_input_tokens"].as_i64().map(|n| n as i32),
    })
}

fn parse_response(data: &Value) -> CompletedLlmResponse {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();

    if let Some(blocks) = data["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(t) = block["text"].as_str() {
                        text.push_str(t);
                    }
                }
                Some("thinking") => {
                    if let Some(t) = block["thinking"].as_str() {
                        reasoning.push_str(t);
                    }
                }
                Some("tool_use") => {
                    if let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) {
                        tool_calls.push(ToolCallRequest {
                            id: id.to_string(),
                            name: name.to_string(),
                            arguments: serde_json::to_string(&block["input"]).unwrap_or_default(),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    let mut blocks: Vec<Block> = Vec::new();
    if !text.is_empty() {
        blocks.push(Block::Text(text));
    }
    if !reasoning.is_empty() {
        blocks.push(Block::Reasoning { text: reasoning });
    }
    for tc in tool_calls {
        blocks.push(Block::ToolCall(tc));
    }

    CompletedLlmResponse {
        response: provider::AssistantResponse::new(blocks).unwrap(),
        usage: parse_usage(&data["usage"]),
        stop_reason: data["stop_reason"].as_str().map(str::to_string),
    }
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(
        &self,
        model: &str,
        request: LlmRequest,
    ) -> Result<CompletedLlmResponse, LlmError> {
        let body = build_body(model, request, false);

        let resp = self
            .client
            .post(&self.messages_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(errors::from_transport)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let headers = resp.headers().clone();
            let text = resp.text().await.unwrap_or_default();
            return Err(errors::from_response(status, &headers, text));
        }

        let data: Value = resp.json().await.map_err(|e| errors::parse_error(format!("parse error: {}", e)))?;

        Ok(parse_response(&data))
    }

    async fn stream(
        &self,
        model: &str,
        request: LlmRequest,
        sink: provider::EventSink<'_>,
    ) -> Result<StreamEnd, LlmError> {
        let body = build_body(model, request, true);

        let resp = self
            .client
            .post(&self.messages_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(errors::from_transport)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let headers = resp.headers().clone();
            let text = resp.text().await.unwrap_or_default();
            return Err(errors::from_response(status, &headers, text));
        }

        struct BlockState {
            kind: BlockKind,
            idx: usize,
        }

        let mut next_block_idx: usize = 0;
        let mut current_block: Option<BlockState> = None;
        let mut input_tokens: Option<i32> = None;
        let mut output_tokens: Option<i32> = None;
        let mut cached_tokens: Option<i32> = None;
        let mut cache_creation_tokens: Option<i32> = None;
        let mut stop_reason: Option<String> = None;
        let mut buf = Vec::new();

        let bytes = resp.bytes_stream();
        futures::pin_mut!(bytes);

        while let Some(chunk) = bytes.next().await {
            match chunk {
                Err(e) => return Err(errors::from_transport(e)),
                Ok(raw) => {
                    buf.extend_from_slice(&raw);
                    while let Some(nl) = buf.iter().position(|byte| *byte == b'\n') {
                        let mut raw_line: Vec<u8> = buf.drain(..=nl).collect();
                        raw_line.pop();
                        if raw_line.last() == Some(&b'\r') {
                            raw_line.pop();
                        }
                        let line = String::from_utf8_lossy(&raw_line);

                        let Some(data_str) = line.strip_prefix("data:").map(str::trim_start) else { continue };
                        if data_str == "[DONE]" {
                            return Ok(StreamEnd { usage: None, stop_reason: None });
                        }

                        let event: Value = match serde_json::from_str(data_str) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        if event["type"].as_str() == Some("error") {
                            let message = event["error"]["message"]
                                .as_str()
                                .unwrap_or("Anthropic stream error")
                                .to_string();
                            return Err(errors::stream_error(
                                message,
                                None,
                                event["error"]["type"].as_str(),
                            ));
                        }

                        match event["type"].as_str() {
                            Some("message_start") => {
                                let usage = &event["message"]["usage"];
                                input_tokens = usage["input_tokens"].as_i64().map(|n| n as i32);
                                cached_tokens = usage["cache_read_input_tokens"].as_i64().map(|n| n as i32);
                                cache_creation_tokens = usage["cache_creation_input_tokens"].as_i64().map(|n| n as i32);
                            }
                            Some("content_block_start") => {
                                let block = &event["content_block"];
                                let kind = match block["type"].as_str() {
                                    Some("text") => BlockKind::Text,
                                    Some("thinking") => BlockKind::Reasoning,
                                    Some("tool_use") => BlockKind::ToolCall {
                                        id: block["id"].as_str().unwrap_or("").to_string(),
                                        name: block["name"].as_str().unwrap_or("").to_string(),
                                    },
                                    _ => continue,
                                };
                                if let Some(previous) = current_block.take() {
                                    sink(StreamEvent::BlockEnd { index: previous.idx });
                                }
                                let idx = next_block_idx;
                                next_block_idx += 1;
                                current_block = Some(BlockState { idx, kind: kind.clone() });
                                sink(StreamEvent::BlockStart { index: idx, kind });
                            }
                            Some("content_block_delta") => {
                                let delta = &event["delta"];
                                if let Some(block) = current_block.as_mut() {
                                    match &block.kind {
                                        BlockKind::Text => {
                                            if let Some(t) = delta["text"].as_str().filter(|t| !t.is_empty()) {
                                                sink(StreamEvent::BlockDelta { index: block.idx, delta: t.to_string() });
                                            }
                                        }
                                        BlockKind::Reasoning => {
                                            if let Some(t) = delta["thinking"].as_str().filter(|t| !t.is_empty()) {
                                                sink(StreamEvent::BlockDelta { index: block.idx, delta: t.to_string() });
                                            }
                                        }
                                        BlockKind::ToolCall { .. } => {
                                            let partial = delta["partial_json"].as_str().unwrap_or("");
                                            sink(StreamEvent::BlockDelta { index: block.idx, delta: partial.to_string() });
                                        }
                                    }
                                }
                            }
                            Some("content_block_stop") => {
                                if let Some(block) = current_block.take() {
                                    sink(StreamEvent::BlockEnd { index: block.idx });
                                }
                            }
                            Some("message_delta") => {
                                stop_reason = event["delta"]["stop_reason"].as_str().map(str::to_string);
                                output_tokens = event["usage"]["output_tokens"].as_i64().map(|n| n as i32);
                            }
                            Some("message_stop") => {
                                let usage = Some(TokenUsage {
                                    prompt_tokens: input_tokens,
                                    completion_tokens: output_tokens,
                                    total_tokens: input_tokens.zip(output_tokens).map(|(i, o)| i + o),
                                    cached_tokens,
                                    cache_creation_tokens,
                                });
                                return Ok(StreamEnd { usage, stop_reason });
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Stream ended without message_stop
        let usage = Some(TokenUsage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens.zip(output_tokens).map(|(i, o)| i + o),
            cached_tokens,
            cache_creation_tokens,
        });
        Ok(StreamEnd { usage, stop_reason })
    }
}
