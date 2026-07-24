use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: Uuid,
    pub model: String,
    pub context_window: u64,
    pub write_token: String,
    pub read_token: String,
}

#[derive(Debug, Clone)]
pub enum ToolDecision {
    Approved,
    Denied,
    Auto,
}

#[derive(Debug, Clone)]
pub enum ChatItem {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Reasoning {
        text: String,
        collapsed: bool,
    },
    Tool {
        id: String,
        name: String,
        args: String,
        result: Option<String>,
        decision: Option<ToolDecision>,
        /// Whether the full diff / file content is shown.
        expanded: bool,
    },
    Error {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Connecting,
    Idle,
    Running,
}

/// Events that flow through the unified event channel
#[derive(Debug)]
pub enum TuiEvent {
    Terminal(crossterm::event::Event),
    App(AppEvent),
}

#[derive(Debug)]
pub enum AppEvent {
    SessionsLoaded(Vec<SessionInfo>),
    SessionCreated {
        session_id: Uuid,
        write_token: String,
        read_token: String,
        context_window: u64,
    },
    /// Snapshot lines (NDJSON) — the app layer interprets them into ChatItems
    SnapshotLines(Vec<String>),
    Sse(SseEvent),
    NetworkError(String),
}

// ── SSE event types ──────────────────────────────────────────────────────────
// The server emits flat JSON with a "type" field, e.g.:
//   { "type": "text_delta", "text": "..." }
//   { "type": "tool_call", "id": "...", "name": "...", "arguments": "..." }

#[derive(Debug)]
pub enum SseEvent {
    Content {
        text: String,
    },
    Reasoning {
        text: String,
    },
    /// A *partial* tool call: `arguments` is a fragment to append, not a whole
    /// argument string. Providers stream these token by token.
    ToolCallDelta {
        id: String,
        name: String,
        arguments: String,
    },
    /// The authoritative, fully-assembled tool call.
    ToolCallComplete {
        id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        id: String,
        content: String,
        /// The server refused or the user denied the call; the UI shows this
        /// differently from a normal result.
        denied: bool,
    },
    Metrics {
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        total_tokens: Option<u64>,
    },
    TurnComplete,
    Interrupted,
    StreamClosed,
    StreamError {
        message: String,
    },
}

/// Flat JSON event matching the server's session_event_json output.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RawSessionEvent {
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    ToolCallComplete {
        id: String,
        name: String,
        arguments: String,
    },
    Done {
        usage: Option<RawUsage>,
        #[serde(rename = "stop_reason")]
        stop_reason: Option<String>,
    },
    ToolConsentRequired {
        tool_calls: Vec<RawToolCallRequest>,
    },
    ToolCallResult {
        id: String,
        result: RawToolCallResult,
    },
    Interrupted,
    StreamError {
        error: String,
    },
    Closed,
}

/// A tool call inside `tool_consent_required`. The server serialises
/// `ToolCallRequest` directly, so these are plain objects with no `type` tag —
/// treating them as a tagged enum fails with "missing field `type`".
#[derive(Debug, Deserialize)]
pub struct RawToolCallRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// The `result` of a `tool_call_result` event. The server serialises the whole
/// `ToolCallResult` object, not a plain string: expecting a string here failed
/// with "invalid type: map, expected a string".
#[derive(Debug, Deserialize)]
pub struct RawToolCallResult {
    pub outcome: RawToolOutcome,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawToolOutcome {
    Success { content: Vec<RawToolData> },
    Error { error: Vec<RawToolData> },
    Denied { reason: Option<String> },
    Interrupted,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawToolData {
    Text { text: String },
}

impl RawToolOutcome {
    /// Flatten an outcome into the text the UI shows under the tool call.
    fn into_text(self) -> String {
        fn join(data: Vec<RawToolData>) -> String {
            data.into_iter()
                .map(|RawToolData::Text { text }| text)
                .collect::<Vec<_>>()
                .join("")
        }
        match self {
            RawToolOutcome::Success { content } => join(content),
            RawToolOutcome::Error { error } => join(error),
            RawToolOutcome::Denied { reason } => {
                reason.unwrap_or_else(|| "Tool call was denied.".into())
            }
            RawToolOutcome::Interrupted => "Tool call was interrupted.".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RawUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// Transform a raw server event into one or more UI events.
pub fn transform_raw_event(ev: RawSessionEvent) -> Vec<SseEvent> {
    match ev {
        RawSessionEvent::TextDelta { text } => vec![SseEvent::Content { text }],
        RawSessionEvent::ReasoningDelta { text } => vec![SseEvent::Reasoning { text }],
        RawSessionEvent::ToolCall {
            id,
            name,
            arguments,
        } => vec![SseEvent::ToolCallDelta {
            id,
            name,
            arguments,
        }],
        // Carries the complete arguments, which supersede whatever the deltas
        // assembled.
        RawSessionEvent::ToolCallComplete {
            id,
            name,
            arguments,
        } => vec![SseEvent::ToolCallComplete {
            id,
            name,
            arguments,
        }],
        RawSessionEvent::Done { usage, .. } => {
            let mut out = Vec::new();
            if let Some(u) = usage {
                out.push(SseEvent::Metrics {
                    prompt_tokens: u.prompt_tokens,
                    completion_tokens: u.completion_tokens,
                    total_tokens: u.total_tokens,
                });
            }
            out.push(SseEvent::TurnComplete);
            out
        }
        // Consent lists whole tool calls, so they are complete by definition.
        RawSessionEvent::ToolConsentRequired { tool_calls } => tool_calls
            .into_iter()
            .map(|tc| SseEvent::ToolCallComplete {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
            })
            .collect(),
        RawSessionEvent::ToolCallResult { id, result } => {
            let denied = matches!(result.outcome, RawToolOutcome::Denied { .. });
            vec![SseEvent::ToolResult {
                id,
                content: result.outcome.into_text(),
                denied,
            }]
        }
        RawSessionEvent::Interrupted => vec![SseEvent::Interrupted],
        RawSessionEvent::StreamError { error } => {
            vec![SseEvent::StreamError { message: error }]
        }
        RawSessionEvent::Closed => vec![SseEvent::StreamClosed],
    }
}
