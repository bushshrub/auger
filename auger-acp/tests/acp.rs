//! Drives `auger_acp::run` over OS pipes with a scripted provider.

use auger_acp::Deps;
use provider::{AssistantResponse, Block, CompletedLlmResponse, LlmProvider, ToolCallRequest};
use provider_dummy::DummyProvider;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

struct Client {
    input: Option<PipeWriter>,
    output: BufReader<PipeReader>,
    agent: Option<JoinHandle<std::io::Result<()>>>,
    next_id: i64,
}

impl Client {
    fn start(provider: DummyProvider, sessions_dir: Option<PathBuf>) -> Self {
        let (in_reader, in_writer) = std::io::pipe().unwrap();
        let (out_reader, out_writer) = std::io::pipe().unwrap();
        let deps = Deps {
            provider: Arc::new(provider) as Arc<dyn LlmProvider>,
            model: "test-model".into(),
            sessions_dir,
            system_prompt: "test".into(),
            mcp_servers: Default::default(),
        };
        let agent = std::thread::spawn(move || auger_acp::run(in_reader, out_writer, deps));
        Self {
            input: Some(in_writer),
            output: BufReader::new(out_reader),
            agent: Some(agent),
            next_id: 1,
        }
    }

    fn send(&mut self, message: Value) {
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{message}").unwrap();
        input.flush().unwrap();
    }

    /// Send a request and return its id.
    fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        let n = self.output.read_line(&mut line).unwrap();
        assert!(n > 0, "agent closed its output");
        serde_json::from_str(&line).expect("agent wrote a non-JSON line")
    }

    /// Read until the response to `id`, returning everything read before it.
    fn until_response(&mut self, id: i64) -> (Vec<Value>, Value) {
        let mut seen = Vec::new();
        loop {
            let message = self.read();
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return (seen, message);
            }
            seen.push(message);
        }
    }

    fn finish(mut self) {
        drop(self.input.take());
        let mut rest = String::new();
        while self.output.read_line(&mut rest).unwrap() > 0 {
            assert!(
                serde_json::from_str::<Value>(rest.trim()).is_ok(),
                "non-JSON output: {rest}"
            );
            rest.clear();
        }
        self.agent.take().unwrap().join().unwrap().unwrap();
    }
}

fn updates(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .filter(|m| m["method"] == "session/update")
        .map(|m| m["params"]["update"].clone())
        .collect()
}

fn text_of(update: &Value) -> &str {
    update["content"]["text"].as_str().unwrap_or_default()
}

fn tool_response(id: &str, name: &str, arguments: Value) -> CompletedLlmResponse {
    CompletedLlmResponse {
        response: AssistantResponse::new(vec![Block::ToolCall(ToolCallRequest {
            id: id.into(),
            name: name.into(),
            arguments: arguments.to_string(),
        })])
        .unwrap(),
        usage: None,
        stop_reason: None,
    }
}

