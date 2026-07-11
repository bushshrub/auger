use std::sync::Arc;

use auger_driver::{Agent, AgentStatus};
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

    let stream = Agent::new(model, "system", Vec::new())
        .send_message(UserPrompt::new("first".to_string()), {
            let event_seen = Arc::clone(&event_seen);
            move |_| event_seen.notify_one()
        })
        .unwrap();
    let interrupt = stream.interrupt_handle();
    let task = tokio::spawn(stream);
    event_seen.notified().await;
    interrupt.cancel();
    let agent = task.await.unwrap();

    assert_eq!(agent.status(), AgentStatus::Interrupted);
    let agent = agent
        .continue_after_interruption(UserPrompt::new("continue".to_string()), true, |_| {})
        .unwrap()
        .await;
    assert_eq!(agent.status(), AgentStatus::WaitingForUserMessage);

    let requests = provider.requests();
    assert!(matches!(
        requests[1].messages(),
        [Message::System(_), Message::User { .. }, Message::Assistant { content, .. }, Message::User { .. }]
            if content == "partial"
    ));
}
