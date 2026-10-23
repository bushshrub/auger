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
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

const DEFAULT_USER_AGENT: &str = "auger-code/0.1.0";

mod catalog;
mod errors;

/// Reasoning effort level for models that support it (o3, o4-mini, etc.).
/// When set, requests reasoning summaries via `reasoning.summary = "auto"`.
#[derive(Debug, Clone, Copy)]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }
}

pub struct OpenAiResponsesProvider {
    client: Client,
    api_key: String,
    base_url: String,
    reasoning_effort: Option<ReasoningEffort>,
}

impl OpenAiResponsesProvider {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self::with_user_agent(api_key, base_url, "")
    }

    pub fn with_user_agent(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        user_agent: impl AsRef<str>,
    ) -> Self {
        Self {
            client: Client::builder()
                .user_agent(format_user_agent(user_agent.as_ref()))
                .build()
                .expect("auger user agent must be valid HTTP header text"),
            api_key: api_key.into(),
            base_url: base_url.into(),
            reasoning_effort: None,
        }
    }

    pub fn with_reasoning(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    fn url(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }

    fn auth_header(&self) -> Option<String> {
        if self.api_key.is_empty() {
            None
        } else {
            Some(format!("Bearer {}", self.api_key))
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

// --- Request types ---

#[derive(Serialize)]
struct ReasoningParam {
    effort: &'static str,
    summary: &'static str,
}

#[derive(Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolSpec>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningParam>,
}

#[derive(Serialize)]
struct ToolSpec {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    parameters: Value,
}

// --- Response types ---

#[derive(Deserialize)]
struct ResponsesResponse {
    status: Option<String>,
    output: Vec<OutputItem>,
    usage: Option<ResponseUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum OutputItem {
    #[serde(rename = "message")]
    Message {
        content: Vec<ContentPart>,
        // llama.cpp puts thinking here instead of a separate reasoning output item
        reasoning_content: Option<String>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        // OpenAI spec uses `summary`, llama.cpp uses `content`
        #[serde(default)]
        summary: Vec<ReasoningPart>,
        #[serde(default)]
        content: Vec<ReasoningPart>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentPart {
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ReasoningPart {
    #[serde(rename = "summary_text")]
    SummaryText { text: String },
    #[serde(rename = "reasoning_text")]
    ReasoningText { text: String },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct ResponseUsage {
    input_tokens: i32,
    output_tokens: i32,
    // Some compatible servers omit this; see map_usage.
    total_tokens: Option<i32>,
    input_tokens_details: Option<InputTokenDetails>,
}

#[derive(Deserialize)]
struct InputTokenDetails {
    cached_tokens: Option<i32>,
}

// --- SSE event types ---

#[derive(Deserialize)]
struct SseEvent {
    #[serde(rename = "type")]
    kind: String,
    delta: Option<String>,
    // llama.cpp sends thinking here on the same delta event (mirrors chat completions behavior)
    reasoning_content: Option<String>,
    // item_id links delta events back to the item added in response.output_item.added.
    // llama.cpp omits output_index and uses item_id exclusively for function call events.
    item_id: Option<String>,
    item: Option<Value>,
    response: Option<CompletedResponse>,
    // Set on a top level `error` event.
    message: Option<String>,
    code: Option<String>,
}

#[derive(Deserialize)]
struct CompletedResponse {
    status: Option<String>,
    usage: Option<ResponseUsage>,
    error: Option<Value>,
}

// --- Helpers ---

fn messages_to_input(messages: &[provider::Message]) -> Vec<Value> {
    let mut items = Vec::new();
    for msg in messages {
        match msg {
            provider::Message::System(content) => {
                items.push(
                    serde_json::json!({"type": "message", "role": "system", "content": content}),
                );
            }
            provider::Message::User { blocks } => {
                for tr in provider::tool_results(blocks) {
                    items.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": tr.tool_call_id,
                        "output": tr.content,
                    }));
                }
                let msg_text = provider::render_text(blocks);
                if !msg_text.is_empty() {
                    items.push(
                        serde_json::json!({"type": "message", "role": "user", "content": msg_text}),
                    );
                }
            }
            provider::Message::Assistant { response } => {
                let blocks = response.blocks();
                let text = blocks
                    .iter()
                    .filter_map(|b| match b {
                        Block::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    items.push(serde_json::json!({"type": "message", "role": "assistant", "content": text}));
                }
                for block in blocks {
                    if let Block::ToolCall(tc) = block {
                        items.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.name,
                            "arguments": tc.arguments,
                        }));
                    }
                }
            }
        }
    }
    items
}

fn tools_to_spec(tools: &[provider::ToolDefinition]) -> Vec<ToolSpec> {
    tools
        .iter()
        .map(|t| ToolSpec {
            kind: "function",
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
        })
        .collect()
}

fn map_usage(u: ResponseUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: Some(u.input_tokens),
        completion_tokens: Some(u.output_tokens),
        total_tokens: u.total_tokens.or(Some(u.input_tokens + u.output_tokens)),
        cached_tokens: u.input_tokens_details.and_then(|d| d.cached_tokens),
        cache_creation_tokens: None,
    }
}

fn extract_text(output: &[OutputItem]) -> String {
    output
        .iter()
        .flat_map(|item| match item {
            OutputItem::Message { content, .. } => content
                .iter()
                .filter_map(|p| match p {
                    ContentPart::OutputText { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect::<Vec<_>>()
        .join("")
}

fn extract_reasoning(output: &[OutputItem]) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for item in output {
        match item {
            OutputItem::Reasoning { summary, content } => {
                for p in summary.iter().chain(content.iter()) {
                    let text = match p {
                        ReasoningPart::SummaryText { text } => Some(text.as_str()),
                        ReasoningPart::ReasoningText { text } => Some(text.as_str()),
                        ReasoningPart::Unknown => None,
                    };
                    if let Some(t) = text {
                        parts.push(t);
                    }
                }
            }
            OutputItem::Message {
                reasoning_content: Some(rc),
                ..
            } if !rc.is_empty() => {
                parts.push(rc.as_str());
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(""))
    }
}

fn extract_tool_calls(output: &[OutputItem]) -> Option<Vec<ToolCallRequest>> {
    let calls: Vec<ToolCallRequest> = output
        .iter()
        .filter_map(|item| match item {
            OutputItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => Some(ToolCallRequest {
                id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            }),
            _ => None,
        })
        .collect();
    if calls.is_empty() { None } else { Some(calls) }
}

fn stop_reason(status: Option<&str>, has_tool_calls: bool) -> Option<String> {
    if has_tool_calls {
        Some("tool_calls".into())
    } else {
        status.map(str::to_string)
    }
}

// --- Provider impl ---

#[async_trait::async_trait]
impl LlmProvider for OpenAiResponsesProvider {
    async fn complete(
        &self,
        model: &str,
        request: LlmRequest,
    ) -> Result<CompletedLlmResponse, LlmError> {
        let body = ResponsesRequest {
            model: model.to_string(),
            input: messages_to_input(request.messages()),
            tools: tools_to_spec(request.tools()),
            stream: false,
            reasoning: self.reasoning_effort.map(|e| ReasoningParam {
                effort: e.as_str(),
                summary: "auto",
            }),
        };

        let mut req = self
            .client
            .post(self.url())
            .header("Content-Type", "application/json");
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.json(&body).send().await.map_err(errors::from_transport)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let headers = resp.headers().clone();
            let text = resp.text().await.unwrap_or_default();
            return Err(errors::from_response(status, &headers, text));
        }

        let data: ResponsesResponse = resp.json().await.map_err(|e| errors::parse_error(format!("parse error: {}", e)))?;

        let tool_calls = extract_tool_calls(&data.output);
        let has_tool_calls = tool_calls.is_some();

        let mut blocks: Vec<Block> = Vec::new();
        let text = extract_text(&data.output);
        if !text.is_empty() {
            blocks.push(Block::Text(text));
        }
        if let Some(rc) = extract_reasoning(&data.output) {
            blocks.push(Block::Reasoning { text: rc });
        }
        if let Some(tcs) = tool_calls {
            for tc in tcs {
                blocks.push(Block::ToolCall(tc));
            }
        }

        Ok(CompletedLlmResponse {
            response: provider::AssistantResponse::new(blocks).ok_or_else(errors::empty_response)?,
            usage: data.usage.map(map_usage),
            stop_reason: stop_reason(data.status.as_deref(), has_tool_calls),
        })
    }

    async fn stream(
        &self,
        model: &str,
        request: LlmRequest,
        sink: provider::EventSink<'_>,
    ) -> Result<StreamEnd, LlmError> {
        let body = ResponsesRequest {
            model: model.to_string(),
            input: messages_to_input(request.messages()),
            tools: tools_to_spec(request.tools()),
            stream: true,
            reasoning: self.reasoning_effort.map(|e| ReasoningParam {
                effort: e.as_str(),
                summary: "auto",
            }),
        };

        let mut req = self
            .client
            .post(self.url())
            .header("Content-Type", "application/json");
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }

        let resp = req.json(&body).send().await.map_err(errors::from_transport)?;

        if !resp.status().is_success() {
            let status = resp.status();
            let headers = resp.headers().clone();
            let text = resp.text().await.unwrap_or_default();
            return Err(errors::from_response(status, &headers, text));
        }

        struct FcAccum {
            block_idx: usize,
        }

        let mut fc_accums: std::collections::HashMap<String, FcAccum> =
            std::collections::HashMap::new();
        let mut next_block_idx: usize = 0;
        let mut content_block: Option<(usize, BlockKind)> = None;
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

                        let Some(data) = line.strip_prefix("data:").map(str::trim_start) else { continue };
                        if data == "[DONE]" {
                            if let Some((index, _)) = content_block.take() {
                                sink(StreamEvent::BlockEnd { index });
                            }
                            return Ok(StreamEnd { usage: None, stop_reason: None });
                        }

                        let event: SseEvent = match serde_json::from_str(data) {
                            Ok(e) => e,
                            Err(_) => continue,
                        };

                        match event.kind.as_str() {
                            "response.output_text.delta" => {
                                if let Some(rc) = event.reasoning_content
                                    && !rc.is_empty()
                                {
                                        let idx = content_index(
                                            BlockKind::Reasoning,
                                            &mut content_block,
                                            &mut next_block_idx,
                                            sink,
                                        );
                                        sink(StreamEvent::BlockDelta { index: idx, delta: rc });
                                }
                                if let Some(delta) = event.delta
                                    && !delta.is_empty()
                                {
                                        let idx = content_index(
                                            BlockKind::Text,
                                            &mut content_block,
                                            &mut next_block_idx,
                                            sink,
                                        );
                                        sink(StreamEvent::BlockDelta { index: idx, delta });
                                }
                            }
                            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                                if let Some(delta) = event.delta
                                    && !delta.is_empty()
                                {
                                        let idx = content_index(
                                            BlockKind::Reasoning,
                                            &mut content_block,
                                            &mut next_block_idx,
                                            sink,
                                        );
                                        sink(StreamEvent::BlockDelta { index: idx, delta });
                                }
                            }
                            "response.output_item.added" => {
                                if let Some(item) = &event.item
                                    && item["type"] == "function_call"
                                {
                                        let call_id =
                                            item["call_id"].as_str().unwrap_or("").to_string();
                                        let name =
                                            item["name"].as_str().unwrap_or("").to_string();
                                        let key = item["id"]
                                            .as_str()
                                            .or_else(|| item["call_id"].as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        if !key.is_empty() {
                                            if let Some((index, _)) = content_block.take() {
                                                sink(StreamEvent::BlockEnd { index });
                                            }
                                            let block_idx = next_block_idx;
                                            next_block_idx += 1;
                                            sink(StreamEvent::BlockStart {
                                                index: block_idx,
                                                kind: BlockKind::ToolCall { id: call_id, name },
                                            });
                                            fc_accums.insert(
                                                key,
                                                FcAccum { block_idx },
                                            );
                                        }
                                }
                            }
                            "response.function_call_arguments.delta" => {
                                if let (Some(item_id), Some(delta)) =
                                    (&event.item_id, event.delta)
                                    && let Some(acc) = fc_accums.get(item_id)
                                {
                                        sink(StreamEvent::BlockDelta {
                                            index: acc.block_idx,
                                            delta,
                                        });
                                }
                            }
                            "response.output_item.done" => {
                                if let Some(item) = &event.item
                                    && item["type"] == "function_call"
                                {
                                        let key = item["id"]
                                            .as_str()
                                            .or_else(|| item["call_id"].as_str())
                                            .unwrap_or("");
                                        if let Some(acc) = fc_accums.get(key) {
                                            sink(StreamEvent::BlockEnd { index: acc.block_idx });
                                        }
                                }
                            }
                            "response.completed" | "response.incomplete" => {
                                if let Some((index, _)) = content_block.take() {
                                    sink(StreamEvent::BlockEnd { index });
                                }
                                let (usage, sr) = match event.response {
                                    Some(r) => {
                                        let has_tc = !fc_accums.is_empty();
                                        let sr = stop_reason(r.status.as_deref(), has_tc);
                                        (r.usage.map(map_usage), sr)
                                    }
                                    None => (None, None),
                                };
                                return Ok(StreamEnd { usage, stop_reason: sr });
                            }
                            "error" => {
                                let message = event
                                    .message
                                    .unwrap_or_else(|| "OpenAI stream error".to_string());
                                return Err(errors::stream_error(message, None, event.code.as_deref()));
                            }
                            "response.failed" => {
                                let error = event.response.as_ref().and_then(|response| response.error.as_ref());
                                let message = error
                                    .and_then(|error| error["message"].as_str())
                                    .unwrap_or("OpenAI response failed")
                                    .to_string();
                                let code = error.and_then(|error| error["code"].as_str());
                                return Err(errors::stream_error(message, None, code));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Err(errors::stream_error("OpenAI stream ended before response.completed".to_string(), None, None))
    }
}

fn content_index(
    kind: BlockKind,
    current: &mut Option<(usize, BlockKind)>,
    next: &mut usize,
    sink: provider::EventSink<'_>,
) -> usize {
    let same_kind = current.as_ref().is_some_and(|(_, current_kind)| {
        std::mem::discriminant(current_kind) == std::mem::discriminant(&kind)
    });
    if same_kind {
        return current.as_ref().expect("content block exists").0;
    }
    if let Some((index, _)) = current.take() {
        sink(StreamEvent::BlockEnd { index });
    }
    let index = *next;
    *next += 1;
    sink(StreamEvent::BlockStart { index, kind: kind.clone() });
    *current = Some((index, kind));
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider::LlmErrorKind;
    use provider::Message;
    use provider::UserBlock;
    use provider::fold_events;
    use provider::ToolResult;
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

    fn request() -> LlmRequest {
        LlmRequest::new(
            vec![Message::User { blocks: vec![UserBlock::Text("hi".to_string())] }],
            vec![],
        )
    }

    fn run_stream(body: &'static str) -> (Result<StreamEnd, LlmError>, Vec<StreamEvent>) {
        let provider = OpenAiResponsesProvider::new("key", serve(body));
        let mut events = Vec::new();
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut sink = |e: StreamEvent| events.push(e);
                provider.stream("model", request(), &mut sink).await
            });
        (result, events)
    }

    fn run_complete(body: &'static str) -> Result<CompletedLlmResponse, LlmError> {
        let provider = OpenAiResponsesProvider::new("key", serve(body));
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(provider.complete("model", request()))
    }

    #[test]
    fn assistant_text_is_kept_alongside_tool_calls() {
        let response = provider::AssistantResponse::new(vec![
            Block::Text("let me look".to_string()),
            Block::ToolCall(ToolCallRequest {
                id: "call_1".to_string(),
                name: "read".to_string(),
                arguments: "{}".to_string(),
            }),
        ])
        .unwrap();
        let input = messages_to_input(&[Message::Assistant { response }]);
        assert_eq!(input[0]["content"], "let me look");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_1");
    }

    #[test]
    fn incomplete_response_reports_usage_and_stop_reason() {
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"usage\":",
            r#"{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}"#,
            "\n\n",
        );
        let (result, events) = run_stream(body);
        let end = result.unwrap();
        assert_eq!(end.stop_reason.as_deref(), Some("incomplete"));
        assert_eq!(end.usage.unwrap().prompt_tokens, Some(3));
        assert_eq!(fold_events(&events).0, vec![Block::Text("hi".to_string())]);
    }

    #[test]
    fn usage_without_total_still_reports_usage() {
        let body = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":",
            r#"{"input_tokens":3,"output_tokens":2}}}"#,
            "\n\n",
        );
        let (result, _) = run_stream(body);
        let usage = result.unwrap().usage.unwrap();
        assert_eq!(usage.total_tokens, Some(5));
    }

    #[test]
    fn reasoning_summary_deltas_stream_as_reasoning() {
        let body = concat!(
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"think\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":",
            r#"{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}"#,
            "\n\n",
        );
        let (result, events) = run_stream(body);
        result.unwrap();
        assert_eq!(
            fold_events(&events).0,
            vec![Block::Reasoning { text: "think".to_string() }]
        );
    }

    #[test]
    fn failed_response_with_fatal_code_is_fatal() {
        let body = concat!(
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",",
            "\"error\":{\"code\":\"context_length_exceeded\",\"message\":\"too long\"}}}\n\n",
        );
        let (result, _) = run_stream(body);
        assert!(matches!(result, Err(LlmError { kind: LlmErrorKind::Fatal, .. })));
    }

    #[test]
    fn stream_cut_before_completed_is_an_error() {
        let body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n";
        let (result, _) = run_stream(body);
        assert!(matches!(result, Err(LlmError { kind: LlmErrorKind::Transient { .. }, .. })));
    }

    #[test]
    fn empty_output_is_an_error_not_a_panic() {
        let body = r#"{"status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":0,"total_tokens":1}}"#;
        assert!(run_complete(body).is_err());
    }

    #[test]
    fn tool_result_input_precedes_user_text() {
        let input = messages_to_input(&[Message::User {
            blocks: vec![
                UserBlock::Text("more".to_string()),
                UserBlock::ToolResult(ToolResult::new("call_1".to_string(), "out".to_string())),
            ],
        }]);
        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[1]["content"], "more");
    }
}
