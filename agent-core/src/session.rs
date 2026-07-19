use crate::SystemPrompt;
use crate::events::{LoopMessage, SessionCommand, SessionEvent};
use crate::tools::auto_approval::AutoApprovalPolicies;
use crate::tools::tool_decisions::{ToolAuthorization, UserToolDecisions};
use crate::tools::tool_execution::ToolExecution;
use crate::tools::tool_registry::ToolRegistry;
use agent_tools::Tool;
use auger_traces::{
    AssistantContent, AssistantStatus, AuthorizationSource, Event, InputContent, ModelInfo,
    ProviderType, SessionRecord, ToolData, ToolDecision, TraceReader, TraceWriter,
};
use auger_driver::{StreamResult, TypedAgent, WaitingForUserMessage};
use provider::{LlmModel, Message, UserPrompt};
use std::fmt;
use std::sync::{mpsc, Arc};
use std::sync::mpsc::Sender;
use either::Either;
use tokio::runtime::Handle;
use tracing::{debug, error, info, warn};
use crate::states::HarnessState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(uuid::Uuid);

#[derive(Clone, Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("session is closed")]
    SessionClosed,
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SessionId {
    pub(crate) fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub fn as_uuid(self) -> uuid::Uuid {
        self.0
    }

    pub fn from_uuid(id: uuid::Uuid) -> Self {
        Self(id)
    }
}

/// A handle to a running auger session
#[derive(Clone)]
pub struct SessionHandle {
    id: SessionId,
    loop_event_tx: mpsc::Sender<LoopMessage>,
}

/// The unique capability to stop a running session.
pub struct SessionOwner {
    loop_event_tx: mpsc::Sender<LoopMessage>,
}

/// The unique receiver for events emitted by a session.
pub struct SessionEventReceiver {
    event_rx: mpsc::Receiver<SessionEvent>,
}

impl SessionHandle {
    fn new(id: SessionId, command_tx: mpsc::Sender<LoopMessage>) -> Self {
        Self {
            id,
            loop_event_tx: command_tx,
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn send_message(&self, prompt: UserPrompt) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::SendMessage(prompt)))
            .map_err(|_| ())
    }

    pub fn interrupt(&self) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::Interrupt))
            .map_err(|_| ())
    }

    /// Clone the recorded trace without changing session state.
    pub fn snapshot(&self) -> Result<SessionRecord, SnapshotError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::Snapshot { reply_tx }))
            .map_err(|_| SnapshotError::SessionClosed)?;
        reply_rx.recv().map_err(|_| SnapshotError::SessionClosed)
    }

    pub fn approve_tool_call(&self, id: impl Into<String>) -> Result<(), ()> {
        self.tool_decision(id, true, None)
    }

    pub fn deny_tool_call(&self, id: impl Into<String>) -> Result<(), ()> {
        self.tool_decision(id, false, Some("Denied by user".to_string()))
    }

    pub fn decide_tool_call(
        &self,
        id: impl Into<String>,
        approved: bool,
        message: Option<String>,
    ) -> Result<(), ()> {
        self.tool_decision(id, approved, message)
    }

    fn tool_decision(
        &self,
        id: impl Into<String>,
        approved: bool,
        message: Option<String>,
    ) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::ToolDecision {
                id: id.into(),
                approved,
                message,
            }))
            .map_err(|_| ())
    }

}

impl SessionOwner {
    /// Stop the session.
    pub fn stop(self) {
        let (reply_tx, reply_rx) = mpsc::channel();
        let _ = self
            .loop_event_tx
            .send(LoopMessage::Cmd(SessionCommand::Stop { reply_tx }));
        let _ = reply_rx.recv();
    }
}

impl SessionEventReceiver {
    /// Receive the next event emitted by the session.
    pub fn recv_event(&self) -> Result<SessionEvent, mpsc::RecvError> {
        self.event_rx.recv()
    }
}

