use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: Uuid,
    pub model: String,
    pub created_at: u64,
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
    SnapshotLoaded(Vec<SnapshotMessage>),
    Sse(SseEvent),
    NetworkError(String),
}

// ── Snapshot types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SnapshotToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SnapshotMessage {
    User { text: String },
    Assistant { reasoning: Option<String>, content: String, tool_calls: Vec<SnapshotToolCall> },
    Tool { tool_call_id: String, content: String },
}

#[derive(Debug)]
pub enum SseEvent {
    Content { text: String },
    Reasoning { text: String },
    ToolCall { id: String, name: String, arguments: String },
    ToolCallAutoApproved { id: String, name: String, arguments: String },
    ToolResult { id: String, content: String },
    Metrics { prompt_tokens: Option<u64>, completion_tokens: Option<u64>, total_tokens: Option<u64> },
    TurnComplete,
    StreamError { message: String },
}

// ── Raw server event deserialization ─────────────────────────────────────────
// The agent-server emits externally-tagged Rust enums, e.g.:
//   { "Clanker": { "ContentDelta": { "delta": "..." } } }

#[derive(Debug, Deserialize)]
pub enum RawSessionEvent {
    Clanker(RawClankerEvent),
    ToolCall(RawToolCallEvent),
    User(()),
}

#[derive(Debug, Deserialize)]
pub enum RawClankerEvent {
    ContentDelta { delta: String },
    ReasoningDelta { delta: String },
    ToolCallRequest { id: String, name: String, arguments: String },
    Done { usage: Option<RawUsage>, stop_reason: Option<String> },
}

#[derive(Debug, Deserialize)]
pub enum RawToolCallEvent {
    Result { id: String, result: String },
    Error { id: String, error: String },
    AutoApproved { id: String, name: String, arguments: String },
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
        RawSessionEvent::Clanker(c) => match c {
            RawClankerEvent::ContentDelta { delta } => vec![SseEvent::Content { text: delta }],
            RawClankerEvent::ReasoningDelta { delta } => vec![SseEvent::Reasoning { text: delta }],
            RawClankerEvent::ToolCallRequest { id, name, arguments } => {
                vec![SseEvent::ToolCall { id, name, arguments }]
            }
            RawClankerEvent::Done { usage, .. } => {
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
        },
        RawSessionEvent::ToolCall(t) => match t {
            RawToolCallEvent::Result { id, result } => {
                vec![SseEvent::ToolResult { id, content: result }]
            }
            RawToolCallEvent::Error { id, error } => {
                vec![SseEvent::ToolResult { id, content: format!("error: {error}") }]
            }
            RawToolCallEvent::AutoApproved { id, name, arguments } => {
                vec![SseEvent::ToolCallAutoApproved { id, name, arguments }]
            }
        },
        RawSessionEvent::User(_) => vec![],
    }
}
