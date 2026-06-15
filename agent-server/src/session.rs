use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast, oneshot};
use uuid::Uuid;

pub const EVENT_CAPACITY: usize = 256;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentEvent {
    Content { text: String },
    ToolCall { id: String, name: String, arguments: serde_json::Value },
    ToolResult { id: String, content: serde_json::Value },
    /// Per-LLM-call stats, emitted once the model's turn finishes generating.
    Metrics(Metrics),
    TurnComplete,
    Error { message: String },
}

/// Token and timing stats for a single model generation.
#[derive(Clone, Debug, Serialize)]
pub struct Metrics {
    /// Tokens in the prompt (the full conversation sent this call).
    pub prompt_tokens: Option<usize>,
    /// Tokens the model generated this call.
    pub completion_tokens: Option<usize>,
    /// prompt + completion.
    pub total_tokens: Option<usize>,
    /// Model context window, for the usage bar.
    pub context_window: usize,
    /// Time to first streamed token, milliseconds.
    pub ttft_ms: Option<u64>,
    /// Average generation throughput (completion tokens / generation seconds).
    pub tokens_per_sec: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Running,
    AwaitingApproval,
}

pub struct PendingApproval {
    pub tool_call_id: String,
    pub tx: oneshot::Sender<bool>,
}

pub struct Session {
    pub id: Uuid,
    pub owner_token: String,
    pub viewer_token: String,
    pub model: String,
    pub context_window: usize,
    pub status: Mutex<SessionStatus>,
    pub history: Mutex<Vec<provider::Message>>,
    pub pending_approval: Mutex<Option<PendingApproval>>,
    pub events: broadcast::Sender<AgentEvent>,
}

impl Session {
    pub fn new(id: Uuid, model: String, context_window: usize) -> Arc<Self> {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Arc::new(Self {
            id,
            owner_token: Uuid::new_v4().to_string(),
            viewer_token: Uuid::new_v4().to_string(),
            model,
            context_window,
            status: Mutex::new(SessionStatus::Idle),
            history: Mutex::new(Vec::new()),
            pending_approval: Mutex::new(None),
            events,
        })
    }

    pub fn is_owner(&self, token: &str) -> bool {
        token == self.owner_token
    }

    pub fn can_view(&self, token: &str) -> bool {
        token == self.owner_token || token == self.viewer_token
    }
}

// --- Request / Response types ---

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct CreateSessionResponse {
    pub session_id: Uuid,
    pub owner_token: String,
    pub viewer_token: String,
    pub context_window: usize,
}

#[derive(Deserialize)]
pub struct UserInputRequest {
    pub content: String,
}

#[derive(Deserialize)]
pub struct ApproveRequest {
    pub tool_call_id: String,
    pub approved: bool,
}