pub struct Session {
    id: SessionId,
    /// Receiver to receive session commands and agent events from
    cmd_rx: mpsc::Receiver<LoopMessage>,
    harness_internal_event_tx: Sender<LoopMessage>,
    /// Sender for the session to emit events through
    event_tx: mpsc::Sender<SessionEvent>,
    tool_registry: Arc<ToolRegistry>,
    auto_approval_policies: Arc<AutoApprovalPolicies>,
    trace: SessionRecord,
    trace_writer: TraceWriter,
}

impl Session {
    pub fn start(
        model: LlmModel,
        system_prompt: SystemPrompt,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approval_policies: impl Into<AutoApprovalPolicies>
    ) -> (SessionOwner, SessionHandle, SessionEventReceiver) {
        Self::spawn(
            SessionId::new(),
            model,
            system_prompt,
            None,
            rt,
            tools,
            auto_approval_policies.into(),
        ).expect("new session history is valid")
    }

    /// Restore a session from a SessionRecord
    pub fn restore(
        model: LlmModel,
        record: SessionRecord,
        system_prompt: SystemPrompt,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approval_policies: impl Into<AutoApprovalPolicies>,
    ) -> (SessionOwner, SessionHandle, SessionEventReceiver) {
        let id = record.header().session_id();
        let messages = record.events();
        Self::spawn(
            SessionId(id),
            model,
            system_prompt,
            Some(messages),
            rt,
            tools,
            auto_approval_policies.into(),
        )
    }

    fn spawn(
        id: SessionId,
        model: LlmModel,
        system_prompt: SystemPrompt,
        messages: Vec<Message>,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approval_policies: AutoApprovalPolicies,
    ) -> (SessionOwner, SessionHandle, SessionEventReceiver) {
        let mut tool_registry = ToolRegistry::new();
        for tool in tools {
            tool_registry.register(tool);
        }
        let tool_registry = Arc::new(tool_registry);
        let llm_tools = tool_registry.list_for_clanker();
        let init_agent = TypedAgent::<WaitingForUserMessage>::restore(
            model,
            messages.clone(),
            llm_tools,
        );
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let trace_path = auger_traces::session_trace_path(id.as_uuid()).expect("trace path should be available");
        let mut trace = if trace_path.exists() {
            TraceReader::read(&trace_path).expect("existing session trace should be valid")
        } else {
            SessionRecord::new(
                id.as_uuid(),
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                ModelInfo::new(ProviderType::Unknown),
            )
        };
        if trace.events().is_empty() {
            if let Some(messages) = messages.as_ref() {
                for message in messages {
                    match message {
                        Message::User { message, .. } => {
                            trace.append_event(Event::InputMessage {
                                content: vec![InputContent::Text { text: message.message().to_owned() }],
                            });
                        }
                        Message::Assistant { reasoning, content: assistant_text, tool_calls } => {
                            let mut trace_content = Vec::new();
                            if let Some(reasoning) = reasoning {
                                trace_content.push(AssistantContent::Reasoning { text: reasoning.clone() });
                            }
                            if !assistant_text.is_empty() {
                                trace_content.push(AssistantContent::Text { text: assistant_text.clone() });
                            }
                            trace_content.extend(tool_calls.iter().map(|call| AssistantContent::ToolCall {
                                id: call.id.clone(), name: call.name.clone(),
                                arguments: serde_json::from_str(&call.arguments).unwrap_or_else(|_| serde_json::Value::String(call.arguments.clone())),
                            }));
                            trace.append_event(Event::AssistantMessage {
                                status: AssistantStatus::Completed,
                                content: trace_content,
                                provider_metadata: None,
                            });
                        }
                        Message::System(_) => {}
                    }
                }
            }
        }
        let trace_writer = TraceWriter::open(&trace).expect("failed to initialize trace storage");
        let session = Self {
            id,
            cmd_rx,
            harness_internal_event_tx: cmd_tx.clone(),
            event_tx,
            tool_registry,
            auto_approval_policies: Arc::new(auto_approval_policies),
            trace,
            trace_writer,
        };
        let handle = SessionHandle::new(session.id, cmd_tx.clone());
        let owner = SessionOwner {
            loop_event_tx: cmd_tx,
        };
        let events = SessionEventReceiver { event_rx };

        std::thread::Builder::new()
            .name(format!("auger-session-{}", session.id.0))
            .spawn(move || session.run(rt, init_agent))
            .expect("failed to spawn session thread");

        Ok((owner, handle, events))
    }

