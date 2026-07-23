use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum TraceRestoreError {
    #[error("trace is empty")]
    Empty,
    #[error("first record must be a session header")]
    MissingSessionHeader,
    #[error("unsupported trace version {0}")]
    UnsupportedVersion(u32),
    #[error("trace contains more than one session header")]
    DuplicateSessionHeader,
    #[error("duplicate turn {0}")]
    DuplicateTurn(Uuid),
    #[error("turn {turn_id} does not follow parent {expected_parent}")]
    InvalidTurnParent { turn_id: Uuid, expected_parent: Uuid },
    #[error("duplicate event {0}")]
    DuplicateEvent(Uuid),
    #[error("event {event_id} refers to unknown turn {turn_id}")]
    UnknownEventTurn { event_id: Uuid, turn_id: Uuid },
    #[error("event {event_id} refers to unknown parent event {parent_event_id}")]
    UnknownParentEvent { event_id: Uuid, parent_event_id: Uuid },
}

