use async_stream::stream;
use futures::StreamExt;
use provider::types::Message;
use provider::{
    LlmError, LlmProvider, LlmRequest, LlmResponse, LlmStream, StreamEvent, TokenUsage,
    ToolCallRequest,
};
use reqwest::Client;
use serde_json::{Value, json};

mod catalog;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8096;

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    messages_url: String,
    models_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let base = base_url.into();
        let base = if base.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            base
        };
        let base = base.trim_end_matches('/');
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            messages_url: format!("{base}/v1/messages"),
            models_url: format!("{base}/v1/models"),
        }
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
                let msg_text = message.message();
                if !msg_text.is_empty() {
                    blocks.push(json!({"type": "text", "text": msg_text}));
                }
                for tr in tool_call_results {
                    blocks.push(json!({
                        "type": "tool_result",
                        "tool_use_id": tr.id(),
                        "content": tr.content(),
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(json!({"type": "text", "text": ""}));
                }
                out.push(json!({"role": "user", "content": blocks}));
            }
            Message::Assistant {
                reasoning: _,
                content,
                tool_calls,
            } => {
                let mut blocks: Vec<Value> = Vec::new();
                if !content.is_empty() {
                    blocks.push(json!({"type": "text", "text": content}));
                }
                for tc in tool_calls {
                    let input: Value = serde_json::from_str(&tc.arguments)
                        .unwrap_or(Value::Object(Default::default()));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": input,
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(json!({"type": "text", "text": ""}));
                }
                out.push(json!({"role": "assistant", "content": blocks}));
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

fn parse_response(data: &Value) -> LlmResponse {
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

    LlmResponse {
        content: text,
        reasoning: if reasoning.is_empty() {
            None
        } else {
            Some(reasoning)
        },
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        usage: parse_usage(&data["usage"]),
        stop_reason: data["stop_reason"].as_str().map(str::to_string),
    }
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, model: &str, request: LlmRequest) -> Result<LlmResponse, LlmError> {
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

        let data: Value = resp.json().await.map_err(|e| LlmError {
            message: format!("parse error: {}", e),
        })?;

        Ok(parse_response(&data))
    }

    async fn stream(&self, model: &str, request: LlmRequest) -> Result<LlmStream, LlmError> {
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

        struct BlockState {
            kind: String,
            id: Option<String>,
            name: Option<String>,
            args: String,
        }

        let s = stream! {
            let mut bytes = resp.bytes_stream();
            let mut buf = String::new();
            let mut current_block: Option<BlockState> = None;
            let mut input_tokens: Option<i32> = None;
            let mut output_tokens: Option<i32> = None;
            let mut cached_tokens: Option<i32> = None;
            let mut cache_creation_tokens: Option<i32> = None;
            let mut stop_reason: Option<String> = None;

            while let Some(chunk) = bytes.next().await {
                match chunk {
                    Err(e) => {
                        yield Err(LlmError { message: e.to_string() });
                        return;
                    }
                    Ok(raw) => {
                        buf.push_str(&String::from_utf8_lossy(&raw));
                        loop {
                            let Some(nl) = buf.find('\n') else { break };
                            let line = buf[..nl].trim_end_matches('\r').to_string();
                            buf = buf[nl + 1..].to_string();

                            let Some(data_str) = line.strip_prefix("data: ") else { continue };
                            if data_str == "[DONE]" {
                                return;
                            }

                            let event: Value = match serde_json::from_str(data_str) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };

                            match event["type"].as_str() {
                                Some("message_start") => {
                                    let usage = &event["message"]["usage"];
                                    input_tokens = usage["input_tokens"].as_i64().map(|n| n as i32);
                                    cached_tokens = usage["cache_read_input_tokens"].as_i64().map(|n| n as i32);
                                    cache_creation_tokens = usage["cache_creation_input_tokens"].as_i64().map(|n| n as i32);
                                }
                                Some("content_block_start") => {
                                    let block = &event["content_block"];
                                    current_block = Some(BlockState {
                                        kind: block["type"].as_str().unwrap_or("").to_string(),
                                        id: block["id"].as_str().map(str::to_string),
                                        name: block["name"].as_str().map(str::to_string),
                                        args: String::new(),
                                    });
                                }
                                Some("content_block_delta") => {
                                    let delta = &event["delta"];
                                    let ev = if let Some(block) = current_block.as_mut() {
                                        match block.kind.as_str() {
                                            "text" => delta["text"]
                                                .as_str()
                                                .filter(|t| !t.is_empty())
                                                .map(|t| Ok(StreamEvent::TextDelta(t.to_string()))),
                                            "thinking" => delta["thinking"]
                                                .as_str()
                                                .filter(|t| !t.is_empty())
                                                .map(|t| Ok(StreamEvent::ReasoningDelta(t.to_string()))),
                                            "tool_use" => {
                                                let partial = delta["partial_json"].as_str().unwrap_or("");
                                                block.args.push_str(partial);
                                                Some(Ok(StreamEvent::ToolCall {
                                                    id: block.id.clone().unwrap_or_default(),
                                                    name: block.name.clone().unwrap_or_default(),
                                                    arguments: partial.to_string(),
                                                }))
                                            }
                                            _ => None,
                                        }
                                    } else {
                                        None
                                    };
                                    if let Some(e) = ev {
                                        yield e;
                                    }
                                }
                                Some("content_block_stop") => {
                                    if let Some(block) = current_block.take() {
                                        if block.kind == "tool_use" {
                                            yield Ok(StreamEvent::ToolCallComplete {
                                                id: block.id.unwrap_or_default(),
                                                name: block.name.unwrap_or_default(),
                                                arguments: block.args,
                                            });
                                        }
                                    }
                                }
                                Some("message_delta") => {
                                    stop_reason = event["delta"]["stop_reason"]
                                        .as_str()
                                        .map(str::to_string);
                                    output_tokens = event["usage"]["output_tokens"]
                                        .as_i64()
                                        .map(|n| n as i32);
                                }
                                Some("message_stop") => {
                                    let usage = Some(TokenUsage {
                                        prompt_tokens: input_tokens,
                                        completion_tokens: output_tokens,
                                        total_tokens: input_tokens
                                            .zip(output_tokens)
                                            .map(|(i, o)| i + o),
                                        cached_tokens,
                                        cache_creation_tokens,
                                    });
                                    yield Ok(StreamEvent::Done {
                                        usage,
                                        stop_reason: stop_reason.take(),
                                    });
                                    return;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        .boxed();

        Ok(LlmStream::new(s))
    }
}