    fn run(mut self, rt: Handle, init_agent: TypedAgent<WaitingForUserMessage>) {
        info!(session_id = %self.id, "Session started");
        let mut curr_state = HarnessState::WaitingForUserMessage { agent: init_agent };
        'session_loop: while let Ok(msg) = self.cmd_rx.recv() {
            match msg {
                LoopMessage::Cmd(cmd) => {
                    match cmd {
                        SessionCommand::Stop { reply_tx } => {
                            let _ = reply_tx.send(());
                            break 'session_loop;
                        }
                        SessionCommand::Snapshot { reply_tx } => {
                            let _ = reply_tx.send(self.trace.clone());
                        }
                        SessionCommand::SendMessage(prompt) => {
                            info!(session_id = %self.id, "Received user message {:?}", prompt);
                            curr_state = match curr_state {
                                HarnessState::WaitingForUserMessage { agent } => {
                                    self.record_input_message(&prompt);
                                    let event_tx = self.event_tx.clone();
                                    let new_agent = agent.add_message(prompt).add_event_callback(move |event| {
                                        let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                    });
                                    let inbox_tx = self.harness_internal_event_tx.clone();
                                    let stream_fut = new_agent.create_stream();
                                    let cancel = stream_fut.interrupt_handle();
                                    let sess_id = self.id;
                                    rt.spawn(async move {
                                        info!(session_id=%sess_id, "Starting stream");
                                        let res = stream_fut.await;
                                        inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                    });
                                    HarnessState::Streaming { cancel }}
                                HarnessState::StreamingInterrupted { agent } => {
                                    self.record_input_message(&prompt);
                                    let event_tx = self.event_tx.clone();
                                    let new_agent = agent
                                        .add_message_to_continue(prompt, true)
                                        .add_event_callback(move |event| {
                                            let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                        });
                                    let inbox_tx = self.harness_internal_event_tx.clone();
                                    let stream_fut = new_agent.create_stream();
                                    let cancel = stream_fut.interrupt_handle();
                                    rt.spawn(async move {
                                        let res = stream_fut.await;
                                        inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                    });
                                    HarnessState::Streaming { cancel }
                                }
                                HarnessState::StreamingFailed { agent } => {
                                    self.record_input_message(&prompt);
                                    let event_tx = self.event_tx.clone();
                                    let new_agent = agent
                                        .add_message_to_continue(prompt)
                                        .add_event_callback(move |event| {
                                            let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                        });
                                    let inbox_tx = self.harness_internal_event_tx.clone();
                                    let stream_fut = new_agent.create_stream();
                                    let cancel = stream_fut.interrupt_handle();
                                    rt.spawn(async move {
                                        let res = stream_fut.await;
                                        inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                    });
                                    HarnessState::Streaming { cancel }
                                }
                                HarnessState::InterruptingStream { pending_message: None } => {
                                    HarnessState::InterruptingStream {
                                        pending_message: Some(prompt),
                                    }
                                }
                                _ => {
                                    warn!(session_id = %self.id, command = "send_message", "Ignoring command in invalid harness state");
                                    curr_state
                                }
                            }

                        }
                        SessionCommand::Interrupt => {
                            curr_state = match curr_state {
                                HarnessState::Streaming { cancel } => {
                                    cancel.cancel();
                                    HarnessState::InterruptingStream { pending_message: None }
                                }
                                HarnessState::ToolCallsAreRunning { agent, cancel } => {
                                    cancel.cancel();
                                    HarnessState::InterruptingToolExecution { agent }
                                }
                                _ => {
                                    warn!(session_id = %self.id, command = "interrupt", "Ignoring command in invalid harness state");
                                    curr_state
                                }
                            }
                        }
                        SessionCommand::ToolDecision { id, approved, message } => {
                            curr_state = match curr_state {
                                HarnessState::NeedToolConsent { agent, user_tool_decisions } => {
                                    let valid_decision = user_tool_decisions.is_undecided(&id);
                                    if valid_decision {
                                        self.record_tool_authorization(
                                            &id,
                                            approved,
                                            AuthorizationSource::User,
                                            message.clone(),
                                        );
                                    }
                                    match user_tool_decisions.record_decision(id, approved, message) {
                                        Either::Left(not_all_decided) => {
                                            HarnessState::NeedToolConsent {
                                                agent,
                                                user_tool_decisions: not_all_decided
                                            }
                                        }
                                        Either::Right(all_decided) => {
                                            let execution = ToolExecution::new(
                                                agent.get_batch(),
                                                ToolAuthorization::PerTool(all_decided),
                                                self.tool_registry.clone(),
                                                self.event_tx.clone(),
                                            ).run();
                                            let cancel = execution.interrupt_handle();
                                            let inbox_tx = self.harness_internal_event_tx.clone();
                                            rt.spawn(async move {
                                                let result = execution.await.resolve();
                                                let _ = inbox_tx.send(LoopMessage::ToolBatchExecutionResult(result));
                                            });
                                            HarnessState::ToolCallsAreRunning { agent, cancel }
                                        }
                                    }
                                }
                                _ => {
                                    warn!(session_id = %self.id, command = "tool_decision", "Ignoring command in invalid harness state");
                                    curr_state
                                }
                            }
                        }
                    }
                }
                LoopMessage::StreamResult(res) => {
                    curr_state = match curr_state {
                        HarnessState::Streaming { cancel } => {
                            drop(cancel);
                            match res {
                                StreamResult::Interrupted(_) => {
                                    panic!("stream returned interrupted while harness was still streaming")
                                }
                                StreamResult::Failed(agent) => {
                                    warn!(
                                        session_id = %self.id,
                                        error = %agent.error(),
                                        "LLM stream failed; waiting for a new user message"
                                    );
                                    let _ = self.event_tx.send(SessionEvent::StreamError {
                                        error: agent.error().to_string(),
                                    });
                                    let messages = agent.snapshot();
                                    self.record_last_assistant(&messages, AssistantStatus::Failed);
                                    HarnessState::StreamingFailed { agent }
                                }
                                StreamResult::WaitingForToolResponses(agent) => {
                                    debug!(session_id = %self.id, "agent has called tools");
                                    let messages = agent.snapshot();
                                    self.record_last_assistant(&messages, AssistantStatus::Completed);
                                    let tool_batch = agent.get_requested_tools();
                                    if self.auto_approval_policies.will_approve_all(&tool_batch) {
                                        info!(session_id=%self.id, "automatically running all tools");
                                        for call in &tool_batch {
                                            self.record_tool_authorization(
                                                &call.id,
                                                true,
                                                AuthorizationSource::Policy,
                                                None,
                                            );
                                        }
                                        let execution = ToolExecution::new(
                                            agent.get_batch(),
                                            ToolAuthorization::AllAutoApproved,
                                            self.tool_registry.clone(),
                                            self.event_tx.clone(),
                                        ).run();
                                        let cancel = execution.interrupt_handle();
                                        let inbox_tx = self.harness_internal_event_tx.clone();
                                        rt.spawn(async move {
                                            let result = execution.await.resolve();
                                            let _ = inbox_tx.send(LoopMessage::ToolBatchExecutionResult(result));
                                        });
                                        HarnessState::ToolCallsAreRunning { agent, cancel }
                                    } else {
                                        info!(session_id=%self.id, "Some tools require consent");
                                        for call in &tool_batch {
                                            if self.auto_approval_policies.is_approved(call) {
                                                self.record_tool_authorization(
                                                    &call.id,
                                                    true,
                                                    AuthorizationSource::Policy,
                                                    None,
                                                );
                                            }
                                        }
                                        let unapproved = self.auto_approval_policies.ids_needing_consent(&tool_batch);
                                        let tool_calls = agent
                                            .get_requested_tools()
                                            .into_iter()
                                            .filter(|call| unapproved.contains(&call.id))
                                            .collect();
                                        let _ = self.event_tx.send(SessionEvent::ToolConsentRequired {
                                            tool_calls,
                                        });
                                        HarnessState::NeedToolConsent { agent, user_tool_decisions: UserToolDecisions::new_undecided(unapproved) }
                                    }
                                }
                                StreamResult::WaitingForUserMessage(agent) => {
                                    info!(session_id=%self.id, "No tools called");
                                    let messages = agent.snapshot();
                                    self.record_last_assistant(&messages, AssistantStatus::Completed);
                                    HarnessState::WaitingForUserMessage { agent }
                                }
                            }
                        }
                        HarnessState::InterruptingStream { pending_message } => match res {
                            StreamResult::Interrupted(agent) => {
                                match pending_message {
                                    Some(prompt) => {
                                        self.record_input_message(&prompt);
                                        let event_tx = self.event_tx.clone();
                                        let new_agent = agent
                                            .add_message_to_continue(prompt, true)
                                            .add_event_callback(move |event| {
                                                let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                            });
                                        let inbox_tx = self.harness_internal_event_tx.clone();
                                        let stream_fut = new_agent.create_stream();
                                        let cancel = stream_fut.interrupt_handle();
                                        rt.spawn(async move {
                                            let res = stream_fut.await;
                                            inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                        });
                                        HarnessState::Streaming { cancel }
                                    }
                                    None => {
                                        let messages = agent.snapshot();
                                        self.record_last_assistant(&messages, AssistantStatus::Interrupted);
                                        let _ = self.event_tx.send(SessionEvent::Interrupted);
                                        HarnessState::StreamingInterrupted { agent }
                                    }
                                }
                            }
                            // TODO: we must handle these
                            StreamResult::Failed(_) => {
                                panic!("stream failed while harness was interrupting the stream")
                            }
                            StreamResult::WaitingForToolResponses(_) => {
                                panic!("stream requested tools while harness was interrupting the stream")
                            }
                            StreamResult::WaitingForUserMessage(_) => {
                                panic!("stream completed while harness was interrupting the stream")
                            }
                        },
                        _ => curr_state
                    };
                }
                LoopMessage::ToolBatchExecutionResult(tool_batch) => {
                    info!(session_id=%self.id, "tools have finished executing");
                    curr_state = match curr_state {
                        HarnessState::ToolCallsAreRunning { agent, cancel } => {
                            drop(cancel);
                            let new_agent = agent.add_all_tool_responses(tool_batch);
                            let messages = new_agent.snapshot();
                            self.record_last_input(&messages);
                            let event_tx = self.event_tx.clone();
                            let stream_fut = new_agent.add_event_callback(move |event| {
                                let _ = event_tx.send(SessionEvent::StreamEvent(event));
                            }).create_stream();
                            let cancel = stream_fut.interrupt_handle();
                            let inbox_tx = self.harness_internal_event_tx.clone();
                            rt.spawn(async move {
                                let res = stream_fut.await;
                                inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                            });
                            HarnessState::Streaming { cancel }
                        }
                        HarnessState::InterruptingToolExecution { agent } => {
                            let new_agent = agent.add_all_tool_responses(tool_batch);
                            let messages = new_agent.snapshot();
                            self.record_last_input(&messages);
                            let event_tx = self.event_tx.clone();
                            let stream_fut = new_agent.add_event_callback(move |event| {
                                let _ = event_tx.send(SessionEvent::StreamEvent(event));
                            }).create_stream();
                            let cancel = stream_fut.interrupt_handle();
                            let inbox_tx = self.harness_internal_event_tx.clone();
                            rt.spawn(async move {
                                let res = stream_fut.await;
                                inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                            });
                            HarnessState::Streaming { cancel }
                        }
                        _ => curr_state
                    }
                }
            }
        }


        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }

    /// Record a human input before it is submitted to the provider.
    fn record_input_message(&mut self, prompt: &UserPrompt) {
        if let Err(error) = self.append_trace_event(
            Event::InputMessage {
                content: vec![InputContent::Text {
                    text: prompt.message().to_owned(),
                }],
            },
        ) {
            self.report_trace_error(error);
        }
    }

    /// Record the provider input produced when completed tool results are resumed.
    fn record_last_input(&mut self, messages: &[Message]) {
        let Some(Message::User {
            message,
            tool_call_results,
        }) = messages.last()
        else {
            return;
        };

        let mut content = Vec::new();
        if !message.message().is_empty() {
            content.push(InputContent::Text {
                text: message.message().to_owned(),
            });
        }
        content.extend(tool_call_results.iter().map(|result| InputContent::ToolResult {
            tool_call_id: result.id().to_owned(),
            content: vec![ToolData::Text {
                text: result.content().to_owned(),
            }],
        }));

        if let Err(error) = self.append_trace_event(Event::InputMessage { content }) {
            self.report_trace_error(error);
        }
    }

    /// Record the committed assistant turn with its terminal stream status.
    fn record_last_assistant(&mut self, messages: &[Message], status: AssistantStatus) {
        let Some(Message::Assistant {
            reasoning,
            content,
            tool_calls,
        }) = messages.last()
        else {
            return;
        };

        let mut trace_content = Vec::new();
        if let Some(reasoning) = reasoning {
            trace_content.push(AssistantContent::Reasoning {
                text: reasoning.clone(),
            });
        }
        if !content.is_empty() {
            trace_content.push(AssistantContent::Text {
                text: content.clone(),
            });
        }
        trace_content.extend(tool_calls.iter().map(|call| AssistantContent::ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: serde_json::from_str(&call.arguments)
                .unwrap_or_else(|_| serde_json::Value::String(call.arguments.clone())),
        }));

        if let Err(error) = self.append_trace_event(
            Event::AssistantMessage {
                status,
                content: trace_content,
                provider_metadata: None,
            },
        ) {
            self.report_trace_error(error);
        }
    }

    fn record_tool_authorization(
        &mut self,
        tool_call_id: &str,
        approved: bool,
        source: AuthorizationSource,
        reason: Option<String>,
    ) {
        if let Err(error) = self.append_trace_event(Event::ToolAuthorization {
            tool_call_id: tool_call_id.to_owned(),
            decision: if approved {
                ToolDecision::Approved
            } else {
                ToolDecision::Denied
            },
            source,
            reason,
        }) {
            self.report_trace_error(error);
        }
    }

    /// Keep the in-memory trace and durable JSONL file in the same append order.
    fn append_trace_event(&mut self, event: Event) -> Result<(), auger_traces::TraceFileError> {
        self.trace.append_event(event);
        let record = self.trace.events().last().expect("event was appended");
        self.trace_writer.append(record)
    }

    fn report_trace_error(&self, error: auger_traces::TraceFileError) {
        error!(session_id = %self.id, error = %error, "failed to persist trace event");
        let _ = self.event_tx.send(SessionEvent::StreamError {
            error: format!("failed to persist session trace: {error}"),
        });
    }

}
