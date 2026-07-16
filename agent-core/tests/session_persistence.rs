use std::sync::Arc;
use std::time::Duration;

use agent_core::{Session, SessionEvent, SystemPrompt};
use auger_traces::{TraceReader, session_trace_path};
use provider::{LlmModel, StreamEvent, UserPrompt};
use provider_dummy::{DummyProvider, DummyResponse};

#[test]
fn session_persists_a_replayable_trace() {
    let provider = DummyProvider::new_responses([DummyResponse::Stream(vec![
        Ok(StreamEvent::TextDelta("hello".to_owned())),
        Ok(StreamEvent::Done {
            usage: None,
            stop_reason: Some("stop".to_owned()),
        }),
    ])]);
    let model = LlmModel::new(Arc::new(provider), "dummy");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let (_owner, handle, events) = Session::start(
            model,
            SystemPrompt::new("system".to_owned()),
            tokio::runtime::Handle::current(),
        );
        let session_id = handle.id().as_uuid();
        handle
            .send_message(UserPrompt::new("Say hello.".to_owned()))
            .unwrap();

        tokio::task::spawn_blocking(move || {
            while !matches!(
                events.recv_event().unwrap(),
                SessionEvent::StreamEvent(StreamEvent::Done { .. })
            ) {}
        })
        .await
        .unwrap();

        let path = session_trace_path(session_id).unwrap();
        let trace = (0..100)
            .find_map(|_| {
                let trace = TraceReader::read(&path).ok()?;
                if trace.events().len() == 2 {
                    Some(trace)
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                    None
                }
            })
            .expect("session trace should contain the completed response");
        let value = serde_json::to_value(trace).unwrap();

        assert_eq!(value["header"]["session_id"], session_id.to_string());
        assert_eq!(value["events"][0]["type"], "input_message");
        assert_eq!(value["events"][0]["content"][0]["text"], "Say hello.");
        assert_eq!(value["events"][1]["type"], "assistant_message");
        assert_eq!(value["events"][1]["content"][0]["text"], "hello");
        eprintln!("trace file: {}", path.display());
    });
}
