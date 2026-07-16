use crate::{EventRecord, SessionHeader, SessionRecord};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum TraceFileError {
    #[error("XDG_CONFIG_HOME and HOME are both unset")]
    ConfigHomeUnavailable,
    #[error("trace file is empty: {0}")]
    EmptyTrace(PathBuf),
    #[error("invalid trace file: {0}")]
    InvalidTrace(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Return the standard on-disk location for a session trace.
///
/// Uses XDG_CONFIG_HOME when available and ~/.auger otherwise, so traces are
/// stable across session restarts without requiring callers to manage paths.
pub fn session_trace_path(session_id: Uuid) -> Result<PathBuf, TraceFileError> {
    let sessions_dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path).join("auger/sessions"),
        _ => std::env::var_os("HOME")
            .map(|path| PathBuf::from(path).join(".auger/sessions"))
            .ok_or(TraceFileError::ConfigHomeUnavailable)?,
    };
    Ok(sessions_dir
        .join(session_id.to_string())
        .join(format!("{session_id}.jsonl")))
}

/// Appends a SessionRecord to its JSONL file as the session progresses.
pub struct TraceWriter {
    path: PathBuf,
    file: File,
}

impl TraceWriter {
    /// Open the standard trace file and write its header when it is new.
    pub fn open(record: &SessionRecord) -> Result<Self, TraceFileError> {
        let path = session_trace_path(record.header().session_id())?;
        Self::open_at(path, record)
    }

    /// Open a trace file at an explicit path.
    ///
    /// This supports callers that need to place a trace outside the standard
    /// session directory, such as isolated integration tests.
    pub fn open_at(path: impl Into<PathBuf>, record: &SessionRecord) -> Result<Self, TraceFileError> {
        let path = path.into();
        let parent = path.parent().expect("trace path has a parent");
        fs::create_dir_all(parent)?;
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        if file.metadata()?.len() == 0 {
            write_json_line(&mut file, record.header())?;
        }
        Ok(Self { path, file })
    }

    /// Persist an event immediately after it is appended to the in-memory trace.
    pub fn append(&mut self, event: &EventRecord) -> Result<(), TraceFileError> {
        write_json_line(&mut self.file, event)
    }

    /// Return the file receiving appended JSONL records.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Reads a complete JSONL trace back into its in-memory representation.
pub struct TraceReader;

impl TraceReader {
    /// Read the session header and ordered events from a JSONL trace file.
    pub fn read(path: impl AsRef<Path>) -> Result<SessionRecord, TraceFileError> {
        let path = path.as_ref();
        let file = File::open(path)?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .ok_or_else(|| TraceFileError::EmptyTrace(path.to_owned()))??;
        let header: SessionHeader = serde_json::from_str(&header)?;
        let mut events = Vec::new();
        for line in lines {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            events.push(serde_json::from_str(&line)?);
        }
        if header.record_type != "session" {
            return Err(TraceFileError::InvalidTrace(path.to_owned()));
        }
        Ok(SessionRecord { header, events })
    }
}

fn write_json_line<T: serde::Serialize>(file: &mut File, value: &T) -> Result<(), TraceFileError> {
    // Sync each append so a persisted trace remains usable after interruption.
    serde_json::to_writer(&mut *file, value)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Event, ModelInfo, ProviderType};

    #[test]
    fn writes_and_reads_jsonl_trace() {
        let path = std::env::temp_dir().join(format!("{}.jsonl", Uuid::new_v4()));
        let mut record = SessionRecord::new(
            Uuid::new_v4(),
            PathBuf::from("/repo"),
            ModelInfo::new(ProviderType::Unknown),
        );
        let mut writer = TraceWriter::open_at(&path, &record).unwrap();
        record.append_event(Event::InputMessage {
            content: Vec::new(),
        });
        writer.append(record.events().last().unwrap()).unwrap();

        let restored = TraceReader::read(&path).unwrap();
        assert_eq!(restored.events().len(), 1);
        assert_eq!(restored.header().session_id(), record.header().session_id());
    }
}
