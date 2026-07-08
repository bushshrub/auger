use crate::system_prompt::SystemPrompt;
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::default_registry::default_tool_registry;
use crate::tools::tool_call_batch::{Complete, Resolving, ToolCallBatch, ToolCallId};
use crate::tools::tool_registry::ToolRegistry;
use agent_loop::{SessionCommand as LoopCommand, SessionEvent as LoopEvent};
use agent_tools::ToolCallResult;
use either::Either;
use futures::future::join_all;
use provider::{LlmModel, Message, StreamEvent, ToolCallRequest, UserPrompt};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tracing::warn;
use uuid::Uuid;

pub type SessionId = Uuid;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session is closed")]
    Closed,
}

#[derive(Clone, Debug)]
pub struct UserMessage {
    input: String,
}

impl UserMessage {
    pub fn new(input: String) -> Self {
        Self { input }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    StateChanged {
        state: &'static str,
    },
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCallRequested {
        id: String,
        name: String,
        arguments: String,
    },
    ToolCallResolved {
        id: String,
    },
    FinalAssistantResponse {
        content: String,
    },
    StreamFailed {
        error: String,
    },
    Interrupted,
}

#[derive(Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub model: String,
    pub created_at: SystemTime,
    commands: mpsc::Sender<HostCommand>,
    events: broadcast::Sender<SessionEvent>,
}

impl SessionHandle {
    pub fn enqueue(&self, message: UserMessage) -> Result<(), SessionError> {
        self.commands
            .send(HostCommand::UserInput(message))
            .map_err(|_| SessionError::Closed)
    }

    pub fn respond_to_tool_call(
        &self,
        tool_call_id: String,
        approved: bool,
        message: Option<String>,
    ) -> Result<(), SessionError> {
        self.commands
            .send(HostCommand::ToolDecision {
                tool_call_id,
                approved,
                message,
            })
            .map_err(|_| SessionError::Closed)
    }

    pub fn retry_response(&self) -> Result<(), SessionError> {
        self.commands
            .send(HostCommand::RetryResponse)
            .map_err(|_| SessionError::Closed)
    }

    pub fn snapshot(&self) -> Result<Vec<Message>, SessionError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.commands
            .send(HostCommand::Snapshot { reply: reply_tx })
            .map_err(|_| SessionError::Closed)?;
        reply_rx.recv().map_err(|_| SessionError::Closed)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }
}

pub struct Session;

impl Session {
    pub fn spawn(system_prompt: SystemPrompt, model: LlmModel) -> SessionHandle {
        Self::spawn_with_policy(system_prompt, model, AutoApprovalPolicy::new(Vec::new()))
    }

    pub(crate) fn spawn_with_policy(
        system_prompt: SystemPrompt,
        model: LlmModel,
        auto_approval: AutoApprovalPolicy,
    ) -> SessionHandle {
        let id = Uuid::new_v4();
        let model_name = model.name().to_string();
        let created_at = SystemTime::now();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, _) = broadcast::channel(256);
        let handle = Handle::current();
        let mut tools = default_tool_registry();
        let loop_handle = agent_loop::Session::start(
            String::from(system_prompt),
            tools.list_for_clanker(),
            model,
            handle.clone(),
        );

        let host_events = event_tx.clone();
        thread::spawn(move || {
            let mut runtime = HostRuntime {
                commands: command_rx,
                events: host_events,
                loop_commands: loop_handle.commands().clone(),
                loop_events: loop_handle.events(),
                tools: &mut tools,
                auto_approval,
                state: HostState::SessionWaitingForUser,
                tokio: handle,
            };
            runtime.run();
        });

        SessionHandle {
            id,
            model: model_name,
            created_at,
            commands: command_tx,
            events: event_tx,
        }
    }
}

