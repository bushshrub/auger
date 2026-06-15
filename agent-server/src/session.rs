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
    TurnComplete,
    Error { message: String },
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
    pub status: Mutex<SessionStatus>,
    pub history: Mutex<Vec<provider::Message>>,
    pub pending_approval: Mutex<Option<PendingApproval>>,
    pub events: broadcast::Sender<AgentEvent>,
}

impl Session {
    pub fn new(id: Uuid, model: String) -> Arc<Self> {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Arc::new(Self {
            id,
            owner_token: Uuid::new_v4().to_string(),
            viewer_token: Uuid::new_v4().to_string(),
            model,
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
