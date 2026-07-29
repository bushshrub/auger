//! Identifiers used across the session record and the harness.
//!
//! Turn and event ids are uuid v7, so they sort by the timestamp they were
//! minted from.

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use uuid::NoContext;
use uuid::Timestamp;
use uuid::Uuid;

/// ID of an auger session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for SessionId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

/// ID of a turn in an auger session. A turn is something like user/assistant
/// etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TurnId(Uuid);

impl TurnId {
    pub(crate) fn new(time: DateTime<Utc>) -> Self {
        Self(uuid_v7_from(time))
    }
}

impl Into<Uuid> for TurnId {
    fn into(self) -> Uuid {
        self.0
    }
}

/// ID of an event in an auger session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(Uuid);

impl EventId {
    pub(crate) fn new(time: DateTime<Utc>) -> Self {
        Self(uuid_v7_from(time))
    }
}

impl Into<Uuid> for EventId {
    fn into(self) -> Uuid {
        self.0
    }
}

fn uuid_v7_from(dt: DateTime<Utc>) -> Uuid {
    let secs = dt.timestamp() as u64;
    let nanos = dt.timestamp_subsec_nanos();
    Uuid::new_v7(Timestamp::from_unix(NoContext, secs, nanos))
}
