use std::sync::Arc;
use std::sync::mpsc;

use agent_tools::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};
use async_trait::async_trait;
use agent_core::{Session, SessionEvent, SessionId, SystemPrompt};
use provider::{LlmError, LlmModel, Message, StreamEvent, UserPrompt};
use provider_dummy::{DummyProvider, DummyResponse};

struct PendingTool {
    started_tx: std::sync::Mutex<Option<mpsc::Sender<()>>>,
}

#[async_trait]
impl Tool for PendingTool {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "pending",
            description: "A tool that remains pending until interrupted.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(serde_json::json!({ "type": "object" }))
    }

    async fn call(&self, _args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        if let Some(started_tx) = self.started_tx.lock().unwrap().take() {
            let _ = started_tx.send(());
        }
        futures::future::pending().await
    }
}

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
        let (_owner, handle, events) = Session::start(
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
                match events
                    .recv_event()
                    .expect("session event channel should stay open")
                {
                    SessionEvent::StreamEvent(StreamEvent::TextDelta(delta)) => {
                        deltas.push(delta);
                    }
                    SessionEvent::StreamEvent(StreamEvent::Done { .. }) => break,
                    SessionEvent::StreamEvent(_)
                    | SessionEvent::ToolConsentRequired { .. }
                    | SessionEvent::ToolCallResult { .. }
                    | SessionEvent::ToolCallError { .. }
                    | SessionEvent::Interrupted
                    | SessionEvent::StreamError { .. } => {}
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
fn session_snapshots_trace_without_changing_state() {
    let provider = DummyProvider::new_responses([]);
    let model = LlmModel::new(Arc::new(provider), "dummy");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let (owner, handle, events) = Session::start(
            model,
            SystemPrompt::new("snapshot system".to_string()),
            tokio::runtime::Handle::current(),
        );
        tokio::task::spawn_blocking(move || {
            let first = handle.snapshot().unwrap();
            let second = handle.snapshot().unwrap();

            assert_eq!(
                serde_json::to_value(&first).unwrap(),
                serde_json::to_value(&second).unwrap()
            );
            owner.stop();
            assert!(matches!(events.recv_event().unwrap(), SessionEvent::Closed));
        })
        .await
        .unwrap();
    });
}

#[test]
fn restores_session_id_and_committed_history() {
    let model = LlmModel::new(Arc::new(DummyProvider::new_responses([])), "dummy");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let id = uuid::Uuid::new_v4();
    let messages = vec![Message::System("restored system".to_string())];

    runtime.block_on(async move {
        let (owner, handle, events) = Session::restore(
            SessionId::from_uuid(id),
            model,
            messages,
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        assert_eq!(handle.id().as_uuid(), id);
        let trace = serde_json::to_value(handle.snapshot().unwrap()).unwrap();
        assert_eq!(trace["header"]["session_id"], id.to_string());
        owner.stop();
        assert!(matches!(events.recv_event().unwrap(), SessionEvent::Closed));
    });
}

#[test]
fn session_records_input_while_streaming() {
    let provider = DummyProvider::new_responses([DummyResponse::PendingStream(vec![Ok(
        StreamEvent::TextDelta("partial".to_string()),
    )])]);
    let model = LlmModel::new(Arc::new(provider), "dummy");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let (owner, handle, events) = Session::start(
            model,
            SystemPrompt::new("system".to_string()),
            tokio::runtime::Handle::current(),
        );
        tokio::task::spawn_blocking(move || {
            handle
                .send_message(UserPrompt::new("start".to_string()))
                .unwrap();
            assert!(matches!(
                events.recv_event().unwrap(),
                SessionEvent::StreamEvent(StreamEvent::TextDelta(_))
            ));
            let trace = serde_json::to_value(handle.snapshot().unwrap()).unwrap();
            assert_eq!(trace["events"][0]["type"], "input_message");
            assert_eq!(trace["events"][0]["content"][0]["text"], "start");
            owner.stop();
        })
        .await
        .unwrap();
    });
}

