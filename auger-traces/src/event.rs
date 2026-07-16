use crate::{AuthorizationSource, ToolCallStatus, ToolData, ToolDecision};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A harness-level event record.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EventRecord {
    /// The ID of this event.
    id: Uuid,
    /// The logical parent of this event.
    parent_id: Option<Uuid>,
    seq: u64,
    /// Timestamp at which this event occurred.
    timestamp: DateTime<Utc>,
    event: Event,
}

impl EventRecord {
    /// Create a new event record with the current timestamp.
    pub fn new(parent_id: Option<Uuid>, seq: u64, event: Event) -> Self {
        let timestamp = Utc::now();
        let id = Uuid::now_v7();
        Self {
            id,
            parent_id,
            seq,
            timestamp,
            event,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum InputContent {
    UserMessage(String),
    ToolCallResult {
        tool_call_id: String,
        content: Vec<ToolData>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    InputMessage {
        content: Vec<InputContent>,
    },
    /// Result emitted by the model.
    AssistantMessage {
        status: AssistantStatus,
        content: Vec<AssistantContent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<serde_json::Value>,
    },
    /// Tool authorization triggered by either the user or policy.
    ToolAuthorization {
        tool_call_id: String,
        decision: ToolDecision,
        source: AuthorizationSource,
        reason: Option<String>,
    },
    /// The result of a tool call.
    ToolCallResult {
        tool_call_id: String,
        status: ToolCallStatus,
        content: Vec<ToolData>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssistantContent {
    Text(String),
    ToolCallRequest {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssistantStatus {
    Completed,
    Interrupted,
    Failed,
}