enum HostCommand {
    UserInput(UserMessage),
    ToolDecision {
        tool_call_id: String,
        approved: bool,
        message: Option<String>,
    },
    RetryResponse,
    Snapshot {
        reply: mpsc::SyncSender<Vec<Message>>,
    },
}

enum HostState {
    SessionWaitingForUser,
    CoreStreaming,
    CoreStreamingFailed,
    PendingUserAction {
        batch: ToolCallBatch<Resolving>,
        decisions: HashMap<ToolCallId, bool>,
        messages: Vec<String>,
    },
}

struct HostRuntime<'a> {
    commands: mpsc::Receiver<HostCommand>,
    events: broadcast::Sender<SessionEvent>,
    loop_commands: mpsc::Sender<LoopCommand>,
    loop_events: &'a mpsc::Receiver<LoopEvent>,
    tools: &'a mut ToolRegistry,
    auto_approval: AutoApprovalPolicy,
    state: HostState,
    tokio: Handle,
}

impl HostRuntime<'_> {
    fn run(&mut self) {
        loop {
            self.drain_loop_events();
            match self.commands.recv_timeout(Duration::from_millis(10)) {
                Ok(command) => self.handle_command(command),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn handle_command(&mut self, command: HostCommand) {
        match command {
            HostCommand::UserInput(message) => self.handle_user_input(message),
            HostCommand::ToolDecision {
                tool_call_id,
                approved,
                message,
            } => self.handle_tool_decision(tool_call_id, approved, message),
            HostCommand::RetryResponse => self.handle_retry_response(),
            HostCommand::Snapshot { reply } => {
                let _ = reply.send(Vec::new());
            }
        }
    }

    fn handle_user_input(&mut self, message: UserMessage) {
        if !matches!(
            self.state,
            HostState::SessionWaitingForUser | HostState::CoreStreamingFailed
        ) {
            return;
        }

        if self
            .loop_commands
            .send(LoopCommand::SubmitInput(UserPrompt::new(message.input)))
            .is_ok()
        {
            self.state = HostState::CoreStreaming;
            self.emit(SessionEvent::StateChanged {
                state: "CoreStreaming",
            });
        }
    }

    fn handle_retry_response(&mut self) {
        if !matches!(self.state, HostState::CoreStreamingFailed) {
            return;
        }

        if self.loop_commands.send(LoopCommand::RetryResponse).is_ok() {
            self.state = HostState::CoreStreaming;
            self.emit(SessionEvent::StateChanged {
                state: "CoreStreaming",
            });
        }
    }

    fn handle_tool_decision(
        &mut self,
        tool_call_id: String,
        approved: bool,
        message: Option<String>,
    ) {
        let HostState::PendingUserAction {
            batch,
            mut decisions,
            mut messages,
        } = std::mem::replace(&mut self.state, HostState::SessionWaitingForUser)
        else {
            return;
        };

        decisions.insert(tool_call_id, approved);
        if let Some(message) = message {
            messages.push(message);
        }

        let all_decided = batch
            .requested()
            .all(|call| decisions.contains_key(&call.id));

        if all_decided {
            self.resolve_decided_batch(batch, decisions, messages);
        } else {
            self.state = HostState::PendingUserAction {
                batch,
                decisions,
                messages,
            };
            self.emit(SessionEvent::StateChanged {
                state: "PendingUserAction",
            });
        }
    }

    fn drain_loop_events(&mut self) {
        while let Ok(event) = self.loop_events.try_recv() {
            self.handle_loop_event(event);
        }
    }

    fn handle_loop_event(&mut self, event: LoopEvent) {
        match event {
            LoopEvent::StreamingStarted => {
                self.state = HostState::CoreStreaming;
                self.emit(SessionEvent::StateChanged {
                    state: "CoreStreaming",
                });
            }
            LoopEvent::StreamEvent(event) => self.forward_stream_event(event),
            LoopEvent::FinalAssistantResponse(message) => {
                self.state = HostState::SessionWaitingForUser;
                self.emit(SessionEvent::FinalAssistantResponse {
                    content: message.content().to_string(),
                });
                self.emit(SessionEvent::StateChanged {
                    state: "SessionWaitingForUser",
                });
            }
            LoopEvent::ModelToolCallBatchReady(calls) => self.handle_tool_batch(calls),
            LoopEvent::Interrupted => {
                self.state = HostState::SessionWaitingForUser;
                self.emit(SessionEvent::Interrupted);
                self.emit(SessionEvent::StateChanged {
                    state: "SessionWaitingForUser",
                });
            }
            LoopEvent::StreamFailed(err) => {
                self.state = HostState::CoreStreamingFailed;
                self.emit(SessionEvent::StreamFailed {
                    error: err.to_string(),
                });
                self.emit(SessionEvent::StateChanged {
                    state: "CoreStreamingFailed",
                });
            }
        }
    }

    fn handle_tool_batch(&mut self, calls: Vec<ToolCallRequest>) {
        for call in &calls {
            self.emit(SessionEvent::ToolCallRequested {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
        }

        let batch = ToolCallBatch::new_batch(calls.clone());
        let decisions: HashMap<_, _> = calls
            .iter()
            .filter(|call| self.auto_approval.is_approved(&call.name))
            .map(|call| (call.id.clone(), true))
            .collect();

        if decisions.len() == calls.len() {
            self.run_approved_tools("RunningAutoApprovedTools", batch, calls, Vec::new());
        } else {
            self.state = HostState::PendingUserAction {
                batch,
                decisions,
                messages: Vec::new(),
            };
            self.emit(SessionEvent::StateChanged {
                state: "PendingUserAction",
            });
        }
    }

    fn resolve_decided_batch(
        &mut self,
        batch: ToolCallBatch<Resolving>,
        decisions: HashMap<ToolCallId, bool>,
        messages: Vec<String>,
    ) {
        let approved: Vec<_> = batch
            .requested()
            .filter(|call| decisions.get(&call.id).copied().unwrap_or(false))
            .cloned()
            .collect();
        let denied: Vec<_> = batch
            .requested()
            .filter(|call| !decisions.get(&call.id).copied().unwrap_or(false))
            .map(|call| call.id.clone())
            .collect();

        let mut pending_batch = Some(batch);
        for id in denied {
            let batch = pending_batch.take().expect("batch still resolving");
            match batch.deny(&id) {
                Ok(Either::Left(next)) => pending_batch = Some(next),
                Ok(Either::Right(complete)) => {
                    self.submit_host_feedback(complete, messages);
                    return;
                }
                Err(err) => {
                    warn!(error = %err, "failed to deny tool call");
                    return;
                }
            }
        }

        if approved.is_empty() {
            return;
        }

        self.run_approved_tools(
            "RunningUserApprovedTools",
            pending_batch.expect("batch still resolving"),
            approved,
            messages,
        );
    }

    fn run_approved_tools(
        &mut self,
        state: &'static str,
        batch: ToolCallBatch<Resolving>,
        approved: Vec<ToolCallRequest>,
        messages: Vec<String>,
    ) {
        self.emit(SessionEvent::StateChanged { state });

        let tools = &*self.tools;
        let results = self.tokio.clone().block_on(async {
            join_all(approved.into_iter().map(|call| async move {
                let id = call.id.clone();
                let result = match tools.invoke(call).await {
                    Ok(result) => result,
                    Err(err) => ToolCallResult::error(err.to_string()),
                };
                (id, result)
            }))
            .await
        });

        let mut pending_batch = Some(batch);
        for (id, result) in results {
            self.emit(SessionEvent::ToolCallResolved { id: id.clone() });
            let batch = pending_batch.take().expect("batch still resolving");
            match batch.resolve(&id, result) {
                Ok(Either::Left(next)) => pending_batch = Some(next),
                Ok(Either::Right(complete)) => {
                    self.submit_host_feedback(complete, messages);
                    return;
                }
                Err(err) => {
                    warn!(error = %err, "failed to resolve tool call");
                    return;
                }
            }
        }
    }

    fn submit_host_feedback(&mut self, complete: ToolCallBatch<Complete>, messages: Vec<String>) {
        for message in messages {
            let _ = self
                .loop_commands
                .send(LoopCommand::AddSteeringPrompt(UserPrompt::new(message)));
        }
        let _ = self
            .loop_commands
            .send(LoopCommand::SubmitHostFeedback(complete.drain()));
        self.state = HostState::CoreStreaming;
        self.emit(SessionEvent::StateChanged {
            state: "CoreStreaming",
        });
    }

    fn forward_stream_event(&self, event: StreamEvent) {
        match event {
            StreamEvent::TextDelta(text) => self.emit(SessionEvent::TextDelta { text }),
            StreamEvent::ReasoningDelta(text) => self.emit(SessionEvent::ReasoningDelta { text }),
            StreamEvent::ToolCallComplete {
                id,
                name,
                arguments,
            } => self.emit(SessionEvent::ToolCallRequested {
                id,
                name,
                arguments,
            }),
            StreamEvent::ToolCall { .. } | StreamEvent::Done { .. } => {}
        }
    }

    fn emit(&self, event: SessionEvent) {
        let _ = self.events.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider::{LlmError, LlmResponse};
    use provider_dummy::DummyProvider;
    use std::sync::Arc;
    use tokio::runtime::Runtime;

    #[test]
    fn all_auto_approved_calls_run_and_submit_feedback() {
        let runtime = test_runtime();
        let _guard = runtime.enter();
        let provider = DummyProvider::new([tool_response("grep"), text_response("done")]);
        let handle = Session::spawn_with_policy(
            SystemPrompt::new("system".to_string()),
            LlmModel::new(Arc::new(provider.clone()), "dummy"),
            AutoApprovalPolicy::new(["grep".to_string()]),
        );
        let mut events = handle.subscribe();

        handle
            .enqueue(UserMessage::new("use tool".to_string()))
            .expect("send input");

        wait_for_state(&runtime, &mut events, "RunningAutoApprovedTools");
        wait_for_final(&runtime, &mut events);
        wait_for_request_count(&provider, 2);
    }

    #[test]
    fn user_gated_call_parks_until_denied() {
        let runtime = test_runtime();
        let _guard = runtime.enter();
        let provider = DummyProvider::new([tool_response("grep"), text_response("done")]);
        let handle = Session::spawn_with_policy(
            SystemPrompt::new("system".to_string()),
            LlmModel::new(Arc::new(provider.clone()), "dummy"),
            AutoApprovalPolicy::new(Vec::new()),
        );
        let mut events = handle.subscribe();

        handle
            .enqueue(UserMessage::new("use tool".to_string()))
            .expect("send input");

        wait_for_state(&runtime, &mut events, "PendingUserAction");
        handle
            .respond_to_tool_call("call_1".to_string(), false, None)
            .expect("deny tool");

        wait_for_state(&runtime, &mut events, "CoreStreaming");
        wait_for_final(&runtime, &mut events);
        wait_for_request_count(&provider, 2);
    }

    #[test]
    fn user_approved_call_runs_after_all_decisions() {
        let runtime = test_runtime();
        let _guard = runtime.enter();
        let provider = DummyProvider::new([tool_response("grep"), text_response("done")]);
        let handle = Session::spawn_with_policy(
            SystemPrompt::new("system".to_string()),
            LlmModel::new(Arc::new(provider.clone()), "dummy"),
            AutoApprovalPolicy::new(Vec::new()),
        );
        let mut events = handle.subscribe();

        handle
            .enqueue(UserMessage::new("use tool".to_string()))
            .expect("send input");
        wait_for_state(&runtime, &mut events, "PendingUserAction");

        handle
            .respond_to_tool_call("call_1".to_string(), true, None)
            .expect("approve tool");

        wait_for_state(&runtime, &mut events, "RunningUserApprovedTools");
        wait_for_final(&runtime, &mut events);
        wait_for_request_count(&provider, 2);
    }

    #[test]
    fn retry_response_leaves_streaming_failed() {
        let runtime = test_runtime();
        let _guard = runtime.enter();
        let provider = DummyProvider::with_results([
            Err(LlmError {
                message: "boom".to_string(),
            }),
            Ok(text_response("done")),
        ]);
        let handle = Session::spawn(
            SystemPrompt::new("system".to_string()),
            LlmModel::new(Arc::new(provider), "dummy"),
        );
        let mut events = handle.subscribe();

        handle
            .enqueue(UserMessage::new("hello".to_string()))
            .expect("send input");
        wait_for_state(&runtime, &mut events, "CoreStreamingFailed");

        handle.retry_response().expect("retry response");
        wait_for_state(&runtime, &mut events, "CoreStreaming");
        wait_for_final(&runtime, &mut events);
    }

    #[test]
    fn chat_leaves_streaming_failed() {
        let runtime = test_runtime();
        let _guard = runtime.enter();
        let provider = DummyProvider::with_results([
            Err(LlmError {
                message: "boom".to_string(),
            }),
            Ok(text_response("done")),
        ]);
        let handle = Session::spawn(
            SystemPrompt::new("system".to_string()),
            LlmModel::new(Arc::new(provider), "dummy"),
        );
        let mut events = handle.subscribe();

        handle
            .enqueue(UserMessage::new("hello".to_string()))
            .expect("send input");
        wait_for_state(&runtime, &mut events, "CoreStreamingFailed");

        handle
            .enqueue(UserMessage::new("new input".to_string()))
            .expect("send new input");
        wait_for_state(&runtime, &mut events, "CoreStreaming");
        wait_for_final(&runtime, &mut events);
    }

    fn test_runtime() -> Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create runtime")
    }

    fn text_response(content: &str) -> LlmResponse {
        LlmResponse {
            content: content.to_string(),
            reasoning: None,
            tool_calls: None,
            usage: None,
            stop_reason: Some("stop".to_string()),
        }
    }

    fn tool_response(name: &str) -> LlmResponse {
        LlmResponse {
            content: String::new(),
            reasoning: None,
            tool_calls: Some(vec![ToolCallRequest {
                id: "call_1".to_string(),
                name: name.to_string(),
                arguments: "{}".to_string(),
            }]),
            usage: None,
            stop_reason: Some("tool_calls".to_string()),
        }
    }

    fn wait_for_state(
        runtime: &Runtime,
        events: &mut broadcast::Receiver<SessionEvent>,
        expected: &'static str,
    ) {
        wait_for_event(
            runtime,
            events,
            |event| matches!(event, SessionEvent::StateChanged { state } if state == expected),
        );
    }

    fn wait_for_final(runtime: &Runtime, events: &mut broadcast::Receiver<SessionEvent>) {
        wait_for_event(runtime, events, |event| {
            matches!(event, SessionEvent::FinalAssistantResponse { .. })
        });
    }

    fn wait_for_event(
        runtime: &Runtime,
        events: &mut broadcast::Receiver<SessionEvent>,
        mut accept: impl FnMut(SessionEvent) -> bool,
    ) {
        runtime.block_on(async {
            let deadline = Duration::from_secs(2);
            loop {
                let event = tokio::time::timeout(deadline, events.recv())
                    .await
                    .expect("timed out waiting for event")
                    .expect("event channel closed");
                if accept(event) {
                    return;
                }
            }
        });
    }

    fn wait_for_request_count(provider: &DummyProvider, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if provider.requests().len() >= count {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for provider request"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
