use chrono::{DateTime, Utc};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    version: u32,
    session_id: Uuid,
    root_turn_id: TurnId,
    created_at: DateTime<Utc>,
    cwd: PathBuf,
    model: ModelInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    provider: String,
    id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    id: TurnId,
    parent_turn_id: Option<TurnId>,
    seq: u64,
    timestamp: DateTime<Utc>,
    #[serde(flatten)]
    turn: Turn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Turn {
    InputMessage(InputMessage),
    AssistantMessage(AssistantMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputMessage {
    content: Vec<InputContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    status: AssistantStatus,
    content: Vec<AssistantContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContent {
    Text(TextData),
    ToolResult(InputToolResult),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputToolResult {
    tool_call_id: ToolCallId,
    status: ToolCallStatus,
    content: Vec<ToolData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Reasoning(TextData),
    Text(TextData),
    ToolCall(AssistantToolCall),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantToolCall {
    id: ToolCallId,
    name: String,
    arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantStatus {
    Completed,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    id: EventId,
    turn_id: TurnId,
    parent_event_id: Option<EventId>,
    seq: u64,
    timestamp: DateTime<Utc>,
    #[serde(flatten)]
    event: Event,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ToolCallRequested(ToolCallRequested),
    ToolAuthorization(ToolAuthorization),
    ToolCallResult(ToolCallResult),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequested {
    tool_call_id: ToolCallId,
    tool_name: String,
    arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAuthorization {
    tool_call_id: ToolCallId,
    decision: ToolDecision,
    source: AuthorizationSource,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    tool_call_id: ToolCallId,
    status: ToolCallStatus,
    content: Vec<ToolData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolData {
    Text(TextData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextData {
    text: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(Uuid);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(String);
