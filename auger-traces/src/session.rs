use crate::{Event, EventRecord, ModelInfo};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionRecord {
    header: SessionHeader,
    events: Vec<EventRecord>,
}

impl SessionRecord {
    pub fn new(session_id: Uuid, cwd: PathBuf, model: ModelInfo) -> Self {
        Self {
            header: SessionHeader::new(session_id, cwd, model),
            events: Vec::new(),
        }
    }

    pub fn add_event(&mut self, parent_id: Option<Uuid>, event: Event) -> Uuid {
        let record = EventRecord::new(parent_id, self.events.len() as u64 + 1, event);
        let id = record.id();
        self.events.push(record);
        id
    }
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
            model,
        }
    }
}
