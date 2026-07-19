use std::path::PathBuf;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::{SessionEvent, SessionId};
use getset::{Getters};
use provider::UserPrompt;

/// A record of an auger session
#[derive(Serialize, Deserialize, Debug, Clone, Getters)]
pub struct SessionRecord {
    session_id: SessionId,
    created_at: DateTime<Utc>,
    cwd: PathBuf,
    #[getset(get = "pub")]
    events: Vec<EventRecord>,
}

impl SessionRecord {
    /// Initialize a new session record. This should be called
    /// at the start of the session.
    pub(crate) fn new(session_id: SessionId, cwd: PathBuf) -> Self {
        let created_at = Utc::now();
        let events = Vec::new();
        Self {
            session_id,
            created_at,
            cwd,
            events,
        }
    }

    /// Record an event in the session. The event should have a parent ID.
    pub(crate) fn record_event(&mut self, event: RecordableEvent, parent_event_id: Option<EventId>) -> EventId {
        let event_id = EventId::new(Utc::now());
        let event_record = EventRecord {
            parent_id: parent_event_id,
            timestamp: Utc::now(),
            event_id,
            event
        };
        self.events.push(event_record);
        event_id
    }
}

/// ID of an event in an auger session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(Uuid);

impl EventId {
    pub(crate) fn new(time: DateTime<Utc>) -> Self {
        Self(Uuid::new_v7(time.into()))
    }
}

/// A record of an event that occurred during an auger session.
/// Only events that the harness actually processed will be recorded.
#[derive(Serialize, Deserialize, Debug, Clone, Getters)]
pub struct EventRecord {
    /// The logical parent of this event.
    parent_id: Option<EventId>,
    /// Timestamp at which this event occurred.
    timestamp: DateTime<Utc>,
    /// Id of this event
    event_id: EventId,
    /// The actual event itself
    event: RecordableEvent,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCallId(String);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolData {
    Text {
        text: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolDecision {
    Approved,
    Denied,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolCallStatus {
    /// The tool call executed successfully and returned a result
    Success,
    /// User denied the tool call
    Denied,
    /// The tool was executed, but the tool itself returned a failure.
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AuthorizationSource {
    User,
    Policy,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum InputContent {
    Text {
        text: String,
    },
    ToolResult {
        tool_call_id: ToolCallId,
        content: Vec<ToolData>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RecordableEvent {
    InputMessage {
        content: Vec<InputContent>
    },
    /// Result emitted by the clanker.
    AssistantMessage {
        status: AssistantStatus,
        content: Vec<AssistantContent>,
    },
    ToolAuthorization {
        tool_call_id: ToolCallId,
        decision: ToolDecision,
        source: AuthorizationSource,
        reason: Option<String>,
    },
    ToolCallResult {
        tool_call_id: ToolCallId,
        status: ToolCallStatus,
        content: Vec<ToolData>
    }
}

impl RecordableEvent {
    pub(crate) fn user_prompt(prompt: UserPrompt) -> Self {
        Self::InputMessage { content: vec![InputContent::Text { text: prompt.into() }] }
    }

    pub(crate) fn tool_decision(call_id: String, approved: bool, source: AuthorizationSource, reason: Option<String>) -> Self {
        Self::ToolAuthorization {
            tool_call_id: ToolCallId(call_id),
            decision: if approved { ToolDecision::Approved } else { ToolDecision::Denied },
            source,
            reason,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssistantContent {
    Reasoning {
        text: String,
    },
    Text {
        text: String,
    },
    ToolCall {
        id: ToolCallId,
        name: String,
        arguments: serde_json::Value,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssistantStatus {
    Completed,
    Interrupted,
    Failed,
}
