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
    #[serde(flatten)]
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

    pub fn id(&self) -> Uuid {
        self.id
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContent {
    Text {
        text: String,
    },
    ToolResult {
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
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantStatus {
    Completed,
    Interrupted,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_a_flat_tagged_tool_result() {
        let record = EventRecord::new(
            None,
            1,
            Event::ToolCallResult {
                tool_call_id: "call_123".to_owned(),
                status: ToolCallStatus::Success,
                content: vec![ToolData::Text {
                    text: "test passed".to_owned(),
                }],
            },
        );

        let value = serde_json::to_value(&record).unwrap();

        assert_eq!(value["type"], "tool_call_result");
        assert_eq!(value["seq"], 1);
        assert_eq!(
            value["content"],
            json!([{"type": "text", "text": "test passed"}])
        );
        assert!(value.get("event").is_none());

        let restored: EventRecord = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), value);
    }
}
