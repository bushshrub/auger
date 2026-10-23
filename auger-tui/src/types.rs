use serde::Deserialize;
use serde_json::Value;
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
    /// Allowed by the approval policy; holds the node's name.
    Auto(String),
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
        /// When the first reasoning delta arrived; `None` when restored from a
        /// snapshot, where the duration is unknown.
        started: Option<std::time::Instant>,
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
        model: String,
        write_token: String,
        read_token: String,
        context_window: u64,
    },
    /// Snapshot lines (NDJSON records), folded into the transcript.
    SnapshotLines(Vec<String>),
    Sse(SseEvent),
    NetworkError(String),
}

// -- SSE event types ----------------------------------------------------------
// The server emits flat JSON with a "type" field, e.g.:
//   { "type": "text_delta", "text": "..." }
//   { "type": "record", "record": { "seq": 3, "type": "stream", ... } }

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
    /// A call that waits for the user's approval.
    ToolConsent {
        id: String,
        name: String,
        arguments: String,
    },
    /// A tool phase ended with a denial: the session waits for the next message.
    ToolResultsWithheld,
    /// A session record, folded into the transcript.
    Record(Value),
    StreamClosed,
    /// A frame that could not be parsed.
    Error {
        message: String,
    },
}

/// Flat JSON event matching the server's SessionEvent output.
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
    ToolConsentRequired {
        tool_calls: Vec<RawToolCallRequest>,
    },
    ToolResultsWithheld,
    Record {
        record: Value,
    },
    Closed,
}

/// A tool call inside `tool_consent_required`. The server serialises
/// `ToolCallRequest` directly, so these are plain objects with no `type` tag -
/// treating them as a tagged enum fails with "missing field `type`".
#[derive(Debug, Deserialize)]
pub struct RawToolCallRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: String,
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
        // Consent lists whole tool calls, so they are complete by definition.
        RawSessionEvent::ToolConsentRequired { tool_calls } => tool_calls
            .into_iter()
            .map(|tc| SseEvent::ToolConsent {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
            })
            .collect(),
        RawSessionEvent::ToolResultsWithheld => vec![SseEvent::ToolResultsWithheld],
        RawSessionEvent::Record { record } => vec![SseEvent::Record(record)],
        RawSessionEvent::Closed => vec![SseEvent::StreamClosed],
    }
}
