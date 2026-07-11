use std::sync::Arc;

use auger_driver::{Agent, AgentStatus};
use provider::{LlmError, LlmModel, LlmResponse, Message, StreamEvent, UserPrompt};
use provider_dummy::{DummyProvider, DummyResponse};

#[tokio::test]
async fn retries_failed_stream_without_partial_response() {
    let provider = DummyProvider::new_responses([
        DummyResponse::Stream(vec![
            Ok(StreamEvent::TextDelta("partial".to_string())),
            Err(LlmError {
                message: "stream failed".to_string(),
            }),
        ]),
        DummyResponse::Response(LlmResponse {
            content: "retried".to_string(),
            reasoning: None,
            tool_calls: None,
            usage: None,
            stop_reason: Some("stop".to_string()),
        }),
    ]);
    let model = LlmModel::new(Arc::new(provider.clone()), "dummy");

    let agent = Agent::new(model, "system", Vec::new())
        .send_message(UserPrompt::new("first".to_string()), |_| {})
        .unwrap()
        .await;
    assert_eq!(agent.status(), AgentStatus::Failed);

    let agent = agent.retry_after_failure(|_| {}).unwrap().await;
    assert_eq!(agent.status(), AgentStatus::WaitingForUserMessage);

    let requests = provider.requests();
    assert!(matches!(
        requests[1].messages(),
        [Message::System(_), Message::User { .. }]
    ));
}