#[test]
fn session_returns_to_waiting_for_user_message_after_streaming() {
    let provider = DummyProvider::new_responses([
        DummyResponse::Stream(vec![
            Ok(StreamEvent::TextDelta("first".to_string())),
            Ok(StreamEvent::Done {
                usage: None,
                stop_reason: Some("stop".to_string()),
            }),
        ]),
        DummyResponse::Stream(vec![
            Ok(StreamEvent::TextDelta("second".to_string())),
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
        let (_owner, handle, events) = Session::start(
            model,
            SystemPrompt::new("system".to_string()),
            tokio::runtime::Handle::current(),
        );
        let request_provider = provider_handle.clone();
        let handle = tokio::task::spawn_blocking(move || {
            handle
                .send_message(UserPrompt::new("first".to_string()))
                .expect("session should accept the first message");

            let mut first_text = String::new();
            loop {
                match events
                    .recv_event()
                    .expect("session event channel should stay open")
                {
                    SessionEvent::StreamEvent(StreamEvent::TextDelta(delta)) => {
                        first_text.push_str(&delta)
                    }
                    SessionEvent::StreamEvent(StreamEvent::Done { .. }) => break,
                    SessionEvent::StreamEvent(_)
                    | SessionEvent::ToolConsentRequired { .. }
                    | SessionEvent::ToolCallResult { .. }
                    | SessionEvent::ToolCallError { .. }
                    | SessionEvent::Interrupted
                    | SessionEvent::StreamError { .. } => {}
                    SessionEvent::Closed => panic!("session closed during first stream"),
                }
            }

            while request_provider.requests().len() < 2 {
                handle
                    .send_message(UserPrompt::new("second".to_string()))
                    .expect("session should accept commands");
                std::thread::yield_now();
            }

            let mut second_text = String::new();
            loop {
                match events
                    .recv_event()
                    .expect("session event channel should stay open")
                {
                    SessionEvent::StreamEvent(StreamEvent::TextDelta(delta)) => {
                        second_text.push_str(&delta)
                    }
                    SessionEvent::StreamEvent(StreamEvent::Done { .. }) => break,
                    SessionEvent::StreamEvent(_)
                    | SessionEvent::ToolConsentRequired { .. }
                    | SessionEvent::ToolCallResult { .. }
                    | SessionEvent::ToolCallError { .. }
                    | SessionEvent::Interrupted
                    | SessionEvent::StreamError { .. } => {}
                    SessionEvent::Closed => panic!("session closed during second stream"),
                }
            }

            (first_text, second_text)
        })
        .await
        .expect("session interaction task should complete");

        assert_eq!(handle, ("first".to_string(), "second".to_string()));
        assert_eq!(provider_handle.requests().len(), 2);
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
        let (_owner, handle, events) = Session::start_with_tools(
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
                match events
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
                    SessionEvent::StreamEvent(_)
                    | SessionEvent::ToolConsentRequired { .. }
                    | SessionEvent::ToolCallResult { .. }
                    | SessionEvent::ToolCallError { .. }
                    | SessionEvent::Interrupted
                    | SessionEvent::StreamError { .. } => {}
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

#[test]
fn session_interrupts_running_tool_and_submits_interrupted_result() {
    let provider = DummyProvider::new_responses([
        DummyResponse::Stream(vec![
            Ok(StreamEvent::ToolCallComplete {
                id: "call-pending".to_string(),
                name: "pending".to_string(),
                arguments: "{}".to_string(),
            }),
            Ok(StreamEvent::Done {
                usage: None,
                stop_reason: Some("tool_calls".to_string()),
            }),
        ]),
        DummyResponse::Stream(vec![
            Ok(StreamEvent::TextDelta("interrupted".to_string())),
            Ok(StreamEvent::Done {
                usage: None,
                stop_reason: Some("stop".to_string()),
            }),
        ]),
    ]);
    let provider_handle = provider.clone();
    let model = LlmModel::new(Arc::new(provider), "dummy");
    let (started_tx, started_rx) = mpsc::channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");

    runtime.block_on(async move {
        let (_owner, handle, events) = Session::start_with_tools(
            model,
            SystemPrompt::new("system".to_string()),
            tokio::runtime::Handle::current(),
            vec![Box::new(PendingTool {
                started_tx: std::sync::Mutex::new(Some(started_tx)),
            })],
            vec!["pending".to_string()],
        );

        tokio::task::spawn_blocking(move || {
            handle
                .send_message(UserPrompt::new("Run the pending tool.".to_string()))
                .unwrap();
            started_rx.recv().expect("tool should start");
            handle.interrupt().unwrap();

            let mut text = String::new();
            let mut error_events = Vec::new();
            loop {
                match events.recv_event().unwrap() {
                    SessionEvent::StreamEvent(StreamEvent::TextDelta(delta)) => {
                        text.push_str(&delta)
                    }
                    SessionEvent::StreamEvent(StreamEvent::Done { stop_reason, .. })
                        if stop_reason.as_deref() == Some("stop") => break,
                    SessionEvent::ToolCallError { id, error } => error_events.push((id, error)),
                    SessionEvent::StreamEvent(_)
                    | SessionEvent::ToolConsentRequired { .. }
                    | SessionEvent::ToolCallResult { .. }
                    | SessionEvent::Interrupted
                    | SessionEvent::StreamError { .. } => {}
                    SessionEvent::Closed => panic!("session closed before tool interruption completed"),
                }
            }

            assert_eq!(text, "interrupted");
            assert!(error_events.iter().any(|(id, error)| {
                id == "call-pending" && error.contains("interrupted before execution")
            }));
            let requests = provider_handle.requests();
            let results = requests[1]
                .messages()
                .iter()
                .find_map(|message| match message {
                    Message::User { tool_call_results, .. } if !tool_call_results.is_empty() => {
                        Some(tool_call_results)
                    }
                    _ => None,
                })
                .expect("second request should contain interrupted tool result");
            assert!(results.iter().any(|result| {
                result.id() == "call-pending"
                    && result.content().contains("interrupted before execution")
            }));
        })
        .await
        .unwrap();
    });
}

#[test]
fn session_lets_user_approve_one_tool_and_deny_another() {
    let provider = DummyProvider::new_responses([
        DummyResponse::Stream(vec![
            Ok(StreamEvent::ToolCallComplete {
                id: "call-approve".to_string(),
                name: "dummy".to_string(),
                arguments: r#"{"message":"approved"}"#.to_string(),
            }),
            Ok(StreamEvent::ToolCallComplete {
                id: "call-deny".to_string(),
                name: "dummy".to_string(),
                arguments: r#"{"message":"denied"}"#.to_string(),
            }),
            Ok(StreamEvent::Done {
                usage: None,
                stop_reason: Some("tool_calls".to_string()),
            }),
        ]),
        DummyResponse::Stream(vec![
            Ok(StreamEvent::TextDelta("finished".to_string())),
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
        let (_owner, handle, events) = Session::start_with_tools(
            model,
            SystemPrompt::new("You are a test agent.".to_string()),
            tokio::runtime::Handle::current(),
            vec![Box::new(builtin_tools::Dummy)],
            Vec::new(),
        );
        tokio::task::spawn_blocking(move || {
            handle
                .send_message(UserPrompt::new("Use both tools.".to_string()))
                .expect("session should accept the message");

            loop {
                match events
                    .recv_event()
                    .expect("session event channel should stay open")
                {
                    SessionEvent::ToolConsentRequired { tool_calls } => {
                        assert_eq!(tool_calls.len(), 2);
                        assert!(tool_calls.iter().any(|call| call.id == "call-approve"));
                        assert!(tool_calls.iter().any(|call| call.id == "call-deny"));
                        break;
                    }
                    SessionEvent::Closed => panic!("session closed before approval"),
                    SessionEvent::StreamEvent(_)
                    | SessionEvent::ToolCallResult { .. }
                    | SessionEvent::ToolCallError { .. }
                    | SessionEvent::Interrupted
                    | SessionEvent::StreamError { .. } => {}
                }
            }

            handle
                .deny_tool_call("call-deny")
                .expect("session should accept denial");
            handle
                .approve_tool_call("call-approve")
                .expect("session should accept approval");

            let mut text = String::new();
            let mut result_events = Vec::new();
            let mut error_events = Vec::new();
            loop {
                match events
                    .recv_event()
                    .expect("session event channel should stay open")
                {
                    SessionEvent::StreamEvent(StreamEvent::TextDelta(delta)) => {
                        text.push_str(&delta)
                    }
                    SessionEvent::StreamEvent(StreamEvent::Done { .. }) => break,
                    SessionEvent::ToolCallResult { id, .. } => result_events.push(id),
                    SessionEvent::ToolCallError { id, error } => error_events.push((id, error)),
                    SessionEvent::Closed => panic!("session closed before second stream completed"),
                    SessionEvent::StreamEvent(_)
                    | SessionEvent::ToolConsentRequired { .. }
                    | SessionEvent::Interrupted
                    | SessionEvent::StreamError { .. } => {}
                }
            }

            assert_eq!(text, "finished");
            assert_eq!(result_events, vec!["call-approve".to_string()]);
            assert!(
                error_events
                    .iter()
                    .any(|(id, error)| id == "call-deny" && error.contains("Denied"))
            );
            let requests = provider_handle.requests();
            assert_eq!(requests.len(), 2);
            let results = requests[1]
                .messages()
                .iter()
                .find_map(|message| match message {
                    provider::Message::User {
                        tool_call_results, ..
                    } if !tool_call_results.is_empty() => Some(tool_call_results),
                    _ => None,
                })
                .expect("second request should contain tool results");
            assert!(results.iter().any(|result| {
                result.id() == "call-approve" && result.content().contains("approved")
            }));
            assert!(results.iter().any(|result| {
                result.id() == "call-deny" && result.content().contains("Denied")
            }));
        })
        .await
        .expect("session interaction task should complete");
    });
}

#[test]
fn session_emits_interrupted_event_when_stream_is_interrupted() {
    let provider = DummyProvider::new_responses([DummyResponse::PendingStream(vec![Ok(
        StreamEvent::TextDelta("partial".to_string()),
    )])]);
    let model = LlmModel::new(Arc::new(provider), "dummy");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let (_owner, handle, events) = Session::start(
            model,
            SystemPrompt::new("system".to_string()),
            tokio::runtime::Handle::current(),
        );
        tokio::task::spawn_blocking(move || {
            handle
                .send_message(UserPrompt::new("start".to_string()))
                .unwrap();
            assert!(matches!(
                events.recv_event().unwrap(),
                SessionEvent::StreamEvent(StreamEvent::TextDelta(_))
            ));

            handle.interrupt().unwrap();
            loop {
                match events.recv_event().unwrap() {
                    SessionEvent::Interrupted => break,
                    SessionEvent::Closed => panic!("session closed before interrupted event"),
                    _ => {}
                }
            }

            let trace = serde_json::to_value(handle.snapshot().unwrap()).unwrap();
            assert_eq!(trace["events"][1]["status"], "interrupted");
            assert_eq!(trace["events"][1]["content"][0]["text"], "partial");
        })
        .await
        .unwrap();
    });
}

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "echo",
            description: "Returns a fixed string.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(serde_json::json!({ "type": "object" }))
    }

    async fn call(&self, _args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        Ok(ToolCallResult::success("echoed".to_string()))
    }
}

/// Regression test: a batch mixing an auto-approved call with one needing
/// consent used to panic in ToolAuthorization::denial_reason because the
/// decision set only tracks the consent-needing ids.
#[test]
fn session_runs_mixed_batch_of_auto_approved_and_consented_tools() {
    let provider = DummyProvider::new_responses([
        DummyResponse::Stream(vec![
            Ok(StreamEvent::ToolCallComplete {
                id: "call-auto".to_string(),
                name: "dummy".to_string(),
                arguments: r#"{"message":"auto"}"#.to_string(),
            }),
            Ok(StreamEvent::ToolCallComplete {
                id: "call-consent".to_string(),
                name: "echo".to_string(),
                arguments: "{}".to_string(),
            }),
            Ok(StreamEvent::Done {
                usage: None,
                stop_reason: Some("tool_calls".to_string()),
            }),
        ]),
        DummyResponse::Stream(vec![
            Ok(StreamEvent::TextDelta("finished".to_string())),
            Ok(StreamEvent::Done {
                usage: None,
                stop_reason: Some("stop".to_string()),
            }),
        ]),
    ]);
    let model = LlmModel::new(Arc::new(provider), "dummy");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");

    runtime.block_on(async move {
        let (_owner, handle, events) = Session::start_with_tools(
            model,
            SystemPrompt::new("You are a test agent.".to_string()),
            tokio::runtime::Handle::current(),
            vec![Box::new(builtin_tools::Dummy), Box::new(EchoTool)],
            vec!["dummy".to_string()],
        );
        tokio::task::spawn_blocking(move || {
            handle
                .send_message(UserPrompt::new("Use both tools.".to_string()))
                .expect("session should accept the message");

            loop {
                match events
                    .recv_event()
                    .expect("session event channel should stay open")
                {
                    SessionEvent::ToolConsentRequired { tool_calls } => {
                        // Only the non-auto-approved call needs consent.
                        assert_eq!(tool_calls.len(), 1);
                        assert_eq!(tool_calls[0].id, "call-consent");
                        break;
                    }
                    SessionEvent::Closed => panic!("session closed before approval"),
                    SessionEvent::StreamEvent(_)
                    | SessionEvent::ToolCallResult { .. }
                    | SessionEvent::ToolCallError { .. }
                    | SessionEvent::Interrupted
                    | SessionEvent::StreamError { .. } => {}
                }
            }

            handle
                .approve_tool_call("call-consent")
                .expect("session should accept approval");

            let mut text = String::new();
            let mut result_events = Vec::new();
            loop {
                match events
                    .recv_event()
                    .expect("session event channel should stay open")
                {
                    SessionEvent::StreamEvent(StreamEvent::TextDelta(delta)) => {
                        text.push_str(&delta)
                    }
                    SessionEvent::StreamEvent(StreamEvent::Done { .. }) => break,
                    SessionEvent::ToolCallResult { id, .. } => result_events.push(id),
                    SessionEvent::ToolCallError { id, error } => {
                        panic!("unexpected tool error for {id}: {error}")
                    }
                    SessionEvent::Closed => panic!("session closed before second stream completed"),
                    SessionEvent::StreamEvent(_)
                    | SessionEvent::ToolConsentRequired { .. }
                    | SessionEvent::Interrupted
                    | SessionEvent::StreamError { .. } => {}
                }
            }

            assert_eq!(text, "finished");
            result_events.sort();
            assert_eq!(result_events, vec!["call-auto", "call-consent"]);
        })
        .await
        .expect("session interaction task should complete");
    });
}

#[test]
fn session_accepts_message_after_interrupt_and_keeps_partial_response() {
    let provider = DummyProvider::new_responses([
        DummyResponse::PendingStream(vec![Ok(StreamEvent::TextDelta("partial".to_string()))]),
        DummyResponse::Stream(vec![Ok(StreamEvent::Done {
            usage: None,
            stop_reason: Some("stop".to_string()),
        })]),
    ]);
    let provider_handle = provider.clone();
    let model = LlmModel::new(Arc::new(provider), "dummy");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let (_owner, handle, events) = Session::start(
            model,
            SystemPrompt::new("system".to_string()),
            tokio::runtime::Handle::current(),
        );
        tokio::task::spawn_blocking(move || {
            handle
                .send_message(UserPrompt::new("first".to_string()))
                .unwrap();
            assert!(matches!(
                events.recv_event().unwrap(),
                SessionEvent::StreamEvent(StreamEvent::TextDelta(_))
            ));

            handle.interrupt().unwrap();
            handle
                .send_message(UserPrompt::new("continue".to_string()))
                .unwrap();
            while !matches!(
                events.recv_event().unwrap(),
                SessionEvent::StreamEvent(StreamEvent::Done { .. })
            ) {}

            let requests = provider_handle.requests();
            assert!(matches!(
                requests[1].messages(),
                [Message::System(_), Message::User { .. }, Message::Assistant { content, .. }, Message::User { .. }]
                    if content == "partial"
            ));
        })
        .await
        .unwrap();
    });
}

#[test]
fn session_accepts_message_after_stream_failure() {
    let provider = DummyProvider::new_responses([
        DummyResponse::Stream(vec![
            Ok(StreamEvent::TextDelta("partial".to_string())),
            Err(LlmError {
                message: "failed".to_string(),
            }),
        ]),
        DummyResponse::Stream(vec![Ok(StreamEvent::Done {
            usage: None,
            stop_reason: Some("stop".to_string()),
        })]),
    ]);
    let provider_handle = provider.clone();
    let model = LlmModel::new(Arc::new(provider), "dummy");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let (_owner, handle, events) = Session::start(
            model,
            SystemPrompt::new("system".to_string()),
            tokio::runtime::Handle::current(),
        );
        tokio::task::spawn_blocking(move || {
            handle
                .send_message(UserPrompt::new("first".to_string()))
                .unwrap();
            assert!(matches!(
                events.recv_event().unwrap(),
                SessionEvent::StreamEvent(StreamEvent::TextDelta(_))
            ));
            while provider_handle.requests().len() < 2 {
                handle
                    .send_message(UserPrompt::new("recover".to_string()))
                    .unwrap();
                std::thread::yield_now();
            }
            while !matches!(
                events.recv_event().unwrap(),
                SessionEvent::StreamEvent(StreamEvent::Done { .. })
            ) {}

            let requests = provider_handle.requests();
            assert!(matches!(
                requests[1].messages(),
                [
                    Message::System(_),
                    Message::User { .. },
                    Message::User { .. }
                ]
            ));
        })
        .await
        .unwrap();
    });
}

#[test]
fn session_emits_stream_error_when_provider_rejects_request() {
    let provider = DummyProvider::new_responses([DummyResponse::Error(LlmError {
        message: "context window exceeded".to_string(),
    })]);
    let model = LlmModel::new(Arc::new(provider), "dummy");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        let (_owner, handle, events) = Session::start(
            model,
            SystemPrompt::new("system".to_string()),
            tokio::runtime::Handle::current(),
        );
        tokio::task::spawn_blocking(move || {
            handle
                .send_message(UserPrompt::new("hello".to_string()))
                .unwrap();

            loop {
                match events.recv_event().unwrap() {
                    SessionEvent::StreamError { error } => {
                        assert_eq!(error, "context window exceeded");
                        break;
                    }
                    SessionEvent::Closed => panic!("session closed before stream error"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
    });
}
