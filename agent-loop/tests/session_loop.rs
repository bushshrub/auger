use agent_loop::{
    LlmDelta, ModelTurnOutcome, Session, SessionCommand, SessionEvent, SessionStatus,
};
use provider::{LlmModel, LlmResponse, Message, UserPrompt};
use provider_dummy::DummyProvider;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn session_loop_accepts_multiple_happy_path_user_turns() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("create tokio runtime");
    let provider = DummyProvider::new([
        llm_response("first response"),
        llm_response("second response"),
    ]);
    let model = LlmModel::new(Arc::new(provider.clone()), "dummy-model");

    let handle = Session::start(
        "You are a test assistant.".to_string(),
        Vec::new(),
        model,
        runtime.handle().clone(),
    );

    handle
        .command_channel()
        .send(SessionCommand::AddUserMessage(UserPrompt::new(
            "hello".to_string(),
        )))
        .expect("send first user message");
    wait_for_request_count(&provider, 1);

    handle
        .command_channel()
        .send(SessionCommand::AddUserMessage(UserPrompt::new(
            "again".to_string(),
        )))
        .expect("send second user message");
    let requests = wait_for_request_count(&provider, 2);

    handle
        .command_channel()
        .send(SessionCommand::Shutdown)
        .expect("send shutdown");

    assert_eq!(requests[0].tools().len(), 0);
    assert_eq!(requests[0].messages().len(), 2);
    assert!(matches!(
        &requests[0].messages()[0],
        Message::System(system) if system == "You are a test assistant."
    ));
    assert!(matches!(
        &requests[0].messages()[1],
        Message::User { message, .. } if message.message() == "hello"
    ));

    assert_eq!(requests[1].messages().len(), 4);
    assert!(matches!(
        &requests[1].messages()[2],
        Message::Assistant { content, .. } if content == "first response"
    ));
    assert!(matches!(
        &requests[1].messages()[3],
        Message::User { message, .. } if message.message() == "again"
    ));
}

#[test]
fn session_loop_emits_model_turn_events() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("create tokio runtime");
    let provider = DummyProvider::new([LlmResponse {
        content: "hello".to_string(),
        reasoning: Some("thinking".to_string()),
        tool_calls: None,
        usage: None,
        stop_reason: Some("stop".to_string()),
    }]);
    let model = LlmModel::new(Arc::new(provider), "dummy-model");

    let handle = Session::start(
        "You are a test assistant.".to_string(),
        Vec::new(),
        model,
        runtime.handle().clone(),
    );

    handle
        .command_channel()
        .send(SessionCommand::AddUserMessage(UserPrompt::new(
            "hello".to_string(),
        )))
        .expect("send user message");

    let events = recv_until_model_turn_done(handle.event_channel());

    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::StateChanged(SessionStatus::LlmTurnRunning)
    )));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::LlmDelta(LlmDelta::AssistantContent(content)) if content == "hello"
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::LlmDelta(LlmDelta::AssistantReasoning(reasoning))
                if reasoning == "thinking"
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::ModelTurnDone(ModelTurnOutcome::AssistantMessageComplete {
                stop_reason,
                ..
            }) if stop_reason.as_deref() == Some("stop")
        )
    }));

    handle
        .command_channel()
        .send(SessionCommand::Shutdown)
        .expect("send shutdown");
}

fn llm_response(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        reasoning: None,
        tool_calls: None,
        usage: None,
        stop_reason: Some("stop".to_string()),
    }
}

fn recv_until_model_turn_done(receiver: &mpsc::Receiver<SessionEvent>) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    loop {
        let event = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("receive session event");
        let done = matches!(event, SessionEvent::ModelTurnDone(_));
        events.push(event);
        if done {
            return events;
        }
    }
}

fn wait_for_request_count(provider: &DummyProvider, count: usize) -> Vec<provider::LlmRequest> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let requests = provider.requests();
        if requests.len() >= count {
            return requests;
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for {count} dummy provider requests"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
