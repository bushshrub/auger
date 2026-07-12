use std::sync::Arc;

use auger_driver::{StreamResult, TypedAgent, WaitingForUserMessage};
use provider::{LlmModel, LlmResponse, Message, StreamEvent, UserPrompt};
use provider_dummy::{DummyProvider, DummyResponse};

#[tokio::test]
async fn continues_after_interruption_with_partial_response() {
    let provider = DummyProvider::new_responses([
        DummyResponse::PendingStream(vec![Ok(StreamEvent::TextDelta("partial".to_string()))]),
        DummyResponse::Response(LlmResponse {
            content: "continued".to_string(),
            reasoning: None,
            tool_calls: None,
            usage: None,
            stop_reason: Some("stop".to_string()),
        }),
    ]);
    let model = LlmModel::new(Arc::new(provider.clone()), "dummy");
    let event_seen = Arc::new(tokio::sync::Notify::new());

    let agent = TypedAgent::<WaitingForUserMessage>::new(model, "system".to_string(), Vec::new())
        .add_message(UserPrompt::new("first".to_string()))
        .add_event_callback({
            let event_seen = Arc::clone(&event_seen);
            move |_| event_seen.notify_one()
        });
    let stream = agent.create_stream();
    let interrupt = stream.interrupt_handle();
    let task = tokio::spawn(stream);
    event_seen.notified().await;
    interrupt.cancel();

    let agent = match task.await.expect("stream task should complete") {
        StreamResult::Interrupted(agent) => agent,
        _ => panic!("expected stream interruption"),
    };
    let agent = agent
        .add_message_to_continue(UserPrompt::new("continue".to_string()), true)
        .create_stream()
        .await;
    assert!(matches!(agent, StreamResult::WaitingForUserMessage(_)));

    let requests = provider.requests();
    assert!(matches!(
        requests[1].messages(),
        [Message::System(_), Message::User { .. }, Message::Assistant { content, .. }, Message::User { .. }]
            if content == "partial"
    ));
}
