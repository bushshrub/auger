//! Minimal JSON-RPC 2.0 client over a child process's stdin and stdout.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

const EXITED: &str = "server has exited";

type Waiting = HashMap<u64, oneshot::Sender<Result<Value, String>>>;

pub(crate) struct Rpc {
    stdin: AsyncMutex<ChildStdin>,
    /// Requests awaiting a response. `None` once the server's output has ended.
    waiting: Arc<Mutex<Option<Waiting>>>,
    next_id: AtomicU64,
}

impl Rpc {
    pub(crate) fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        let waiting = Arc::new(Mutex::new(Some(HashMap::new())));
        tokio::spawn(read_responses(stdout, waiting.clone()));
        Self {
            stdin: AsyncMutex::new(stdin),
            waiting,
            next_id: AtomicU64::new(1),
        }
    }

    /// Send a request and wait for its result. Fails if the server exits first.
    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match lock(&self.waiting).as_mut() {
            Some(waiting) => {
                waiting.insert(id, tx);
            }
            None => return Err(EXITED.into()),
        }
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(error) = self.send(&message).await {
            if let Some(waiting) = lock(&self.waiting).as_mut() {
                waiting.remove(&id);
            }
            return Err(error);
        }
        rx.await.unwrap_or_else(|_| Err(EXITED.into()))
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn send(&self, message: &Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("cannot write to server: {error}"))?;
        stdin
            .flush()
            .await
            .map_err(|error| format!("cannot write to server: {error}"))
    }
}

async fn read_responses(stdout: ChildStdout, waiting: Arc<Mutex<Option<Waiting>>>) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            tracing::debug!("ignoring non-JSON line from MCP server");
            continue;
        };
        // Notifications and requests from the server are not used.
        if message.get("method").is_some() {
            continue;
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let result = match (message.get("result"), message.get("error")) {
            (Some(result), _) => Ok(result.clone()),
            (None, Some(error)) => Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("JSON-RPC error")
                .to_string()),
            (None, None) => Err("response has neither result nor error".into()),
        };
        let sender = lock(&waiting).as_mut().and_then(|map| map.remove(&id));
        if let Some(sender) = sender {
            let _ = sender.send(result);
        }
    }
    // Output ended: dropping the senders fails every pending and later request.
    let pending = lock(&waiting).take();
    drop(pending);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
