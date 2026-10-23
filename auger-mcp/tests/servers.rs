//! Runs the fake MCP server from `src/bin` as a real child process.

use auger_mcp::{McpAllow, McpServers, ServerConfig};
use auger_toolengine::{PolicyCall, PolicyNode, Tool, ToolContext, ToolError, Verdict};
use std::collections::BTreeMap;
use std::time::Duration;

fn fake(allow: &[&str]) -> ServerConfig {
    ServerConfig {
        command: env!("CARGO_BIN_EXE_fake-mcp-server").to_string(),
        args: Vec::new(),
        env: BTreeMap::from([("FAKE_MCP_VALUE".to_string(), "from config".to_string())]),
        allow: allow.iter().map(|s| s.to_string()).collect(),
    }
}

fn servers(entries: &[(&str, ServerConfig)]) -> BTreeMap<String, ServerConfig> {
    entries
        .iter()
        .map(|(name, spec)| (name.to_string(), spec.clone()))
        .collect()
}

async fn start(entries: &[(&str, ServerConfig)]) -> (Vec<Box<dyn Tool>>, McpAllow) {
    let cwd = std::env::current_dir().unwrap();
    McpServers::start(&servers(entries), &cwd).await.into_parts()
}

fn find<'a>(tools: &'a [Box<dyn Tool>], name: &str) -> &'a dyn Tool {
    tools
        .iter()
        .find(|t| t.details().name == name)
        .map(|t| t.as_ref())
        .unwrap_or_else(|| panic!("no tool {name}"))
}

fn ctx() -> ToolContext {
    ToolContext::new(std::env::current_dir().unwrap())
}

fn check(policy: &McpAllow, tool: &str) -> Verdict {
    let cwd = std::env::current_dir().unwrap();
    policy.check(&PolicyCall {
        tool,
        path: None,
        command: None,
        cwd: &cwd,
        workspace: &cwd,
    })
}

#[tokio::test]
async fn lists_all_pages_with_prefixed_names() {
    let (tools, _) = start(&[("fake", fake(&[]))]).await;
    let mut names: Vec<String> = tools.iter().map(|t| t.details().name).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "mcp__fake__crash",
            "mcp__fake__echo",
            "mcp__fake__env",
            "mcp__fake__fail",
            "mcp__fake__image",
        ]
    );
    let echo = find(&tools, "mcp__fake__echo");
    assert_eq!(echo.details().description, "Echoes its message.");
    assert_eq!(echo.parameters().0["properties"]["message"]["type"], "string");
}

#[tokio::test]
async fn call_round_trip_returns_text_and_env_is_passed() {
    let (tools, _) = start(&[("fake", fake(&[]))]).await;
    let out = find(&tools, "mcp__fake__echo")
        .call(&ctx(), serde_json::json!({ "message": "hi" }))
        .await
        .unwrap();
    assert_eq!(out, "echo: hi");
    let env = find(&tools, "mcp__fake__env")
        .call(&ctx(), serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(env, "from config");
}

#[tokio::test]
async fn image_items_are_summarized() {
    let (tools, _) = start(&[("fake", fake(&[]))]).await;
    let out = find(&tools, "mcp__fake__image")
        .call(&ctx(), serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(out, "look\n[image omitted]");
}

#[tokio::test]
async fn is_error_maps_to_execution_error() {
    let (tools, _) = start(&[("fake", fake(&[]))]).await;
    let err = find(&tools, "mcp__fake__fail")
        .call(&ctx(), serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(matches!(&err, ToolError::Execution(msg) if msg == "boom"), "{err:?}");
}

#[tokio::test]
async fn exited_server_fails_calls_instead_of_hanging() {
    let (tools, _) = start(&[("fake", fake(&[]))]).await;
    let crash = find(&tools, "mcp__fake__crash");
    let echo = find(&tools, "mcp__fake__echo");
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let first = crash.call(&ctx(), serde_json::json!({})).await;
        let second = echo.call(&ctx(), serde_json::json!({ "message": "x" })).await;
        (first, second)
    })
    .await
    .expect("calls must not hang");
    assert!(matches!(result.0, Err(ToolError::Execution(_))), "{:?}", result.0);
    assert!(matches!(result.1, Err(ToolError::Execution(_))), "{:?}", result.1);
}

#[tokio::test]
async fn failed_server_is_left_out_and_others_run() {
    let broken = ServerConfig {
        command: "/nonexistent/auger-mcp-test-server".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        allow: vec!["*".to_string()],
    };
    let (tools, policy) = start(&[("broken", broken), ("fake", fake(&[]))]).await;
    assert!(tools.iter().all(|t| t.details().name.starts_with("mcp__fake__")));
    assert_eq!(tools.len(), 5);
    assert_eq!(check(&policy, "mcp__broken__anything"), Verdict::Next);
}

#[tokio::test]
async fn policy_allows_only_listed_tools_and_star() {
    let (_, policy) = start(&[
        ("fake", fake(&["echo"])),
        ("other", fake(&["*"])),
    ])
    .await;
    assert_eq!(check(&policy, "mcp__fake__echo"), Verdict::Allow { node: "mcp-allow" });
    assert_eq!(check(&policy, "mcp__fake__fail"), Verdict::Next);
    assert_eq!(check(&policy, "mcp__other__fail"), Verdict::Allow { node: "mcp-allow" });
    assert_eq!(check(&policy, "mcp__other__image"), Verdict::Allow { node: "mcp-allow" });
    assert_eq!(check(&policy, "read_file"), Verdict::Next);
}

#[tokio::test]
async fn policy_ignores_allow_entries_for_other_servers() {
    let (_, policy) = start(&[("fake", fake(&["*"]))]).await;
    assert_eq!(check(&policy, "mcp__fake__echo"), Verdict::Allow { node: "mcp-allow" });
    assert_eq!(check(&policy, "mcp__other__echo"), Verdict::Next);
}
