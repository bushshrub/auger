//! Session traces on disk: one JSONL file per session, plus the records of
//! each live session kept in memory for snapshots.

use auger_core::JsonlSink;
use auger_core::Record;
use auger_core::RecordBody;
use auger_core::RecordSink;
use auger_core::SessionHeader;
use auger_core::SessionId;
use auger_core::SinkError;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::BufReader;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::warn;

pub(crate) type RecordLog = Arc<Mutex<Vec<Record>>>;

/// Writes each record to the session's file, synced, then to its log.
pub(crate) struct ServerSink {
    file: Option<JsonlSink<File>>,
    log: RecordLog,
}

impl ServerSink {
    /// Append to `id`'s file in `dir`, creating it if needed. Without a file
    /// the session still runs, but only in memory.
    pub(crate) fn open(dir: Option<&Path>, id: SessionId, log: RecordLog) -> Self {
        let file = dir.and_then(|dir| {
            let path = trace_path(dir, id);
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .inspect_err(|error| warn!(%error, path = %path.display(), "cannot write trace"))
                .ok()
        });
        Self {
            file: file.map(JsonlSink::synced),
            log,
        }
    }
}

impl RecordSink for ServerSink {
    fn append(&mut self, record: &Record) -> Result<(), SinkError> {
        if let Some(file) = &mut self.file {
            file.append(record)?;
        }
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(record.clone());
        Ok(())
    }
}

/// `$AUGER_SESSIONS_DIR`, else `$XDG_DATA_HOME/auger/sessions`, else
/// `~/.local/share/auger/sessions`. Created if missing.
pub(crate) fn sessions_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("AUGER_SESSIONS_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(|d| PathBuf::from(d).join("auger/sessions")))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/auger/sessions"))
        })?;
    match std::fs::create_dir_all(&dir) {
        Ok(()) => Some(dir),
        Err(error) => {
            warn!(%error, dir = %dir.display(), "cannot create sessions dir; traces stay in memory");
            None
        }
    }
}

fn trace_path(dir: &Path, id: SessionId) -> PathBuf {
    dir.join(format!("{id}.jsonl"))
}

/// The traces in `dir` that start with a session header. Unreadable files
/// are skipped with a warning.
pub(crate) fn load_all(dir: &Path) -> Vec<(SessionHeader, Vec<Record>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut traces = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let records = File::open(&path)
            .map_err(|e| e.to_string())
            .and_then(|f| auger_core::read_jsonl(BufReader::new(f)).map_err(|e| e.to_string()));
        match records {
            Ok(records) => match records.first().map(|r| &r.body) {
                Some(RecordBody::Session(header)) => traces.push((header.clone(), records)),
                _ => warn!(path = %path.display(), "trace has no session header"),
            },
            Err(error) => warn!(%error, path = %path.display(), "cannot read trace"),
        }
    }
    traces
}

/// Records as NDJSON, the snapshot format.
pub(crate) fn ndjson(records: &[Record]) -> String {
    let mut out = String::new();
    for record in records {
        out.push_str(&serde_json::to_string(record).expect("records serialize"));
        out.push('\n');
    }
    out
}
