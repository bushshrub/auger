use crate::schema::{OwnedTraceRecord, SessionHeader, TraceRecord, TraceRecordRef};
use crate::session::history::{EventRecord, SessionData, TurnId, TurnRecord};
use crate::session::SessionRecord;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum TraceWriteError {
    #[error("failed to serialize trace record")]
    Serialize(#[from] serde_json::Error),
    #[error("failed to write trace record")]
    Io(#[from] std::io::Error),
}

pub struct TraceWriter<W> {
    writer: W,
}

impl<W: Write> TraceWriter<W> {
    pub fn new(writer: W, session: &SessionData) -> Result<Self, TraceWriteError> {
        let mut trace_writer = Self { writer };
        trace_writer.write_record(&TraceRecordRef::Session(SessionHeader::new(session.clone())))?;
        Ok(trace_writer)
    }

    pub fn write_turn(&mut self, turn: &TurnRecord) -> Result<(), TraceWriteError> {
        self.write_record(&TraceRecordRef::Turn { record: turn.data() })
    }

    pub fn write_event(&mut self, turn_id: TurnId, event: &EventRecord) -> Result<(), TraceWriteError> {
        self.write_record(&TraceRecordRef::Event { turn_id, record: event })
    }

    pub fn into_inner(self) -> W {
        self.writer
    }

    fn write_record(&mut self, record: &TraceRecordRef<'_>) -> Result<(), TraceWriteError> {
        serde_json::to_writer(&mut self.writer, record)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

pub struct TraceReader;

impl TraceReader {
    pub fn read(reader: impl BufRead) -> Result<SessionRecord, TraceReadError> {
        let mut header = None;
        let mut turns = Vec::new();
        let mut turn_indexes = HashMap::new();
        let mut event_ids = HashSet::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let record: OwnedTraceRecord = serde_json::from_str(&line)?;
            match record {
                TraceRecord::Session(record) => {
                    if header.replace(record).is_some() {
                        return Err(TraceRestoreError::DuplicateSessionHeader.into());
                    }
                    if !turns.is_empty() {
                        return Err(TraceRestoreError::MissingSessionHeader.into());
                    }
                }
                TraceRecord::Turn { record } => {
                    if header.is_none() {
                        return Err(TraceRestoreError::MissingSessionHeader.into());
                    }

                    let turn_id = record.turn_id();
                    let turn_uuid: Uuid = turn_id.into();
                    if turn_indexes.contains_key(&turn_id) {
                        return Err(TraceRestoreError::DuplicateTurn(turn_uuid).into());
                    }

                    let expected_parent = turns.last().map(|turn: &TurnRecord| turn.data().turn_id());
                    if record.parent_id() != expected_parent {
                        return Err(TraceRestoreError::InvalidTurnParent {
                            turn_id: turn_uuid,
                            expected_parent: expected_parent.map(Into::into),
                        }.into());
                    }

                    turn_indexes.insert(turn_id, turns.len());
                    turns.push(TurnRecord::from_parts(record, Vec::new()));
                }
                TraceRecord::Event { turn_id, record } => {
                    let event_id = record.event_id();
                    let event_uuid: Uuid = event_id.into();
                    if event_ids.contains(&event_id) {
                        return Err(TraceRestoreError::DuplicateEvent(event_uuid).into());
                    }

                    let Some(turn_index) = turn_indexes.get(&turn_id).copied() else {
                        return Err(TraceRestoreError::UnknownEventTurn {
                            event_id: event_uuid,
                            turn_id: turn_id.into(),
                        }.into());
                    };

                    if let Some(parent_event_id) = record.parent_id() {
                        if !event_ids.contains(&parent_event_id) {
                            return Err(TraceRestoreError::UnknownParentEvent {
                                event_id: event_uuid,
                                parent_event_id: parent_event_id.into(),
                            }.into());
                        }
                    }

                    event_ids.insert(event_id);
                    turns[turn_index].restore_event(record);
                }
            }
        }

        let header = header.ok_or(TraceRestoreError::Empty)?;
        if *header.version() != 1 {
            return Err(TraceRestoreError::UnsupportedVersion(*header.version()).into());
        }
        let data = header.data();
        Ok(SessionRecord::from_trace_parts(
            data.session_id(),
            data.created_at(),
            data.cwd().clone(),
            data.model_info().clone(),
            turns,
        ))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TraceReadError {
    #[error("failed to read trace record")]
    Io(#[from] std::io::Error),
    #[error("failed to deserialize trace record")]
    Deserialize(#[from] serde_json::Error),
    #[error(transparent)]
    Restore(#[from] TraceRestoreError),
}

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
    #[error("turn {turn_id} does not follow parent {expected_parent:?}")]
    InvalidTurnParent { turn_id: Uuid, expected_parent: Option<Uuid> },
    #[error("duplicate event {0}")]
    DuplicateEvent(Uuid),
    #[error("event {event_id} refers to unknown turn {turn_id}")]
    UnknownEventTurn { event_id: Uuid, turn_id: Uuid },
    #[error("event {event_id} refers to unknown parent event {parent_event_id}")]
    UnknownParentEvent { event_id: Uuid, parent_event_id: Uuid },
}
