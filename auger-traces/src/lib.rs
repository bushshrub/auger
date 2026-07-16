use chrono::{DateTime, Utc};
use uuid::Uuid;
use serde::{Deserialize, Serialize};

pub struct EventRecord {
    /// The ID of this event
    id: Uuid,
    /// The logical parent of this event.
    /// For example the logical parent of an assistant message could be a user message
    parent_id: Option<Uuid>,
    seq: u64,
    /// Timestamp at which this event occurred.
    timestamp: DateTime<Utc>,
    event: Event
}

#[derive(Debug, Deserialize, Serialize)]
pub enum InputContent {
    UserMessage(String),
    ToolCallResult {
        tool_call_id: String,
        content: Vec<ToolData>
    }// For now only string messages
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    InputMessage {
        content: Vec<InputContent>,
    },
    /// Result emitted by clanker
    AssistantMessage {
        status: AssistantStatus,
        content: Vec<AssistantContent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<serde_json::Value>,
    },
    /// Tool authorization event - can be triggered either by user or by policy
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
        content: Vec<ToolData>
    }
}

/// Data returned by tool.
/// Currently only text, image will eventually come.
#[derive(Debug, Serialize, Deserialize)]
pub enum ToolData {
    Text(String)
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ToolDecision {
    Approved,
    Denied
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ToolCallStatus {
    Success,
    Denied,
    Error
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AuthorizationSource {
    User,
    Policy
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AssistantContent {
    Text(String),
    ToolCallRequest {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AssistantStatus {
    Completed,
    Interrupted,
    Failed
}