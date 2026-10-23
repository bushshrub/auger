//! A tool phase resumed after a crash never runs a call twice.

use async_trait::async_trait;
use auger_driver::HarnessEntry;
use auger_driver::InputEntry;
use auger_driver::RestoreState;
use auger_driver::RestoredAgent;
use auger_driver::Turn;
use auger_toolengine::JsonSchema;
use auger_toolengine::Phase;
use auger_toolengine::PolicyCall;
use auger_toolengine::PolicyNode;
use auger_toolengine::Tool;
use auger_toolengine::ToolContext;
use auger_toolengine::ToolDetails;
use auger_toolengine::ToolError;
use auger_toolengine::ToolFact;
use auger_toolengine::ToolPhaseEngine;
use auger_toolengine::ToolSetup;
use auger_toolengine::Verdict;
use provider::AssistantResponse;
use provider::Block;
use provider::LlmModel;
use provider::ToolCallRequest;
use provider::UserPrompt;
use provider_dummy::DummyProvider;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

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

fn call(id: &str) -> ToolCallRequest {
    ToolCallRequest {
        id: id.to_string(),
        name: "counted".to_string(),
        arguments: "{}".to_string(),
    }
}

fn finished(id: &str, text: &str) -> ToolFact {
    serde_json::from_value(serde_json::json!({"finished": {
        "tool_call_id": id,
        "outcome": {"success": {"content": [{"text": {"text": text}}]}},
    }}))
    .unwrap()
}

#[test]
fn resume_keeps_finished_calls_and_never_reruns_started_ones() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let journaled = Arc::new(Mutex::new(Vec::new()));
    let journal = {
        let journaled = journaled.clone();
        Arc::new(move |fact| {
            journaled.lock().unwrap().push(fact);
            Ok(())
        })
    };
    let setup = ToolSetup::new(
        vec![Box::new(Counted(runs.clone()))],
        Arc::new(AllowAll),
        PathBuf::from("/"),
        PathBuf::from("/"),
        journal,
    );

    let response = AssistantResponse::new(vec![
        Block::ToolCall(call("a")),
        Block::ToolCall(call("b")),
        Block::ToolCall(call("c")),
    ])
    .unwrap();
    let turns = vec![
        Turn::Input {
            entries: vec![InputEntry::User(UserPrompt::new("go".to_string()))],
        },
        Turn::Output(response),
    ];
    let model = LlmModel::new(Arc::new(DummyProvider::new([])), "dummy");
    let restored = auger_driver::restore(model, String::new(), Vec::new(), RestoreState::new(turns, None));
    let Ok(RestoredAgent::WaitingForToolResponses(agent)) = restored else {
        panic!("expected pending tool calls");
    };

    let facts = vec![
        finished("a", "earlier"),
        ToolFact::Started {
            tool_call_id: "b".to_string(),
            cwd: PathBuf::from("/"),
        },
    ];
    let (tx, rx) = mpsc::channel();
    let send = Arc::new(move |msg| tx.send(msg).unwrap());
    let mut phase = ToolPhaseEngine::resume(agent.get_batch(), setup, rt.handle(), send, &facts);
    let batch = loop {
        match phase {
            Phase::Pending { engine, .. } => phase = engine.receive(rx.recv().unwrap()),
            Phase::Done(batch) => break batch,
            Phase::Withheld(_) => panic!("nothing was denied"),
        }
    };

    assert_eq!(runs.load(Ordering::SeqCst), 1, "only the call that never started runs");
    let started: Vec<String> = journaled
        .lock()
        .unwrap()
        .iter()
        .filter_map(|fact| match fact {
            ToolFact::Started { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec!["c".to_string()]);

    let agent = agent.add_all_tool_responses(batch);
    let Some(Turn::Input { entries }) = agent.turns().last() else {
        panic!("expected the tool results");
    };
    let results: Vec<(&str, &str)> = entries
        .iter()
        .filter_map(|entry| match entry {
            InputEntry::Harness(HarnessEntry::ToolResult(result)) => {
                Some((result.tool_call_id.as_str(), result.content.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(results[0], ("a", "earlier"));
    assert_eq!(results[1].0, "b");
    assert!(results[1].1.contains("outcome is"), "{}", results[1].1);
    assert_eq!(results[2], ("c", "ran"));
}
