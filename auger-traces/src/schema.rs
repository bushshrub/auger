use chrono::{DateTime, Utc};
use getset::Getters;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceRecord {
    Session(SessionHeader),
    Turn(TurnRecord),
    Event(EventRecord),
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct SessionHeader {
    version: u32,
    session_id: Uuid,
    root_turn_id: TurnId,
    created_at: DateTime<Utc>,
    cwd: PathBuf,
    model: ModelInfo,
}

impl SessionHeader {
    pub fn new(
        version: u32,
        session_id: Uuid,
        root_turn_id: TurnId,
        created_at: DateTime<Utc>,
        cwd: PathBuf,
        model: ModelInfo,
    ) -> Self {
        Self {
            version,
            session_id,
            root_turn_id,
            created_at,
            cwd,
            model,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct ModelInfo {
    provider: String,
    id: String,
}

impl ModelInfo {
    pub fn new(provider: String, id: String) -> Self {
        Self { provider, id }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct TurnRecord {
    id: TurnId,
    parent_turn_id: Option<TurnId>,
    // TODO: Add a sequence number when the source history records one.
    timestamp: DateTime<Utc>,
    #[serde(flatten)]
    turn: Turn,
}

impl TurnRecord {
    pub fn new(id: TurnId, parent_turn_id: Option<TurnId>, timestamp: DateTime<Utc>, turn: Turn) -> Self {
        Self {
            id,
            parent_turn_id,
            timestamp,
            turn,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Turn {
    InputMessage(InputMessage),
    AssistantMessage(AssistantMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct InputMessage {
    content: Vec<InputContent>,
}

impl InputMessage {
    pub fn new(content: Vec<InputContent>) -> Self {
        Self { content }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct AssistantMessage {
    status: AssistantStatus,
    content: Vec<AssistantContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_metadata: Option<Value>,
}

impl AssistantMessage {
    pub fn new(status: AssistantStatus, content: Vec<AssistantContent>) -> Self {
        Self {
            status,
            content,
            provider_metadata: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContent {
    Text(TextData),
    ToolResult(InputToolResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct InputToolResult {
    tool_call_id: ToolCallId,
    // TODO: Persist the tool result status.
    content: Vec<ToolData>,
}

impl InputToolResult {
    pub fn new(tool_call_id: ToolCallId, content: Vec<ToolData>) -> Self {
        Self {
            tool_call_id,
            content,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Reasoning(TextData),
    Text(TextData),
    ToolCall(AssistantToolCall),
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct AssistantToolCall {
    id: ToolCallId,
    name: String,
    arguments: String,
}

impl AssistantToolCall {
    pub fn new(id: ToolCallId, name: String, arguments: String) -> Self {
        Self { id, name, arguments }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantStatus {
    Completed,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct EventRecord {
    id: EventId,
    turn_id: TurnId,
    parent_event_id: Option<EventId>,
    // TODO: Add a sequence number when the source history records one.
    timestamp: DateTime<Utc>,
    #[serde(flatten)]
    event: Event,
}

impl EventRecord {
    pub fn new(
        id: EventId,
        turn_id: TurnId,
        parent_event_id: Option<EventId>,
        timestamp: DateTime<Utc>,
        event: Event,
    ) -> Self {
        Self {
            id,
            turn_id,
            parent_event_id,
            timestamp,
            event,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ToolCallRequested(ToolCallRequested),
    ToolAuthorization(ToolAuthorization),
    ToolCallResult(ToolCallResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct ToolCallRequested {
    tool_call_id: ToolCallId,
    tool_name: String,
    arguments: String,
}

impl ToolCallRequested {
    pub fn new(tool_call_id: ToolCallId, tool_name: String, arguments: String) -> Self {
        Self {
            tool_call_id,
            tool_name,
            arguments,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct ToolAuthorization {
    tool_call_id: ToolCallId,
    decision: ToolDecision,
    source: AuthorizationSource,
    reason: Option<String>,
}

impl ToolAuthorization {
    pub fn new(
        tool_call_id: ToolCallId,
        decision: ToolDecision,
        source: AuthorizationSource,
        reason: Option<String>,
    ) -> Self {
        Self {
            tool_call_id,
            decision,
            source,
            reason,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct ToolCallResult {
    tool_call_id: ToolCallId,
    // TODO: Persist the tool result status.
    content: Vec<ToolData>,
}

impl ToolCallResult {
    pub fn new(tool_call_id: ToolCallId, content: Vec<ToolData>) -> Self {
        Self {
            tool_call_id,
            content,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolData {
    Text(TextData),
}

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct TextData {
    text: String,
}

impl TextData {
    pub fn new(text: String) -> Self {
        Self { text }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDecision {
    Approved,
    Denied,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Success,
    Denied,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationSource {
    User,
    Policy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(Uuid);

impl From<Uuid> for TurnId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<TurnId> for Uuid {
    fn from(value: TurnId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(Uuid);

impl From<Uuid> for EventId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<EventId> for Uuid {
    fn from(value: EventId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(String);

impl From<String> for ToolCallId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<ToolCallId> for String {
    fn from(value: ToolCallId) -> Self {
        value.0
    }
}
