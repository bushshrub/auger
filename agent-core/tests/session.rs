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
                match handle.recv_event().expect("session event channel should stay open") {
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
