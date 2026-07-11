use crate::SystemPrompt;
use crate::events::{LoopEvent, SessionCommand, SessionEvent};
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::tool_registry::ToolRegistry;
use agent_tools::Tool;
use auger_driver::{Agent, AgentStatus, AgentStream, Resolved, Resolving, ToolBatch};
use provider::LlmModel;
use provider::UserPrompt;
use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(uuid::Uuid);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SessionId {
    pub(crate) fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

/// A handle to a running auger session
pub struct SessionHandle {
    id: SessionId,
    loop_event_tx: mpsc::Sender<LoopEvent>,
    event_rx: mpsc::Receiver<SessionEvent>,
}

impl SessionHandle {
    fn new(
        id: SessionId,
        command_tx: mpsc::Sender<LoopEvent>,
        event_rx: mpsc::Receiver<SessionEvent>,
    ) -> Self {
        Self {
            id,
            loop_event_tx: command_tx,
            event_rx,
        }
    }

    /// Receive the next event emitted by the session.
    pub fn recv_event(&self) -> Result<SessionEvent, mpsc::RecvError> {
        self.event_rx.recv()
    }

    /// Send a user message to the session.
    pub fn send_message(&self, message: UserPrompt) -> Result<(), mpsc::SendError<SessionCommand>> {
        self.loop_event_tx
            .send(LoopEvent::Cmd(SessionCommand::SendMessage(message)))
            .map_err(|error| match error.0 {
                LoopEvent::Cmd(command) => mpsc::SendError(command),
                LoopEvent::StreamCompletion(_) => {
                    unreachable!("the session handle only sends commands")
                }
                LoopEvent::AgentToolResults(_) => {
                    unreachable!("the session handle only sends commands")
                }
            })
    }

    /// Approve a pending tool call.
    pub fn approve_tool_call(
        &self,
        id: impl Into<String>,
    ) -> Result<(), mpsc::SendError<SessionCommand>> {
        self.send_command(SessionCommand::ToolDecision {
            id: id.into(),
            approved: true,
        })
    }

    /// Deny a pending tool call.
    pub fn deny_tool_call(
        &self,
        id: impl Into<String>,
    ) -> Result<(), mpsc::SendError<SessionCommand>> {
        self.send_command(SessionCommand::ToolDecision {
            id: id.into(),
            approved: false,
        })
    }

    /// Interrupt the current model stream or tool execution.
    pub fn interrupt(&self) -> Result<(), mpsc::SendError<SessionCommand>> {
        self.send_command(SessionCommand::Interrupt)
    }

    fn send_command(&self, command: SessionCommand) -> Result<(), mpsc::SendError<SessionCommand>> {
        self.loop_event_tx
            .send(LoopEvent::Cmd(command))
            .map_err(|error| match error.0 {
                LoopEvent::Cmd(command) => mpsc::SendError(command),
                _ => unreachable!("the session handle only sends commands"),
            })
    }

    /// Stop the session.
    pub fn stop(self) {
        todo!()
    }
}

pub struct Session {
    id: SessionId,
    agent: Agent,
    active_operation: ActiveOperation,
    /// Receiver to receive session commands and agent events from
    inbox: mpsc::Receiver<LoopEvent>,
    loop_event_tx: mpsc::Sender<LoopEvent>,
    /// Sender for the session to emit events through
    event_tx: mpsc::Sender<SessionEvent>,
    tool_registry: std::sync::Arc<ToolRegistry>,
    auto_approval: std::sync::Arc<AutoApprovalPolicy>,
}

enum ActiveOperation {
    Idle,
    Streaming {
        cancellation: CancellationToken,
        pending_message_after_interrupt: Option<UserPrompt>,
    },
    AwaitingToolDecisions {
        batch: ToolBatch<Resolving>,
        decisions: HashMap<String, bool>,
    },
    ExecutingTools(CancellationToken),
}

impl Session {
    pub fn start(model: LlmModel, system_prompt: SystemPrompt, rt: Handle) -> SessionHandle {
        Self::start_with_tools(model, system_prompt, rt, Vec::new(), Vec::new())
    }

    pub fn start_with_tools(
        model: LlmModel,
        system_prompt: SystemPrompt,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approved_tools: Vec<String>,
    ) -> SessionHandle {
        let mut tool_registry = ToolRegistry::new();
        for tool in tools {
            tool_registry.register(tool);
        }
        let tool_registry = std::sync::Arc::new(tool_registry);
        let tool_defs = tool_registry.list_for_clanker();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let session = Self {
            id: SessionId::new(),
            agent: Agent::new(model, system_prompt, tool_defs),
            active_operation: ActiveOperation::Idle,
            inbox: cmd_rx,
            loop_event_tx: cmd_tx.clone(),
            event_tx,
            tool_registry,
            auto_approval: std::sync::Arc::new(AutoApprovalPolicy::new(auto_approved_tools)),
        };
        let handle = SessionHandle::new(session.id, cmd_tx, event_rx);

        std::thread::Builder::new()
            .name(format!("auger-session-{}", session.id.0))
            .spawn(move || session.run(rt))
            .expect("failed to spawn session thread");

        handle
    }

    fn run(mut self, rt: Handle) {
        info!(session_id = %self.id, "Session started");
        while let Ok(event) = self.inbox.recv() {
            match event {
                LoopEvent::Cmd(command) => match command {
                    SessionCommand::SendMessage(message) => {
                        if let ActiveOperation::Streaming {
                            cancellation,
                            pending_message_after_interrupt,
                        } = &mut self.active_operation
                        {
                            if cancellation.is_cancelled()
                                && pending_message_after_interrupt.is_none()
                            {
                                *pending_message_after_interrupt = Some(message);
                            }
                            continue;
                        }
                        if !matches!(self.active_operation, ActiveOperation::Idle) {
                            continue;
                        }
                        self.send_message_and_start_stream(&rt, message);
                    }
                    SessionCommand::Interrupt => match &self.active_operation {
                        ActiveOperation::Streaming { cancellation, .. } => {
                            info!(session_id = %self.id, "Interrupting current stream");
                            cancellation.cancel();
                        }
                        ActiveOperation::ExecutingTools(cancellation) => {
                            info!(session_id = %self.id, "Interrupting tool execution");
                            cancellation.cancel();
                        }
                        ActiveOperation::Idle | ActiveOperation::AwaitingToolDecisions { .. } => {
                            warn!(session_id = %self.id, "Received interrupt command while idle");
                        }
                    },
                    SessionCommand::ToolDecision { id, approved } => {
                        self.handle_tool_decision(&rt, id, approved);
                    }
                },
                LoopEvent::AgentToolResults(batch) => {
                    self.submit_tool_results_and_start_stream(&rt, batch);
                }
                LoopEvent::StreamCompletion(completion) => {
                    self.agent.complete(completion);
                    match self.agent.status() {
                        AgentStatus::WaitingForUserMessage => {
                            info!(session_id = %self.id, "Session is awaiting user message");
                            self.active_operation = ActiveOperation::Idle;
                        }
                        AgentStatus::WaitingForToolResponses => {
                            debug!(session_id = %self.id, "LLM has requested tool calls");
                            let batch = self
                                .agent
                                .pending_tools()
                                .expect("tool responses must be pending");
                            self.active_operation = ActiveOperation::AwaitingToolDecisions {
                                batch,
                                decisions: HashMap::new(),
                            };
                            if let ActiveOperation::AwaitingToolDecisions {
                                batch, decisions, ..
                            } = &mut self.active_operation
                            {
                                for call in batch.requested() {
                                    if self.auto_approval.is_approved(&call.name) {
                                        decisions.insert(call.id.clone(), true);
                                    }
                                }
                            }
                            self.dispatch_tools_if_decided(&rt);
                        }
                        AgentStatus::Interrupted => {
                            let pending_message = match self.take_active_operation() {
                                ActiveOperation::Streaming {
                                    pending_message_after_interrupt,
                                    ..
                                } => pending_message_after_interrupt,
                                _ => None,
                            };
                            if let Some(message) = pending_message {
                                self.send_message_and_start_stream(&rt, message);
                            } else {
                                self.active_operation = ActiveOperation::Idle;
                            }
                        }
                        AgentStatus::Failed => {
                            self.active_operation = ActiveOperation::Idle;
                        }
                    }
                }
            }
        }

        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }

    fn handle_tool_decision(&mut self, rt: &Handle, id: String, approved: bool) {
        if matches!(&self.active_operation, ActiveOperation::Streaming { cancellation, .. } if cancellation.is_cancelled())
        {
            return;
        }
        let ActiveOperation::AwaitingToolDecisions {
            batch, decisions, ..
        } = &mut self.active_operation
        else {
            return;
        };
        if !batch.requested().any(|call| call.id == id) {
            warn!(session_id = %self.id, tool_call_id = %id, "Ignoring decision for unknown tool call");
            return;
        }
        decisions.insert(id, approved);
        self.dispatch_tools_if_decided(rt);
    }

    fn dispatch_tools_if_decided(&mut self, rt: &Handle) {
        let ActiveOperation::AwaitingToolDecisions {
            batch, decisions, ..
        } = &self.active_operation
        else {
            return;
        };
        if !batch
            .requested()
            .all(|call| decisions.contains_key(&call.id))
        {
            return;
        }

        let ActiveOperation::AwaitingToolDecisions { batch, decisions } =
            self.take_active_operation()
        else {
            unreachable!("tool batch was checked");
        };
        let calls = batch.requested().cloned().collect::<Vec<_>>();
        let registry = self.tool_registry.clone();
        let loop_event_tx = self.loop_event_tx.clone();
        let cancellation = CancellationToken::new();
        self.active_operation = ActiveOperation::ExecutingTools(cancellation.clone());
        rt.spawn(async move {
            let executions = futures::future::join_all(calls.into_iter().map(|call| {
                let registry = registry.clone();
                let approved = decisions[&call.id];
                async move {
                    if !approved {
                        return provider::ToolResult::new(
                            call.id,
                            "Tool call denied by user".to_string(),
                        );
                    }
                    info!(tool_call_id = %call.id, tool_name = %call.name, "Invoking tool");
                    let result = match registry.invoke(call.clone()).await {
                        Ok(result) => provider::ToolResult::new(call.id.clone(), result.to_string()),
                        Err(error) => provider::ToolResult::new(call.id.clone(), error.to_string()),
                    };
                    info!(tool_call_id = %call.id, tool_name = %call.name, "Tool invocation finished");
                    result
                }
            }));
            let resolved = tokio::select! {
                results = executions => batch
                    .resolve_all(results)
                    .expect("every tool call must have a result"),
                () = cancellation.cancelled() => batch.interrupt_remaining(),
            };
            let _ = loop_event_tx.send(LoopEvent::AgentToolResults(resolved));
        });
    }

    fn submit_tool_results_and_start_stream(&mut self, rt: &Handle, batch: ToolBatch<Resolved>) {
        let event_tx = self.event_tx.clone();
        let stream = match self.agent.submit_tool_results(batch, move |event| {
            let _ = event_tx.send(SessionEvent::StreamEvent(event));
        }) {
            Ok(stream) => stream,
            Err(_) => return,
        };
        self.start_stream(rt, stream);
    }

    fn start_stream(&mut self, rt: &Handle, stream: AgentStream) {
        self.active_operation = ActiveOperation::Streaming {
            cancellation: stream.interrupt_handle(),
            pending_message_after_interrupt: None,
        };
        let loop_event_tx = self.loop_event_tx.clone();
        let session_id = self.id;
        rt.spawn(async move {
            info!(%session_id, "Starting stream");
            let result = stream.await;
            info!(%session_id, "Stream completed");
            let _ = loop_event_tx.send(LoopEvent::StreamCompletion(result));
        });
    }

    fn send_message_and_start_stream(&mut self, rt: &Handle, message: UserPrompt) {
        let status = self.agent.status();
        let event_tx = self.event_tx.clone();
        let callback = move |event| {
            let _ = event_tx.send(SessionEvent::StreamEvent(event));
        };
        let stream = match status {
            AgentStatus::WaitingForUserMessage => self.agent.send_message(message, callback),
            AgentStatus::Interrupted => self
                .agent
                .continue_after_interruption(message, true, callback),
            AgentStatus::Failed => self.agent.continue_after_failure(message, callback),
            AgentStatus::WaitingForToolResponses => {
                return;
            }
        };
        match stream {
            Ok(stream) => self.start_stream(rt, stream),
            Err(_) => self.active_operation = ActiveOperation::Idle,
        }
    }

    fn take_active_operation(&mut self) -> ActiveOperation {
        std::mem::replace(&mut self.active_operation, ActiveOperation::Idle)
    }
}
