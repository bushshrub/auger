use crate::{Event, EventRecord, ModelInfo};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionRecord {
    pub(crate) header: SessionHeader,
    pub(crate) events: Vec<EventRecord>,
}

impl SessionRecord {
    /// Create an empty in-memory trace for a session.
    pub fn new(session_id: Uuid, cwd: PathBuf, model: ModelInfo) -> Self {
        Self {
            header: SessionHeader::new(session_id, cwd, model),
            events: Vec::new(),
        }
    }

    /// Add an event with an explicit logical parent.
    pub fn add_event(&mut self, parent_id: Option<Uuid>, event: Event) -> Uuid {
        let record = EventRecord::new(parent_id, self.events.len() as u64 + 1, event);
        let id = record.id();
        self.events.push(record);
        id
    }

    /// Add an event after the most recently appended event for linear replay.
    pub fn append_event(&mut self, event: Event) -> Uuid {
        self.add_event(self.events.last().map(EventRecord::id), event)
    }

    /// Return the session metadata written as the first JSONL record.
    pub fn header(&self) -> &SessionHeader {
        &self.header
    }

    /// Return events in their physical append order.
    pub fn events(&self) -> &[EventRecord] {
        &self.events
    }

}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionHeader {
    #[serde(rename = "type", default = "session_record_type")]
    pub(crate) record_type: String,
    version: u32,
    session_id: Uuid,
    created_at: DateTime<Utc>,
    cwd: PathBuf,
    model: ModelInfo,
}

impl SessionHeader {
    pub(crate) fn new(session_id: Uuid, cwd: PathBuf, model: ModelInfo) -> Self {
        Self {
            record_type: session_record_type(),
            version: 1,
            session_id,
            created_at: Utc::now(),
            cwd,
            model,
        }
    }

    /// Return the ID shared by the trace header and its storage path.
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

}

fn session_record_type() -> String {
    "session".to_owned()
}
