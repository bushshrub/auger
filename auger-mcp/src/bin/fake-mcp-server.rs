//! A tiny stdio MCP server for tests. Not built for users.

use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = message.get("id").cloned() else { continue };
        let params = message.get("params").cloned().unwrap_or(json!({}));
        let result = match message["method"].as_str().unwrap_or("") {
            "initialize" => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fake", "version": "0.0.0" },
            }),
            "tools/list" if params.get("cursor").is_none() => json!({
                "tools": [
                    tool("echo", "Echoes its message.", json!({"message": {"type": "string"}})),
                    tool("fail", "Always reports an error.", json!({})),
                    tool("crash", "Exits the process.", json!({})),
                    tool("env", "Returns FAKE_MCP_VALUE.", json!({})),
                ],
                "nextCursor": "page2",
            }),
            "tools/list" => json!({
                "tools": [tool("image", "Returns an image.", json!({}))],
            }),
            "tools/call" => call(&params),
            _ => {
                let error = json!({ "code": -32601, "message": "method not found" });
                println!("{}", json!({ "jsonrpc": "2.0", "id": id, "error": error }));
                continue;
            }
        };
        println!("{}", json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        std::io::stdout().flush().ok();
    }
}

fn tool(name: &str, description: &str, properties: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties },
    })
}

fn call(params: &Value) -> Value {
    let arguments = &params["arguments"];
    match params["name"].as_str().unwrap_or("") {
        "echo" => text(&format!("echo: {}", arguments["message"].as_str().unwrap_or(""))),
        "fail" => json!({ "content": [{ "type": "text", "text": "boom" }], "isError": true }),
        "crash" => std::process::exit(3),
        "env" => text(&std::env::var("FAKE_MCP_VALUE").unwrap_or_default()),
        "image" => json!({
            "content": [
                { "type": "text", "text": "look" },
                { "type": "image", "data": "AAAA", "mimeType": "image/png" },
            ],
        }),
        other => json!({ "content": [{ "type": "text", "text": format!("unknown {other}") }], "isError": true }),
    }
}

fn text(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}
