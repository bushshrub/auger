use std::path::PathBuf;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionRecord {
    header: SessionHeader,
    events: Vec<EventRecord>
}

impl SessionRecord {
    pub fn new(session_id: Uuid, cwd: PathBuf, model: ModelInfo) -> Self {
        Self {
            header: SessionHeader::new(session_id, cwd, model),
            events: Vec::new()
        }
    }

}

/// A harness level event record.
#[derive(Serialize, Deserialize, Debug, Clone)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionHeader {
    version: u32,
    session_id: Uuid,
    created_at: DateTime<Utc>,
    cwd: PathBuf,
    model: ModelInfo,
}

impl SessionHeader {
    pub(crate) fn new(session_id: Uuid, cwd: PathBuf, model: ModelInfo) -> Self {
        Self {
            version: 1,
            session_id,
            created_at: Utc::now(),
            cwd,
            model
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ProviderType {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModelInfo {
    provider_type: ProviderType,
}

impl ModelInfo {
    pub fn new(provider_type: ProviderType) -> Self {
        Self { provider_type }
    }
}

impl EventRecord {
    /// Create a new event record with the timestamp being the time
    /// function was invoked.
    pub fn new(parent_id: Option<Uuid>, seq: u64, event: Event) -> Self {
        let timestamp = Utc::now();
        let id = Uuid::new_v7(timestamp.into());
        Self {
            id,
            parent_id,
            seq,
            timestamp,
            event
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum InputContent {
    UserMessage(String), // For now only string messages
    ToolCallResult {
        tool_call_id: String,
        content: Vec<ToolData>
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolData {
    Text(String)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolDecision {
    Approved,
    Denied
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolCallStatus {
    Success,
    Denied,
    Error
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AuthorizationSource {
    User,
    Policy
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssistantContent {
    Text(String),
    ToolCallRequest {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssistantStatus {
    Completed,
    Interrupted,
    Failed
}