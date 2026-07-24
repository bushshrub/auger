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
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    ToolCallComplete {
        id: String,
    },
    ToolResult {
        id: String,
        content: String,
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
        tool_calls: Vec<RawToolCallEvent>,
    },
    ToolCallResult {
        id: String,
        result: String,
    },
    Interrupted,
    StreamError {
        error: String,
    },
    Closed,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RawToolCallEvent {
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
        } => vec![SseEvent::ToolCall {
            id,
            name,
            arguments,
        }],
        RawSessionEvent::ToolCallComplete { id, .. } => {
            vec![SseEvent::ToolCallComplete { id }]
        }
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
        RawSessionEvent::ToolConsentRequired { tool_calls } => tool_calls
            .into_iter()
            .filter_map(|tc| match tc {
                RawToolCallEvent::ToolCall {
                    id,
                    name,
                    arguments,
                } => Some(SseEvent::ToolCall {
                    id,
                    name,
                    arguments,
                }),
                _ => None,
            })
            .collect(),
        RawSessionEvent::ToolCallResult { id, result } => {
            let content = if result.starts_with('{') || result.starts_with('[') {
                result
            } else {
                result
            };
            vec![SseEvent::ToolResult {
                id,
                content,
            }]
        }
        RawSessionEvent::Interrupted => vec![SseEvent::Interrupted],
        RawSessionEvent::StreamError { error } => {
            vec![SseEvent::StreamError { message: error }]
        }
        RawSessionEvent::Closed => vec![SseEvent::StreamClosed],
    }
}
