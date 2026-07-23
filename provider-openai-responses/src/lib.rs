use async_stream::stream;
use futures::StreamExt;
use provider::{
    CompletedLlmResponse, LlmError, LlmProvider, LlmRequest, LlmStream, StreamEvent, TokenUsage,
    ToolCallRequest,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_USER_AGENT: &str = "auger-code/0.1.0";

mod catalog;

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
    total_tokens: i32,
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
}

#[derive(Deserialize)]
struct CompletedResponse {
    status: Option<String>,
    usage: Option<ResponseUsage>,
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
            provider::Message::User {
                message,
                tool_call_results,
            } => {
                let msg_text = &message.message;
                if !msg_text.is_empty() {
                    items.push(
                        serde_json::json!({"type": "message", "role": "user", "content": msg_text}),
                    );
                }
                for tr in tool_call_results {
                    items.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": tr.tool_call_id,
                        "output": tr.content,
                    }));
                }
            }
            provider::Message::Assistant { response } => {
                let provider::AssistantResponse {
                    reasoning: _,
                    content,
                    tool_calls,
                } = response;
                if tool_calls.is_empty() {
                    if !content.is_empty() {
                        items.push(serde_json::json!({"type": "message", "role": "assistant", "content": content}));
                    }
                } else {
                    for tc in tool_calls {
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
        total_tokens: Some(u.total_tokens),
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

        let resp = req.json(&body).send().await.map_err(|e| LlmError {
            message: e.to_string(),
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError {
                message: format!("HTTP {}: {}", status, text),
            });
        }

        let data: ResponsesResponse = resp.json().await.map_err(|e| LlmError {
            message: format!("parse error: {}", e),
        })?;

        let tool_calls = extract_tool_calls(&data.output);
        let has_tool_calls = tool_calls.is_some();

        Ok(CompletedLlmResponse {
            content: extract_text(&data.output),
            reasoning: extract_reasoning(&data.output),
            tool_calls,
            usage: data.usage.map(map_usage),
            stop_reason: stop_reason(data.status.as_deref(), has_tool_calls),
        })
    }

    async fn stream(&self, model: &str, request: LlmRequest) -> Result<LlmStream, LlmError> {
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

        let resp = req.json(&body).send().await.map_err(|e| LlmError {
            message: e.to_string(),
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError {
                message: format!("HTTP {}: {}", status, text),
            });
        }

        struct FcAccum {
            call_id: String,
            name: String,
            arguments: String,
        }

        let s = stream! {
            let mut bytes = resp.bytes_stream();
            let mut buf = String::new();
            // Keyed by item_id (= call_id on llama.cpp, = item.id on OpenAI).
            // output_index is absent from llama.cpp's function call events so we
            // cannot use it as the key here.
            let mut fc_accums: std::collections::HashMap<String, FcAccum> =
                std::collections::HashMap::new();

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

                            let Some(data) = line.strip_prefix("data: ") else { continue };
                            if data == "[DONE]" {
                                return;
                            }

                            let event: SseEvent = match serde_json::from_str(data) {
                                Ok(e) => e,
                                Err(_) => continue,
                            };

                            match event.kind.as_str() {
                                "response.output_text.delta" => {
                                    if let Some(rc) = event.reasoning_content {
                                        if !rc.is_empty() {
                                            yield Ok(StreamEvent::ReasoningDelta(rc));
                                        }
                                    }
                                    if let Some(delta) = event.delta {
                                        if !delta.is_empty() {
                                            yield Ok(StreamEvent::TextDelta(delta));
                                        }
                                    }
                                }
                                "response.reasoning_text.delta" => {
                                    if let Some(delta) = event.delta {
                                        if !delta.is_empty() {
                                            yield Ok(StreamEvent::ReasoningDelta(delta));
                                        }
                                    }
                                }
                                "response.output_item.added" => {
                                    if let Some(item) = &event.item {
                                        if item["type"] == "function_call" {
                                            let call_id =
                                                item["call_id"].as_str().unwrap_or("").to_string();
                                            let name =
                                                item["name"].as_str().unwrap_or("").to_string();
                                            // OpenAI uses item.id as the item_id in delta events;
                                            // llama.cpp omits item.id and uses call_id as item_id.
                                            let key = item["id"]
                                                .as_str()
                                                .or_else(|| item["call_id"].as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            if !key.is_empty() {
                                                fc_accums.insert(
                                                    key,
                                                    FcAccum { call_id, name, arguments: String::new() },
                                                );
                                            }
                                        }
                                    }
                                }
                                "response.function_call_arguments.delta" => {
                                    if let (Some(item_id), Some(delta)) =
                                        (&event.item_id, event.delta)
                                    {
                                        if let Some(acc) = fc_accums.get_mut(item_id) {
                                            acc.arguments.push_str(&delta);
                                            yield Ok(StreamEvent::ToolCall {
                                                id: acc.call_id.clone(),
                                                name: acc.name.clone(),
                                                arguments: delta,
                                            });
                                        }
                                    }
                                }
                                "response.output_item.done" => {
                                    if let Some(item) = &event.item {
                                        if item["type"] == "function_call" {
                                            if let (Some(call_id), Some(name), Some(arguments)) = (
                                                item["call_id"].as_str(),
                                                item["name"].as_str(),
                                                item["arguments"].as_str(),
                                            ) {
                                                yield Ok(StreamEvent::ToolCallComplete {
                                                    id: call_id.to_string(),
                                                    name: name.to_string(),
                                                    arguments: arguments.to_string(),
                                                });
                                            }
                                        }
                                    }
                                }
                                "response.completed" => {
                                    let (usage, sr) = match event.response {
                                        Some(r) => {
                                            let has_tc = !fc_accums.is_empty();
                                            let sr = stop_reason(r.status.as_deref(), has_tc);
                                            (r.usage.map(map_usage), sr)
                                        }
                                        None => (None, None),
                                    };
                                    yield Ok(StreamEvent::Done { usage, stop_reason: sr });
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
