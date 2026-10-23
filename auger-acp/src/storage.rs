//! Session traces in the sessions directory: one JSONL file per session.

use crate::jsonrpc::RpcError;
use auger_core::{JsonlSink, Record, RecordBody, SessionBuilder, read_jsonl};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, ErrorKind};
use std::path::{Path, PathBuf};
use tracing::warn;
use uuid::Uuid;

/// The canonical form of the `sessionId` in `params`.
pub(crate) fn session_key(params: &Value) -> Result<String, RpcError> {
    let raw = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid("sessionId is required"))?;
    Uuid::parse_str(raw)
        .map(|uuid| uuid.to_string())
        .map_err(|_| RpcError::invalid("sessionId is not a session id"))
}

/// Append the session's records to its file in `dir`. Without a directory
/// the session runs in memory only.
pub(crate) fn attach_sink(builder: SessionBuilder, dir: Option<&Path>) -> SessionBuilder {
    let Some(dir) = dir else { return builder };
    let path = dir.join(format!("{}.jsonl", builder.id()));
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => builder.sink(JsonlSink::synced(file)),
        Err(error) => {
            warn!(%error, path = %path.display(), "cannot write trace; session is not stored");
            builder
        }
    }
}

pub(crate) fn read(dir: &Path, key: &str) -> Result<Vec<Record>, RpcError> {
    let path: PathBuf = dir.join(format!("{key}.jsonl"));
    let file = File::open(&path).map_err(|error| match error.kind() {
        ErrorKind::NotFound => RpcError::invalid(format!("unknown session {key}")),
        _ => RpcError::internal(format!("cannot open {}: {error}", path.display())),
    })?;
    read_jsonl(BufReader::new(file)).map_err(|error| RpcError::internal(error.to_string()))
}

pub(crate) fn header_cwd(records: &[Record]) -> Result<PathBuf, RpcError> {
    match records.first().map(|record| &record.body) {
        Some(RecordBody::Session(header)) => Ok(header.data.cwd().clone()),
        _ => Err(RpcError::internal("trace has no session header")),
    }
}
