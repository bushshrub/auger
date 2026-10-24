use auger_driver::ToolCallId;
use chrono::DateTime;
use chrono::Utc;
use getset::CopyGetters;
use getset::Getters;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

/// ID of an event in an auger session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(Uuid);

impl EventId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Into<Uuid> for EventId {
    fn into(self) -> Uuid {
        self.0
    }
}

/// A record of an event that occurred during an auger session.
/// Only events that the harness actually processed will be recorded.
#[derive(Serialize, Deserialize, Debug, Clone, CopyGetters, Getters)]
pub struct EventRecord {
    /// The logical parent of this event.
    #[getset(get_copy = "pub")]
    parent_id: Option<EventId>,
    /// Timestamp at which this event occurred.
    #[getset(get = "pub")]
    timestamp: DateTime<Utc>,
    /// Id of this event
    #[getset(get_copy = "pub")]
    event_id: EventId,
    /// The actual event itself
    #[getset(get = "pub")]
    event: RecordableEvent,
}

impl EventRecord {
    pub(super) fn new(
        parent_id: Option<EventId>,
        timestamp: DateTime<Utc>,
        event: RecordableEvent,
    ) -> Self {
        let event_id = EventId::new();
        Self {
            parent_id,
            timestamp,
            event_id,
            event,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum RecordableEvent {
    ToolAuthorization {
        tool_call_id: ToolCallId,
        decision: ToolDecision,
        source: AuthorizationSource,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDecision {
    Approved,
    Denied,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationSource {
    User,
    Policy,
}
