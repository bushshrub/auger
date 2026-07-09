use agent_loop::{
    LlmDelta, ModelTurnOutcome, Session, SessionCommand, SessionEvent, SessionStatus,
};
use provider::{LlmModel, LlmResponse, Message, ToolCallRequest, ToolResult, UserPrompt};
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

#[test]
fn session_loop_waits_until_all_tool_results_are_provided() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("create tokio runtime");
    let provider = DummyProvider::new([
        LlmResponse {
            content: String::new(),
            reasoning: None,
            tool_calls: Some(vec![
                tool_call("call_1", "read_file"),
                tool_call("call_2", "list_files"),
            ]),
            usage: None,
            stop_reason: Some("tool_calls".to_string()),
        },
        llm_response("done"),
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
            "use tools".to_string(),
        )))
        .expect("send user message");

    let events = recv_until_model_turn_done(handle.event_channel());
    assert!(events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::ModelTurnDone(ModelTurnOutcome::NeedsToolResults { tool_calls })
                if tool_calls.len() == 2
                    && tool_calls[0].id == "call_1"
                    && tool_calls[1].id == "call_2"
        )
    }));

    let event = handle
        .event_channel()
        .recv_timeout(Duration::from_secs(2))
        .expect("receive initial awaiting feedback status");
    assert!(matches!(
        event,
        SessionEvent::StateChanged(SessionStatus::AwaitingHostFeedback)
    ));

    handle
        .command_channel()
        .send(SessionCommand::AddToolResults(vec![ToolResult::new(
            "call_1".to_string(),
            "first result".to_string(),
        )]))
        .expect("send first tool result");

    let event = handle
        .event_channel()
        .recv_timeout(Duration::from_secs(2))
        .expect("receive awaiting feedback status");
    assert!(matches!(
        event,
        SessionEvent::StateChanged(SessionStatus::AwaitingHostFeedback)
    ));
    assert_eq!(provider.requests().len(), 1);

    handle
        .command_channel()
        .send(SessionCommand::AddToolResults(vec![ToolResult::new(
            "call_1".to_string(),
            "duplicate result".to_string(),
        )]))
        .expect("send duplicate tool result");

    let event = handle
        .event_channel()
        .recv_timeout(Duration::from_secs(2))
        .expect("receive invalid tool result error");
    assert!(matches!(
        event,
        SessionEvent::Error(agent_loop::SessionError::InvalidToolResult(_))
    ));
    assert_eq!(provider.requests().len(), 1);

    handle
        .command_channel()
        .send(SessionCommand::AddToolResults(vec![ToolResult::new(
            "call_2".to_string(),
            "second result".to_string(),
        )]))
        .expect("send second tool result");

    let requests = wait_for_request_count(&provider, 2);
    assert_eq!(requests[1].messages().len(), 4);
    assert!(matches!(
        &requests[1].messages()[3],
        Message::User {
            tool_call_results,
            ..
        } if tool_call_results.len() == 2
            && tool_call_results[0].id() == "call_1"
            && tool_call_results[1].id() == "call_2"
    ));

    handle
        .command_channel()
        .send(SessionCommand::Shutdown)
        .expect("send shutdown");
}

#[test]
fn session_snapshot_contains_loop_owned_thread_state() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("create tokio runtime");
    let provider = DummyProvider::new([llm_response("hello")]);
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
    recv_until_model_turn_done(handle.event_channel());

    let snapshot = snapshot(&handle);
    assert_eq!(snapshot.status(), SessionStatus::Idle);
    assert_eq!(snapshot.messages().len(), 3);
    assert!(matches!(
        &snapshot.messages()[1],
        Message::User { message, .. } if message.message() == "hello"
    ));
    assert!(matches!(
        &snapshot.messages()[2],
        Message::Assistant { content, .. } if content == "hello"
    ));

    handle
        .command_channel()
        .send(SessionCommand::Shutdown)
        .expect("send shutdown");
}

#[test]
fn session_snapshot_contains_tool_feedback_thread_state() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("create tokio runtime");
    let provider = DummyProvider::new([
        LlmResponse {
            content: String::new(),
            reasoning: None,
            tool_calls: Some(vec![tool_call("call_1", "read_file")]),
            usage: None,
            stop_reason: Some("tool_calls".to_string()),
        },
        llm_response("done"),
    ]);
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
            "use tools".to_string(),
        )))
        .expect("send user message");
    recv_until_model_turn_done(handle.event_channel());

    let awaiting_snapshot = snapshot(&handle);
    assert_eq!(
        awaiting_snapshot.status(),
        SessionStatus::AwaitingHostFeedback
    );
    assert_eq!(awaiting_snapshot.messages().len(), 3);
    assert!(matches!(
        &awaiting_snapshot.messages()[2],
        Message::Assistant { tool_calls, .. }
            if tool_calls.len() == 1 && tool_calls[0].id == "call_1"
    ));

    handle
        .command_channel()
        .send(SessionCommand::AddToolResults(vec![ToolResult::new(
            "call_1".to_string(),
            "file contents".to_string(),
        )]))
        .expect("send tool result");
    recv_until_model_turn_done(handle.event_channel());

    let done_snapshot = snapshot(&handle);
    assert_eq!(done_snapshot.status(), SessionStatus::Idle);
    assert_eq!(done_snapshot.messages().len(), 5);
    assert!(matches!(
        &done_snapshot.messages()[3],
        Message::User {
            tool_call_results,
            ..
        } if tool_call_results.len() == 1
            && tool_call_results[0].id() == "call_1"
            && tool_call_results[0].content() == "file contents"
    ));

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

fn tool_call(id: &str, name: &str) -> ToolCallRequest {
    ToolCallRequest {
        id: id.to_string(),
        name: name.to_string(),
        arguments: "{}".to_string(),
    }
}

fn snapshot(handle: &agent_loop::SessionHandle) -> agent_loop::SessionSnapshot {
    let (tx, rx) = mpsc::sync_channel(1);
    handle
        .command_channel()
        .send(SessionCommand::Snapshot { reply: tx })
        .expect("request snapshot");
    rx.recv().expect("receive snapshot")
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
