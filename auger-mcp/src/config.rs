use serde::Deserialize;
use std::collections::BTreeMap;

/// One MCP server, from `[mcp.servers.<name>]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Tools of this server that run without asking. `"*"` allows all of them.
    #[serde(default)]
    pub allow: Vec<String>,
}

impl ServerConfig {
    pub(crate) fn allows(&self, tool: &str) -> bool {
        self.allow.iter().any(|allowed| allowed == "*" || allowed == tool)
    }
}
