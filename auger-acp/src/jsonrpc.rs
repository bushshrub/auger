//! JSON-RPC 2.0 framing for ACP: one message per line on stdout.

use serde_json::{Value, json};
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

pub(crate) const PARSE_ERROR: i64 = -32700;
pub(crate) const INVALID_REQUEST: i64 = -32600;
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const INTERNAL_ERROR: i64 = -32603;
/// A prompt that failed inside the agent (the model stream errored).
pub(crate) const PROMPT_FAILED: i64 = -32000;

#[derive(Debug)]
pub(crate) struct RpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
}

impl RpcError {
    pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, message)
    }
}

/// A message read from the client.
pub(crate) enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// A reply to a request the agent sent. `result` is null for an error reply.
    Response {
        id: Value,
        result: Value,
    },
    Invalid {
        id: Value,
        error: RpcError,
    },
}

pub(crate) fn parse(line: &str) -> Incoming {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return Incoming::Invalid {
                id: Value::Null,
                error: RpcError::new(PARSE_ERROR, error.to_string()),
            };
        }
    };
    let id = value.get("id").cloned();
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        let method = method.to_string();
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        return match id {
            Some(id) => Incoming::Request { id, method, params },
            None => Incoming::Notification { method, params },
        };
    }
    match id {
        Some(id) if value.get("result").is_some() || value.get("error").is_some() => {
            Incoming::Response {
                id,
                result: value.get("result").cloned().unwrap_or(Value::Null),
            }
        }
        _ => Incoming::Invalid {
            id: Value::Null,
            error: RpcError::new(INVALID_REQUEST, "not a JSON-RPC message"),
        },
    }
}

pub(crate) fn result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub(crate) fn error(id: &Value, error: &RpcError) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": error.code, "message": error.message}})
}

pub(crate) fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub(crate) fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// The writing half of the connection. Logs go to stderr, never here.
pub(crate) struct Out {
    writer: Mutex<Box<dyn Write + Send>>,
    next_id: AtomicI64,
}

impl Out {
    pub(crate) fn new(writer: impl Write + Send + 'static) -> Self {
        Self {
            writer: Mutex::new(Box::new(writer)),
            next_id: AtomicI64::new(1),
        }
    }

    /// A fresh id for a request the agent sends to the client.
    pub(crate) fn request_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn send(&self, message: &Value) {
        let mut line = message.to_string();
        line.push('\n');
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(error) = writer.write_all(line.as_bytes()).and_then(|()| writer.flush()) {
            tracing::error!(%error, "cannot write to client");
        }
    }
}
