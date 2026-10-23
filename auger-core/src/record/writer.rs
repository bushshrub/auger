//! The single writer of a session's records.

use crate::SessionEvent;
use crate::record::Record;
use crate::record::RecordBody;
use crate::record::RecordSink;
use crate::record::SinkError;
use crate::record::cursor::TurnsCursor;
use auger_driver::Turn;
use auger_toolengine::Journal;
use auger_toolengine::JournalError;
use chrono::Utc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use tracing::error;

struct Inner {
    sink: Box<dyn RecordSink>,
    next_seq: u64,
    events: Sender<SessionEvent>,
    cursor: TurnsCursor,
}

/// Numbers records, appends them to the sink, and mirrors them to the
/// session's event channel. Shared by the session loop and the tool engine's
/// journal, which may write from tool tasks.
#[derive(Clone)]
pub(crate) struct RecordWriter(Arc<Mutex<Inner>>);

impl RecordWriter {
    pub(crate) fn new(
        sink: Box<dyn RecordSink>,
        next_seq: u64,
        events: Sender<SessionEvent>,
        turns: &[Turn],
    ) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            sink,
            next_seq,
            events,
            cursor: TurnsCursor::new(turns),
        })))
    }

    pub(crate) fn append(&self, body: RecordBody) -> Result<(), SinkError> {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.append(body)
    }

    /// Record how the turns changed since the last call.
    pub(crate) fn sync_turns(&self, turns: &[Turn]) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for change in inner.cursor.advance(turns) {
            if let Err(error) = inner.append(RecordBody::Turns(change)) {
                error!(%error, "lost a turns record; the trace no longer restores this session");
            }
        }
    }

    /// Append without checking the result, for records only clients need to
    /// be told about promptly.
    pub(crate) fn note(&self, body: RecordBody) {
        if let Err(error) = self.append(body) {
            error!(%error, "failed to append a record");
        }
    }

    /// The tool engine's journal: each fact becomes a record.
    pub(crate) fn journal(&self) -> Journal {
        let writer = self.clone();
        Arc::new(move |fact| {
            writer
                .append(RecordBody::Tool(fact))
                .map_err(|error| JournalError(error.to_string()))
        })
    }
}

impl Inner {
    fn append(&mut self, body: RecordBody) -> Result<(), SinkError> {
        let record = Record {
            seq: self.next_seq,
            ts: Utc::now(),
            body,
        };
        self.sink.append(&record)?;
        self.next_seq += 1;
        let _ = self.events.send(SessionEvent::Record(record));
        Ok(())
    }
}
