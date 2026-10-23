//! Records as JSON lines: the common way to store a trace.

use crate::record::Record;
use crate::record::RecordSink;
use crate::record::SinkError;
use std::fs::File;
use std::io::BufRead;
use std::io::Write;

/// Appends one JSON line per record and flushes it. With a [`File`],
/// [`JsonlSink::synced`] also syncs to disk, which crash-safe tool execution
/// needs.
pub struct JsonlSink<W> {
    writer: W,
    sync: fn(&mut W) -> std::io::Result<()>,
}

impl<W: Write + Send> JsonlSink<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            sync: |_| Ok(()),
        }
    }
}

impl JsonlSink<File> {
    pub fn synced(file: File) -> Self {
        Self {
            writer: file,
            sync: |file| file.sync_data(),
        }
    }
}

impl<W: Write + Send> RecordSink for JsonlSink<W> {
    fn append(&mut self, record: &Record) -> Result<(), SinkError> {
        let mut line = serde_json::to_vec(record).map_err(|e| SinkError(e.to_string()))?;
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .and_then(|()| self.writer.flush())
            .and_then(|()| (self.sync)(&mut self.writer))
            .map_err(|e| SinkError(e.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("failed to read trace")]
    Io(#[from] std::io::Error),
    #[error("line {line} is not a record")]
    Corrupt {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
}

/// Read records written by [`JsonlSink`]. A torn last line, left by a crash
/// mid-write, is dropped; a bad line anywhere else is an error.
pub fn read_jsonl(reader: impl BufRead) -> Result<Vec<Record>, ReadError> {
    let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;
    let mut records = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(record) => records.push(record),
            Err(_) if index + 1 == lines.len() => break,
            Err(source) => {
                return Err(ReadError::Corrupt {
                    line: index + 1,
                    source,
                });
            }
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::RecordBody;
    use crate::record::StreamRecord;
    use chrono::Utc;

    fn started(seq: u64) -> Record {
        Record {
            seq,
            ts: Utc::now(),
            body: RecordBody::Stream(StreamRecord::Started),
        }
    }

    fn written(records: &[Record]) -> Vec<u8> {
        let mut sink = JsonlSink::new(Vec::new());
        for record in records {
            sink.append(record).unwrap();
        }
        sink.writer
    }

    #[test]
    fn drops_a_torn_last_line() {
        let mut bytes = written(&[started(0), started(1)]);
        bytes.truncate(bytes.len() - 5);
        let records = read_jsonl(bytes.as_slice()).unwrap();
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn rejects_a_bad_line_before_the_end() {
        let mut bytes = b"{not json\n".to_vec();
        bytes.extend(written(&[started(0)]));
        assert!(matches!(
            read_jsonl(bytes.as_slice()),
            Err(ReadError::Corrupt { line: 1, .. })
        ));
    }
}
