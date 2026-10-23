//! A restored session whose tool set lost a tool it called says so once, at
//! start, before anything else.

use async_trait::async_trait;
use auger_core::JsonlSink;
use auger_core::RecordBody;
use auger_core::SessionBuilder;
use auger_core::SessionCommand;
use auger_core::SessionEvent;
use auger_core::StreamRecord;
use auger_core::SystemPrompt;
use auger_driver::Prompt;
use auger_toolengine::JsonSchema;
use auger_toolengine::PolicyCall;
use auger_toolengine::PolicyNode;
use auger_toolengine::Tool;
use auger_toolengine::ToolContext;
use auger_toolengine::ToolDetails;
use auger_toolengine::ToolError;
use auger_toolengine::Verdict;
use provider::AssistantResponse;
use provider::Block;
use provider::CompletedLlmResponse;
use provider::LlmModel;
use provider::ToolCallRequest;
use provider::UserPrompt;
use provider_dummy::DummyProvider;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Named(&'static str);

#[async_trait]
impl Tool for Named {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: self.0.to_string(),
            description: String::new(),
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(serde_json::json!({"type": "object"}))
    }

    async fn call(&self, _ctx: &ToolContext, _args: serde_json::Value) -> Result<String, ToolError> {
        Ok("ran".to_string())
    }
}

struct AllowAll;

impl PolicyNode for AllowAll {
    fn check(&self, _call: &PolicyCall<'_>) -> Verdict {
        Verdict::Allow { node: "test" }
    }
}

fn reply(blocks: Vec<Block>) -> CompletedLlmResponse {
    CompletedLlmResponse {
        response: AssistantResponse::new(blocks).unwrap(),
        usage: None,
        stop_reason: None,
    }
}

fn start(
    provider: DummyProvider,
    builder: SessionBuilder,
    tool: &'static str,
    rt: &tokio::runtime::Runtime,
) -> (auger_core::SessionHandle, std::sync::mpsc::Receiver<SessionEvent>) {
    builder
        .start(
            LlmModel::new(Arc::new(provider), "dummy"),
            rt.handle().clone(),
            vec![Box::new(Named(tool))],
            Arc::new(AllowAll),
        )
        .unwrap()
}

#[test]
fn restored_session_reports_tools_it_called_that_are_gone() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let call = Block::ToolCall(ToolCallRequest {
        id: "call_1".to_string(),
        name: "old_tool".to_string(),
        arguments: "{}".to_string(),
    });
    let provider = DummyProvider::new(vec![
        reply(vec![call]),
        reply(vec![Block::Text("done".to_string())]),
    ]);
    let trace = SharedBuf::default();
    let builder = SessionBuilder::new("dummy".to_string(), SystemPrompt::new("sys".to_string()))
        .sink(JsonlSink::new(trace.clone()));
    let (handle, events) = start(provider, builder, "old_tool", &rt);
    handle.send_command(SessionCommand::SendMessage(Prompt::User(UserPrompt::new("go".into()))))
        .unwrap();
    let mut completions = 0;
    while completions < 2 {
        if let SessionEvent::Record(record) = events.recv_timeout(Duration::from_secs(5)).unwrap() {
            if matches!(record.body, RecordBody::Stream(StreamRecord::Completed { .. })) {
                completions += 1;
            }
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    handle.send_command(SessionCommand::Stop { reply_tx: tx }).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let records = auger_core::read_jsonl(trace.0.lock().unwrap().as_slice()).unwrap();
    let builder = SessionBuilder::restore(records).unwrap();
    let (handle, events) = start(DummyProvider::new(vec![]), builder, "new_tool", &rt);
    let first = events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        matches!(&first, SessionEvent::ToolsMissing { names } if names == &["old_tool"]),
        "{first:?}"
    );
    let (tx, rx) = std::sync::mpsc::channel();
    handle.send_command(SessionCommand::Stop { reply_tx: tx }).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    while let Ok(event) = events.recv_timeout(Duration::from_secs(5)) {
        assert!(!matches!(event, SessionEvent::ToolsMissing { .. }), "emitted twice");
        if matches!(event, SessionEvent::Closed) {
            break;
        }
    }
}
