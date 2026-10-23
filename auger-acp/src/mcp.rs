//! MCP servers for a session: the configured ones plus those the client
//! passes in `session/new` or `session/load`.

use auger_mcp::ServerConfig;
use serde_json::Value;
use std::collections::BTreeMap;
use tracing::warn;

/// `configured` with the client's servers added. A client server replaces a
/// configured one of the same name. Client servers never get an allow list,
/// so their tools always ask. Only stdio servers are supported; others are
/// skipped with a warning.
pub(crate) fn servers(
    configured: &BTreeMap<String, ServerConfig>,
    params: &Value,
) -> BTreeMap<String, ServerConfig> {
    let mut servers = configured.clone();
    let entries = params.get("mcpServers").and_then(Value::as_array);
    for entry in entries.into_iter().flatten() {
        match stdio_server(entry) {
            Some((name, config)) => {
                servers.insert(name, config);
            }
            None => warn!(server = %entry, "skipping MCP server: only stdio servers are supported"),
        }
    }
    servers
}

/// A stdio entry has no `type`; http and sse entries do.
fn stdio_server(entry: &Value) -> Option<(String, ServerConfig)> {
    if entry.get("type").is_some() {
        return None;
    }
    let name = entry.get("name")?.as_str()?.to_string();
    let command = entry.get("command")?.as_str()?.to_string();
    let args = strings(entry.get("args"));
    let env = entry
        .get("env")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|var| {
            Some((
                var.get("name")?.as_str()?.to_string(),
                var.get("value")?.as_str()?.to_string(),
            ))
        })
        .collect();
    Some((
        name,
        ServerConfig {
            command,
            args,
            env,
            allow: Vec::new(),
        },
    ))
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn configured() -> BTreeMap<String, ServerConfig> {
        let mut servers = BTreeMap::new();
        servers.insert(
            "docs".to_string(),
            ServerConfig {
                command: "docs-server".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                allow: vec!["*".to_string()],
            },
        );
        servers
    }

    #[test]
    fn client_stdio_servers_join_the_configured_ones_and_always_ask() {
        let params = json!({"mcpServers": [
            {"name": "fs", "command": "fs-server", "args": ["--root", "/w"],
             "env": [{"name": "TOKEN", "value": "t"}]},
            {"type": "http", "name": "remote", "url": "https://x", "headers": []},
        ]});
        let servers = servers(&configured(), &params);
        assert_eq!(servers.keys().collect::<Vec<_>>(), vec!["docs", "fs"]);
        let fs = &servers["fs"];
        assert_eq!(fs.args, vec!["--root", "/w"]);
        assert_eq!(fs.env.get("TOKEN").map(String::as_str), Some("t"));
        assert!(fs.allow.is_empty());
    }

    #[test]
    fn a_client_server_replaces_a_configured_one_of_the_same_name() {
        let params = json!({"mcpServers": [{"name": "docs", "command": "other", "args": [], "env": []}]});
        let servers = servers(&configured(), &params);
        assert_eq!(servers["docs"].command, "other");
        assert!(servers["docs"].allow.is_empty());
    }
}
