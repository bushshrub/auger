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
            Message::User { blocks: user_blocks } => {
                // Anthropic wants tool results ahead of any text.
                let mut blocks: Vec<Value> = provider::tool_results(user_blocks)
                    .map(|tr| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": tr.tool_call_id,
                            "content": tr.content,
                        })
                    })
                    .collect();
                let msg_text = provider::render_text(user_blocks);
                if !msg_text.is_empty() {
                    blocks.push(json!({"type": "text", "text": msg_text}));
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

fn parse_response(data: &Value) -> Result<CompletedLlmResponse, LlmError> {
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

    Ok(CompletedLlmResponse {
        response: provider::AssistantResponse::new(blocks).ok_or_else(errors::empty_response)?,
        usage: parse_usage(&data["usage"]),
        stop_reason: data["stop_reason"].as_str().map(str::to_string),
    })
}

/// A content block being streamed. Tool input arrives as partial JSON that
/// can be empty for tools without arguments, so those close with "{}".
struct OpenBlock {
    kind: BlockKind,
    idx: usize,
    got_input: bool,
}

fn close_block(block: OpenBlock, sink: provider::EventSink<'_>) {
    if matches!(block.kind, BlockKind::ToolCall { .. }) && !block.got_input {
        sink(StreamEvent::BlockDelta { index: block.idx, delta: "{}".to_string() });
    }
    sink(StreamEvent::BlockEnd { index: block.idx });
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

        parse_response(&data)
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

        let mut next_block_idx: usize = 0;
        let mut current_block: Option<OpenBlock> = None;
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
                                    close_block(previous, sink);
                                }
                                let idx = next_block_idx;
                                next_block_idx += 1;
                                current_block = Some(OpenBlock { idx, kind: kind.clone(), got_input: false });
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
                                            if let Some(partial) = delta["partial_json"].as_str().filter(|t| !t.is_empty()) {
                                                block.got_input = true;
                                                sink(StreamEvent::BlockDelta { index: block.idx, delta: partial.to_string() });
                                            }
                                        }
                                    }
                                }
                            }
                            Some("content_block_stop") => {
                                if let Some(block) = current_block.take() {
                                    close_block(block, sink);
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

        // The connection closed before message_stop, so the response is cut short.
        Err(errors::stream_error("Anthropic stream ended before message_stop".to_string(), None, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider::UserBlock;
    use provider::LlmErrorKind;
    use provider::fold_events;
    use std::io::{Read, Write};

    /// Serves one HTTP response with `body` and returns the base URL.
    fn serve(body: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut chunk = [0u8; 4096];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&req).to_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map(|v| v.trim().parse().unwrap())
                .unwrap_or(0);
            let body_start = req.windows(4).position(|w| w == b"\r\n\r\n").map_or(req.len(), |p| p + 4);
            let mut have = req.len() - body_start;
            while have < len {
                let n = sock.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                have += n;
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).unwrap();
        });
        format!("http://{addr}")
    }

    fn run_stream(body: &'static str) -> (Result<StreamEnd, LlmError>, Vec<StreamEvent>) {
        let provider = AnthropicProvider::new("key", serve(body));
        let request = LlmRequest::new(
            vec![Message::User { blocks: vec![UserBlock::Text("hi".to_string())] }],
            vec![],
        );
        let mut events = Vec::new();
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut sink = |e: StreamEvent| events.push(e);
                provider.stream("model", request, &mut sink).await
            });
        (result, events)
    }

    const NO_ARG_TOOL_STREAM: &str = concat!(
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"ping\",\"input\":{}}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\"}}\n\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    #[test]
    fn tool_without_input_streams_empty_object() {
        let (result, events) = run_stream(NO_ARG_TOOL_STREAM);
        result.unwrap();
        let (blocks, _) = fold_events(&events);
        match &blocks[..] {
            [Block::ToolCall(tc)] => assert_eq!(tc.arguments, "{}"),
            other => panic!("unexpected blocks: {other:?}"),
        }
    }

    #[test]
    fn stream_cut_before_message_stop_is_an_error() {
        let truncated = NO_ARG_TOOL_STREAM.trim_end_matches("data: {\"type\":\"message_stop\"}\n\n");
        let (result, _) = run_stream(Box::leak(truncated.to_string().into_boxed_str()));
        assert!(matches!(result, Err(LlmError { kind: LlmErrorKind::Transient { .. }, .. })));
    }

    #[test]
    fn empty_content_is_an_error_not_a_panic() {
        let data = json!({"content": [], "stop_reason": "end_turn"});
        assert!(parse_response(&data).is_err());
    }
}
