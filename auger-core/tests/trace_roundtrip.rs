//! A session's trace, written as JSONL and read back, restores the session:
//! the model sees the same conversation, and a tool call that was cut off is
//! not run again.

use async_trait::async_trait;
use auger_core::JsonlSink;
use auger_core::Record;
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
use auger_toolengine::ToolFact;
use auger_toolengine::Verdict;
use provider::AssistantResponse;
use provider::Block;
use provider::CompletedLlmResponse;
use provider::LlmModel;
use provider::Message;
use provider::ToolCallRequest;
use provider::UserBlock;
use provider::UserPrompt;
use provider_dummy::DummyProvider;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
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

impl SharedBuf {
    fn records(&self) -> Vec<Record> {
        auger_core::read_jsonl(self.0.lock().unwrap().as_slice()).unwrap()
    }
}

struct Counted(Arc<AtomicUsize>);

#[async_trait]
impl Tool for Counted {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "counted".to_string(),
            description: "counts its runs".to_string(),
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(serde_json::json!({"type": "object"}))
    }

    async fn call(&self, _ctx: &ToolContext, _args: serde_json::Value) -> Result<String, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
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

fn tool_call() -> Block {
    Block::ToolCall(ToolCallRequest {
        id: "call_1".to_string(),
        name: "counted".to_string(),
        arguments: "{}".to_string(),
    })
}

fn text(text: &str) -> Block {
    Block::Text(text.to_string())
}

fn user(text: &str) -> SessionCommand {
    SessionCommand::SendMessage(Prompt::User(UserPrompt::new(text.to_string())))
}

/// Wait until `count` streams have completed.
fn wait_for_completions(events: &Receiver<SessionEvent>, count: usize) {
    let mut seen = 0;
    while seen < count {
        let event = events
            .recv_timeout(Duration::from_secs(5))
            .expect("session went quiet");
        if let SessionEvent::Record(Record {
            body: RecordBody::Stream(StreamRecord::Completed { .. }),
            ..
        }) = event
        {
            seen += 1;
        }
    }
}

struct Run {
    provider: DummyProvider,
    runs: Arc<AtomicUsize>,
}

impl Run {
    fn new(replies: Vec<CompletedLlmResponse>, runs: Arc<AtomicUsize>) -> Self {
        Self {
            provider: DummyProvider::new(replies),
            runs,
        }
    }

    fn start(
        &self,
        builder: SessionBuilder,
        rt: &tokio::runtime::Runtime,
    ) -> (auger_core::SessionHandle, Receiver<SessionEvent>) {
        builder
            .start(
                LlmModel::new(Arc::new(self.provider.clone()), "dummy"),
                rt.handle().clone(),
                vec![Box::new(Counted(self.runs.clone()))],
                Arc::new(AllowAll),
            )
            .unwrap()
    }

    /// The conversation the model was last sent, without the system prompt.
    fn last_request(&self) -> Vec<Message> {
        let request = self.provider.requests().pop().unwrap();
        request.messages()[1..].to_vec()
    }
}

/// A short rendering of messages, enough to compare conversations.
fn shape(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .map(|message| match message {
            Message::System(_) => "system".to_string(),
            Message::User { blocks } => blocks
                .iter()
                .map(|block| match block {
                    UserBlock::Text(text) => format!("user:{text}"),
                    UserBlock::Harness(text) => format!("harness:{text}"),
                    UserBlock::ToolResult(result) => {
                        format!("result:{}:{}", result.tool_call_id, result.content)
                    }
                })
                .collect::<Vec<_>>()
                .join(" | "),
            Message::Assistant { response } => format!("assistant:{:?}", response.blocks()),
        })
        .collect()
}

fn stop(handle: &auger_core::SessionHandle) {
    let (tx, rx) = std::sync::mpsc::channel();
    handle.send_command(SessionCommand::Stop { reply_tx: tx }).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
}

#[test]
fn restored_session_sends_the_same_conversation() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let first = Run::new(vec![reply(vec![tool_call()]), reply(vec![text("done")])], runs.clone());
    let trace = SharedBuf::default();
    let builder = SessionBuilder::new("dummy".to_string(), SystemPrompt::new("sys".to_string()))
        .sink(JsonlSink::new(trace.clone()));
    let (handle, events) = first.start(builder, &rt);
    handle.send_command(user("go")).unwrap();
    wait_for_completions(&events, 2);
    stop(&handle);

    let records = trace.records();
    assert!(records.iter().enumerate().all(|(i, r)| r.seq == i as u64));

    let second = Run::new(vec![reply(vec![text("again")])], runs.clone());
    let (handle, events) = second.start(SessionBuilder::restore(records).unwrap(), &rt);
    handle.send_command(user("more")).unwrap();
    wait_for_completions(&events, 1);
    stop(&handle);

    assert_eq!(
        shape(&second.last_request()),
        vec![
            "user:go".to_string(),
            format!("assistant:{:?}", vec![tool_call()]),
            "result:call_1:ran".to_string(),
            format!("assistant:{:?}", vec![text("done")]),
            "user:more".to_string(),
        ]
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn a_tool_cut_off_mid_run_is_not_run_again() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let first = Run::new(vec![reply(vec![tool_call()]), reply(vec![text("done")])], runs.clone());
    let trace = SharedBuf::default();
    let builder = SessionBuilder::new("dummy".to_string(), SystemPrompt::new("sys".to_string()))
        .sink(JsonlSink::new(trace.clone()));
    let (handle, events) = first.start(builder, &rt);
    handle.send_command(user("go")).unwrap();
    wait_for_completions(&events, 2);
    stop(&handle);

    // Crash right after the call started: drop everything from there on.
    let mut records = trace.records();
    let started = records
        .iter()
        .position(|r| matches!(r.body, RecordBody::Tool(ToolFact::Started { .. })))
        .unwrap();
    records.truncate(started + 1);

    let second = Run::new(vec![reply(vec![text("recovered")])], runs.clone());
    let (handle, events) = second.start(SessionBuilder::restore(records).unwrap(), &rt);
    wait_for_completions(&events, 1);
    stop(&handle);

    assert_eq!(runs.load(Ordering::SeqCst), 1, "the cut-off call must not run again");
    let sent = shape(&second.last_request());
    assert!(
        sent.last().unwrap().starts_with("result:call_1:auger stopped"),
        "{sent:?}"
    );
}
