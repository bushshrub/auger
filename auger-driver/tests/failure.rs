use std::sync::Arc;

use auger_driver::{StreamResult, TypedAgent, WaitingForUserMessage};
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

    let agent = TypedAgent::<WaitingForUserMessage>::new(model, "system".to_string(), Vec::new())
        .add_message(UserPrompt::new("first".to_string()));
    let agent = match agent.create_stream().await {
        StreamResult::Failed(agent) => {
            assert_eq!(agent.error().message, "stream failed");
            agent.retry()
        }
        _ => panic!("expected stream failure"),
    };
    let result = agent.create_stream().await;
    assert!(matches!(result, StreamResult::WaitingForUserMessage(_)));

    let requests = provider.requests();
    assert!(matches!(
        requests[1].messages(),
        [Message::System(_), Message::User { .. }]
    ));
}
