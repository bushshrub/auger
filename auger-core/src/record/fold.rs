//! Folds a trace back into the state a session resumes from.

use crate::record::Record;
use crate::record::RecordBody;
use crate::record::SessionHeader;
use crate::record::StreamRecord;
use crate::record::TurnsChange;
use crate::record::cursor;
use auger_driver::Turn;
use auger_toolengine::ToolFact;
use provider::AssistantResponse;
use std::path::PathBuf;

pub(crate) const TRACE_VERSION: u32 = 2;

/// What a session resumes from.
#[derive(Debug)]
pub(crate) struct Restored {
    pub(crate) header: SessionHeader,
    pub(crate) turns: Vec<Turn>,
    /// The partial response of an interrupted or failed stream that was not
    /// resolved yet.
    pub(crate) partial: Option<AssistantResponse>,
    /// Tool facts since the last output turn, i.e. for its batch.
    pub(crate) batch_facts: Vec<ToolFact>,
    pub(crate) cwd: PathBuf,
    pub(crate) next_seq: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("trace is empty")]
    Empty,
    #[error("first record must be the session header")]
    MissingHeader,
    #[error("unsupported trace version {0}")]
    UnsupportedVersion(u32),
    #[error("record {found} found where {expected} was expected")]
    Gap { expected: u64, found: u64 },
    #[error("record {seq} is a second session header")]
    DuplicateHeader { seq: u64 },
    #[error("record {seq} does not apply to the turns before it")]
    BadChange { seq: u64 },
    #[error(transparent)]
    Driver(#[from] auger_driver::RestoreError),
}

pub(crate) fn fold(records: impl IntoIterator<Item = Record>) -> Result<Restored, RestoreError> {
    let mut records = records.into_iter();
    let first = records.next().ok_or(RestoreError::Empty)?;
    let header = match first.body {
        RecordBody::Session(header) if first.seq == 0 => header,
        _ => return Err(RestoreError::MissingHeader),
    };
    if header.version != TRACE_VERSION {
        return Err(RestoreError::UnsupportedVersion(header.version));
    }
    let mut restored = Restored {
        cwd: header.data.cwd().clone(),
        header,
        turns: Vec::new(),
        partial: None,
        batch_facts: Vec::new(),
        next_seq: 1,
    };
    for record in records {
        if record.seq != restored.next_seq {
            return Err(RestoreError::Gap {
                expected: restored.next_seq,
                found: record.seq,
            });
        }
        restored.next_seq += 1;
        match record.body {
            RecordBody::Session(_) => {
                return Err(RestoreError::DuplicateHeader { seq: record.seq });
            }
            RecordBody::Turns(change) => {
                if matches!(
                    change,
                    TurnsChange::PushTurn(Turn::Output(_)) | TurnsChange::Truncate { .. }
                ) {
                    restored.batch_facts.clear();
                }
                cursor::apply(&mut restored.turns, change)
                    .map_err(|()| RestoreError::BadChange { seq: record.seq })?;
                restored.partial = None;
            }
            RecordBody::Stream(stream) => match stream {
                StreamRecord::Started | StreamRecord::Completed { .. } => restored.partial = None,
                StreamRecord::Interrupted { partial } | StreamRecord::Failed { partial, .. } => {
                    restored.partial = partial
                }
            },
            RecordBody::Tool(fact) => {
                if let ToolFact::CwdChanged { cwd } = &fact {
                    restored.cwd = cwd.clone();
                }
                restored.batch_facts.push(fact);
            }
        }
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::ModelInfo;
    use crate::record::SessionData;
    use crate::record::SessionId;
    use auger_driver::InputEntry;
    use chrono::Utc;
    use provider::UserPrompt;

    fn record(seq: u64, body: RecordBody) -> Record {
        Record {
            seq,
            ts: Utc::now(),
            body,
        }
    }

    fn header() -> RecordBody {
        RecordBody::Session(SessionHeader {
            version: TRACE_VERSION,
            data: SessionData::new(
                SessionId::new(),
                Utc::now(),
                PathBuf::from("/w"),
                ModelInfo::new("p".to_string(), "m".to_string()),
            ),
            system_prompt: "sys".to_string(),
        })
    }

    fn prompt(text: &str) -> RecordBody {
        RecordBody::Turns(TurnsChange::PushTurn(Turn::Input {
            entries: vec![InputEntry::User(UserPrompt::new(text.to_string()))],
        }))
    }

    #[test]
    fn rejects_a_gap_in_seq() {
        let records = vec![record(0, header()), record(2, prompt("a"))];
        assert!(matches!(
            fold(records),
            Err(RestoreError::Gap {
                expected: 1,
                found: 2
            })
        ));
    }

    #[test]
    fn rejects_a_change_that_does_not_apply() {
        let entry = InputEntry::User(UserPrompt::new("a".to_string()));
        let records = vec![
            record(0, header()),
            record(1, RecordBody::Turns(TurnsChange::PushEntry(entry))),
        ];
        assert!(matches!(fold(records), Err(RestoreError::BadChange { seq: 1 })));
    }

    #[test]
    fn a_started_stream_clears_the_interrupted_partial() {
        let partial = provider::AssistantResponse::new(vec![provider::Block::Text(
            "half".to_string(),
        )]);
        let interrupted = RecordBody::Stream(StreamRecord::Interrupted { partial });
        let records = vec![record(0, header()), record(1, prompt("a")), record(2, interrupted)];
        assert!(fold(records.clone()).unwrap().partial.is_some());

        let mut retried = records;
        retried.push(record(3, RecordBody::Stream(StreamRecord::Started)));
        assert!(fold(retried).unwrap().partial.is_none());
    }

    #[test]
    fn cwd_follows_the_last_move() {
        let moved = RecordBody::Tool(ToolFact::CwdChanged {
            cwd: PathBuf::from("/w/sub"),
        });
        let records = vec![record(0, header()), record(1, moved)];
        assert_eq!(fold(records).unwrap().cwd, PathBuf::from("/w/sub"));
    }
}
