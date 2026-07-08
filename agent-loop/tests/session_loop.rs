use agent_loop::{Session, SessionCommand, SessionEvent};
use provider::{LlmModel, LlmResponse, Message, ToolCallRequest, ToolResult, UserPrompt};
use provider_dummy::DummyProvider;
use std::sync::Arc;
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
        .commands()
        .send(SessionCommand::SubmitInput(UserPrompt::new(
            "hello".to_string(),
        )))
        .expect("send first user message");
    wait_for_final_response(&handle);
    wait_for_request_count(&provider, 1);

    handle
        .commands()
        .send(SessionCommand::SubmitInput(UserPrompt::new(
            "again".to_string(),
        )))
        .expect("send second user message");
    wait_for_final_response(&handle);
    let requests = wait_for_request_count(&provider, 2);

    handle
        .commands()
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
fn session_loop_emits_tool_call_batch_ready() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("create tokio runtime");
    let provider = DummyProvider::new([llm_response_with_tool()]);
    let model = LlmModel::new(Arc::new(provider), "dummy-model");

    let handle = Session::start(
        "You are a test assistant.".to_string(),
        Vec::new(),
        model,
        runtime.handle().clone(),
    );

    handle
        .commands()
        .send(SessionCommand::SubmitInput(UserPrompt::new(
            "use a tool".to_string(),
        )))
        .expect("send user message");

    let calls = wait_for_tool_batch(&handle);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
}

#[test]
fn session_loop_accepts_host_feedback_and_streams_again() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("create tokio runtime");
    let provider = DummyProvider::new([
        llm_response_with_tool(),
        llm_response("tool result received"),
    ]);
    let model = LlmModel::new(Arc::new(provider), "dummy-model");

    let handle = Session::start(
        "You are a test assistant.".to_string(),
        Vec::new(),
        model,
        runtime.handle().clone(),
    );

    handle
        .commands()
        .send(SessionCommand::SubmitInput(UserPrompt::new(
            "use a tool".to_string(),
        )))
        .expect("send user message");
    wait_for_tool_batch(&handle);

    handle
        .commands()
        .send(SessionCommand::SubmitHostFeedback(vec![ToolResult::new(
            "call_1".to_string(),
            "done".to_string(),
        )]))
        .expect("send tool result");

    wait_for_final_response(&handle);
}

#[test]
fn session_loop_emits_stream_failed() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("create tokio runtime");
    let provider = DummyProvider::new([]);
    let model = LlmModel::new(Arc::new(provider), "dummy-model");

    let handle = Session::start(
        "You are a test assistant.".to_string(),
        Vec::new(),
        model,
        runtime.handle().clone(),
    );

    handle
        .commands()
        .send(SessionCommand::SubmitInput(UserPrompt::new(
            "hello".to_string(),
        )))
        .expect("send user message");

    wait_for_stream_failed(&handle);
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

fn llm_response_with_tool() -> LlmResponse {
    LlmResponse {
        content: String::new(),
        reasoning: None,
        tool_calls: Some(vec![ToolCallRequest {
            id: "call_1".to_string(),
            name: "example".to_string(),
            arguments: "{}".to_string(),
        }]),
        usage: None,
        stop_reason: Some("tool_calls".to_string()),
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

fn wait_for_final_response(handle: &agent_loop::SessionHandle) {
    wait_for_event(handle, |event| {
        matches!(event, SessionEvent::FinalAssistantResponse(_))
    });
}

fn wait_for_tool_batch(handle: &agent_loop::SessionHandle) -> Vec<ToolCallRequest> {
    let mut calls = None;
    wait_for_event(handle, |event| {
        if let SessionEvent::ModelToolCallBatchReady(batch) = event {
            calls = Some(batch);
            true
        } else {
            false
        }
    });
    calls.expect("tool batch event")
}

fn wait_for_stream_failed(handle: &agent_loop::SessionHandle) {
    wait_for_event(handle, |event| {
        matches!(event, SessionEvent::StreamFailed(_))
    });
}

fn wait_for_event(
    handle: &agent_loop::SessionHandle,
    mut accept: impl FnMut(SessionEvent) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match handle.events().recv_timeout(Duration::from_millis(10)) {
            Ok(event) => {
                if accept(event) {
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(err) => panic!("event channel closed: {err}"),
        }

        assert!(Instant::now() < deadline, "timed out waiting for event");
    }
}
