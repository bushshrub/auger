//! One running MCP server process and the requests made to it.

use crate::config::ServerConfig;
use crate::rpc::Rpc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const PROTOCOL_VERSION: &str = "2025-06-18";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// A tool as the server describes it.
#[derive(Debug, Deserialize)]
pub(crate) struct RemoteTool {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub(crate) input_schema: Value,
}

/// The process is killed when the server is dropped.
pub(crate) struct Server {
    pub(crate) name: String,
    rpc: Rpc,
    _child: Child,
}

impl Server {
    /// Start the process in `cwd`, initialize it and list its tools.
    pub(crate) async fn start(
        name: &str,
        spec: &ServerConfig,
        cwd: &Path,
    ) -> Result<(Self, Vec<RemoteTool>), String> {
        let mut child = Command::new(&spec.command)
            .args(&spec.args)
            .envs(&spec.env)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("cannot start `{}`: {error}", spec.command))?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("cannot open the server's pipes".into());
        };
        let log_name = name.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(server = %log_name, "{line}");
            }
        });
        let server = Self {
            name: name.to_string(),
            rpc: Rpc::new(stdin, stdout),
            _child: child,
        };
        let tools = with_timeout(STARTUP_TIMEOUT, async {
            server.initialize().await?;
            server.list_tools().await
        })
        .await?;
        Ok((server, tools))
    }

    async fn initialize(&self) -> Result<(), String> {
        self.rpc
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "auger", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
            .await?;
        self.rpc.notify("notifications/initialized", json!({})).await
    }

    /// Follows `nextCursor` until the server has no more pages.
    async fn list_tools(&self) -> Result<Vec<RemoteTool>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let page = self.rpc.request("tools/list", params).await?;
            let items = page.get("tools").cloned().unwrap_or(json!([]));
            let items: Vec<RemoteTool> = serde_json::from_value(items)
                .map_err(|error| format!("bad tools/list result: {error}"))?;
            tools.extend(items);
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }

    /// The raw `tools/call` result: `content` and `isError`.
    pub(crate) async fn call_tool(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        let params = json!({ "name": tool, "arguments": arguments });
        with_timeout(CALL_TIMEOUT, self.rpc.request("tools/call", params)).await
    }
}

async fn with_timeout<T>(
    limit: Duration,
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(limit, future)
        .await
        .unwrap_or_else(|_| Err(format!("timed out after {}s", limit.as_secs())))
}
