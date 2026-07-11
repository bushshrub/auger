use std::sync::Arc;

use agent_core::{Session, SessionEvent, SystemPrompt};
use provider::{LlmModel, StreamEvent, UserPrompt};
use provider_dummy::{DummyProvider, DummyResponse};

#[test]
fn session_streams_all_provider_deltas() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let provider = DummyProvider::new_responses([DummyResponse::Stream(vec![
        Ok(StreamEvent::TextDelta("hello".to_string())),
        Ok(StreamEvent::TextDelta(" world".to_string())),
        Ok(StreamEvent::Done {
            usage: None,
            stop_reason: Some("stop".to_string()),
        }),
    ])]);
    let model = LlmModel::new(Arc::new(provider), "dummy");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");

    runtime.block_on(async move {
        let handle = Session::start(
            model,
            SystemPrompt::new("You are a test agent.".to_string()),
            tokio::runtime::Handle::current(),
        );
        handle
            .send_message(UserPrompt::new("Say hello.".to_string()))
            .expect("session should accept the message");

        let events = tokio::task::spawn_blocking(move || {
            let mut deltas = Vec::new();
            loop {
                match handle
                    .recv_event()
                    .expect("session event channel should stay open")
                {
                    SessionEvent::StreamEvent(StreamEvent::TextDelta(delta)) => {
                        deltas.push(delta);
                    }
                    SessionEvent::StreamEvent(StreamEvent::Done { .. }) => break,
                    SessionEvent::StreamEvent(_) => {}
                    SessionEvent::Closed => panic!("session closed before stream completed"),
                }
            }
            deltas
        })
        .await
        .expect("event receiver task should complete");

        assert_eq!(events, vec!["hello", " world"]);
    });
}

#[test]
fn session_runs_two_agentic_iterations_with_auto_approved_tool() {
    let provider = DummyProvider::new_responses([
        DummyResponse::Stream(vec![
            Ok(StreamEvent::ToolCallComplete {
                id: "call-1".to_string(),
                name: "dummy".to_string(),
                arguments: r#"{"message":"hello"}"#.to_string(),
            }),
            Ok(StreamEvent::Done {
                usage: None,
                stop_reason: Some("tool_calls".to_string()),
            }),
        ]),
        DummyResponse::Stream(vec![
            Ok(StreamEvent::TextDelta("done".to_string())),
            Ok(StreamEvent::Done {
                usage: None,
                stop_reason: Some("stop".to_string()),
            }),
        ]),
    ]);
    let provider_handle = provider.clone();
    let model = LlmModel::new(Arc::new(provider), "dummy");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");

    runtime.block_on(async move {
        let handle = Session::start_with_tools(
            model,
            SystemPrompt::new("You are a test agent.".to_string()),
            tokio::runtime::Handle::current(),
            vec![Box::new(builtin_tools::Dummy)],
            vec!["dummy".to_string()],
        );
        handle
            .send_message(UserPrompt::new("Use the dummy tool.".to_string()))
            .expect("session should accept the message");

        let events = tokio::task::spawn_blocking(move || {
            let mut text = Vec::new();
            let mut done_events = 0;
            loop {
                match handle
                    .recv_event()
                    .expect("session event channel should stay open")
                {
                    SessionEvent::StreamEvent(StreamEvent::TextDelta(delta)) => text.push(delta),
                    SessionEvent::StreamEvent(StreamEvent::Done { .. }) => {
                        done_events += 1;
                        if done_events == 2 {
                            break text;
                        }
                    }
                    SessionEvent::StreamEvent(_) => {}
                    SessionEvent::Closed => panic!("session closed before stream completed"),
                }
            }
        })
        .await
        .expect("event receiver task should complete");

        assert_eq!(events, vec!["done"]);
        assert_eq!(provider_handle.requests().len(), 2);
    });
}