fn text_response(text: &str) -> CompletedLlmResponse {
    CompletedLlmResponse {
        response: AssistantResponse::new(vec![Block::Text(text.into())]).unwrap(),
        usage: None,
        stop_reason: None,
    }
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("auger-acp-{name}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn new_session(client: &mut Client, cwd: &Path) -> String {
    let id = client.request(
        "session/new",
        json!({"cwd": cwd, "mcpServers": []}),
    );
    let (_, response) = client.until_response(id);
    response["result"]["sessionId"].as_str().unwrap().to_string()
}

fn initialize(client: &mut Client) -> Value {
    let id = client.request("initialize", json!({"protocolVersion": 1, "clientCapabilities": {}}));
    let (_, response) = client.until_response(id);
    response
}

#[test]
fn initialize_advertises_load_and_protocol_v1() {
    let mut client = Client::start(DummyProvider::new([]), None);
    let response = initialize(&mut client);
    assert_eq!(response["result"]["protocolVersion"], 1);
    assert_eq!(response["result"]["agentCapabilities"]["loadSession"], true);
    client.finish();
}

#[test]
fn prompt_with_a_tool_call_streams_updates_and_ends_turn() {
    let dir = scratch_dir("read");
    fs::write(dir.join("hello.txt"), "hi there").unwrap();
    let provider = DummyProvider::new([
        tool_response("call_1", "read_file", json!({"path": "hello.txt"})),
        text_response("it says hi"),
    ]);
    let mut client = Client::start(provider, None);
    initialize(&mut client);
    let session = new_session(&mut client, &dir);

    let id = client.request(
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": "read it"}]}),
    );
    let (seen, response) = client.until_response(id);
    assert_eq!(response["result"]["stopReason"], "end_turn");

    let updates = updates(&seen);
    let call = updates
        .iter()
        .find(|u| u["sessionUpdate"] == "tool_call")
        .expect("tool_call update");
    assert_eq!(call["toolCallId"], "call_1");
    assert_eq!(call["kind"], "read");
    assert!(call["title"].as_str().is_some());
    assert_eq!(call["locations"][0]["path"], dir.join("hello.txt").to_str().unwrap());

    let statuses: Vec<&str> = updates
        .iter()
        .filter(|u| u["sessionUpdate"] == "tool_call_update")
        .map(|u| u["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["in_progress", "completed"]);
    let done = updates
        .iter()
        .rev()
        .find(|u| u["sessionUpdate"] == "tool_call_update")
        .unwrap();
    assert!(done["content"][0]["content"]["text"].as_str().unwrap().contains("hi there"));

    let text: String = updates
        .iter()
        .filter(|u| u["sessionUpdate"] == "agent_message_chunk")
        .map(text_of)
        .collect();
    assert_eq!(text, "it says hi");
    client.finish();
}

#[test]
fn write_asks_permission_and_allow_runs_the_tool() {
    let dir = scratch_dir("allow");
    let provider = DummyProvider::new([
        tool_response("call_w", "write_file", json!({"path": "out.txt", "content": "made"})),
        text_response("wrote it"),
    ]);
    let mut client = Client::start(provider, None);
    initialize(&mut client);
    let session = new_session(&mut client, &dir);

    let id = client.request(
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": "write"}]}),
    );
    let mut seen = Vec::new();
    let permission = loop {
        let message = client.read();
        if message["method"] == "session/request_permission" {
            break message;
        }
        seen.push(message);
    };
    assert_eq!(permission["params"]["sessionId"], session);
    assert_eq!(permission["params"]["toolCall"]["toolCallId"], "call_w");
    let option_ids: Vec<&str> = permission["params"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["optionId"].as_str().unwrap())
        .collect();
    assert!(option_ids.contains(&"allow-once"));
    assert!(option_ids.contains(&"reject-once"));

    client.send(json!({"jsonrpc": "2.0", "id": permission["id"],
        "result": {"outcome": {"outcome": "selected", "optionId": "allow-once"}}}));
    let (rest, response) = client.until_response(id);
    seen.extend(rest);
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(fs::read_to_string(dir.join("out.txt")).unwrap(), "made");
    assert!(updates(&seen).iter().any(|u| u["status"] == "completed"));
    client.finish();
}

#[test]
fn rejected_write_is_not_run_and_turn_ends() {
    let dir = scratch_dir("reject");
    let provider = DummyProvider::new([tool_response(
        "call_r",
        "write_file",
        json!({"path": "nope.txt", "content": "x"}),
    )]);
    let mut client = Client::start(provider, None);
    initialize(&mut client);
    let session = new_session(&mut client, &dir);

    let id = client.request(
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": "write"}]}),
    );
    let permission = loop {
        let message = client.read();
        if message["method"] == "session/request_permission" {
            break message;
        }
    };
    client.send(json!({"jsonrpc": "2.0", "id": permission["id"],
        "result": {"outcome": {"outcome": "selected", "optionId": "reject-once"}}}));
    let (_, response) = client.until_response(id);
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert!(!dir.join("nope.txt").exists());
    client.finish();
}

#[test]
fn load_replays_history_before_responding() {
    let dir = scratch_dir("load");
    let sessions = scratch_dir("load-sessions");
    fs::write(dir.join("a.txt"), "alpha").unwrap();
    let provider = DummyProvider::new([
        tool_response("call_l", "read_file", json!({"path": "a.txt"})),
        text_response("read alpha"),
    ]);
    let mut first = Client::start(provider, Some(sessions.clone()));
    initialize(&mut first);
    let session = new_session(&mut first, &dir);
    let id = first.request(
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": "read a"}]}),
    );
    first.until_response(id);
    first.finish();

    // A fresh agent process loads the stored session.
    let mut second = Client::start(DummyProvider::new([]), Some(sessions));
    initialize(&mut second);
    let id = second.request(
        "session/load",
        json!({"sessionId": session, "cwd": dir, "mcpServers": []}),
    );
    let (replayed, response) = second.until_response(id);
    assert_eq!(response["result"], json!({}));

    let updates = updates(&replayed);
    let user: String = updates
        .iter()
        .filter(|u| u["sessionUpdate"] == "user_message_chunk")
        .map(text_of)
        .collect();
    assert_eq!(user, "read a");
    assert!(updates.iter().any(|u| u["sessionUpdate"] == "tool_call" && u["toolCallId"] == "call_l"));
    assert!(updates.iter().any(|u| u["status"] == "completed"));
    let agent: String = updates
        .iter()
        .filter(|u| u["sessionUpdate"] == "agent_message_chunk")
        .map(text_of)
        .collect();
    assert_eq!(agent, "read alpha");
    second.finish();
}

#[test]
fn unknown_session_and_bad_cwd_are_errors() {
    let mut client = Client::start(DummyProvider::new([]), None);
    initialize(&mut client);
    let id = client.request("session/new", json!({"cwd": "relative", "mcpServers": []}));
    let (_, response) = client.until_response(id);
    assert!(response["error"]["message"].as_str().is_some());

    let id = client.request(
        "session/prompt",
        json!({"sessionId": "00000000-0000-0000-0000-000000000000", "prompt": [{"type": "text", "text": "hi"}]}),
    );
    let (_, response) = client.until_response(id);
    assert_eq!(response["error"]["code"], -32602);
    client.finish();
}
