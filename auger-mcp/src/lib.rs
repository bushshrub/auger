//! MCP servers as auger tools.
//!
//! Each configured server runs as a child process for one session. Its tools
//! are offered to the model as `mcp__<server>__<tool>`.

mod config;
mod policy;
mod rpc;
mod server;
mod tool;

pub use config::ServerConfig;
pub use policy::McpAllow;

use auger_toolengine::Tool;
use server::Server;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tool::McpTool;

/// The tools of every server that started, and the policy for them.
#[derive(Default)]
pub struct McpServers {
    tools: Vec<Box<dyn Tool>>,
    allowed: HashSet<String>,
}

impl McpServers {
    /// Start each server in `cwd`. A server that fails is logged and left
    /// out; the others still count.
    pub async fn start(servers: &BTreeMap<String, ServerConfig>, cwd: &Path) -> Self {
        let starts = servers.iter().map(|(name, spec)| async move {
            (name, spec, Server::start(name, spec, cwd).await)
        });
        let mut out = Self::default();
        for (name, spec, started) in futures::future::join_all(starts).await {
            match started {
                Ok((server, tools)) => out.add(name, spec, Arc::new(server), tools),
                Err(error) => {
                    tracing::warn!(server = %name, %error, "MCP server unavailable; its tools are not offered")
                }
            }
        }
        out
    }

    fn add(
        &mut self,
        name: &str,
        spec: &ServerConfig,
        server: Arc<Server>,
        tools: Vec<server::RemoteTool>,
    ) {
        for remote in tools {
            let full = format!("mcp__{name}__{}", remote.name);
            if !valid_name(&full) || self.tools.iter().any(|t| t.details().name == full) {
                tracing::warn!(tool = %full, "skipping MCP tool with an unusable or duplicate name");
                continue;
            }
            if spec.allows(&remote.name) {
                self.allowed.insert(full.clone());
            }
            self.tools.push(Box::new(McpTool {
                server: server.clone(),
                remote_name: remote.name,
                name: full,
                description: remote.description.unwrap_or_default(),
                schema: remote.input_schema,
            }));
        }
    }

    /// The tools to register with the session, and the policy node that
    /// allows the configured ones. The tools keep their servers alive; the
    /// servers are killed when the last tool is dropped.
    pub fn into_parts(self) -> (Vec<Box<dyn Tool>>, McpAllow) {
        (self.tools, McpAllow::new(self.allowed))
    }
}

/// Names must fit what model APIs accept for tool names.
fn valid_name(name: &str) -> bool {
    name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
