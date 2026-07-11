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
                LoopEvent::AgentToolResults(_, _) => {
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
    agent: Option<Agent>,
    active_operation: Option<ActiveOperation>,
    pending_batch: Option<ToolBatch<Resolving>>,
    /// Receiver to receive session commands and agent events from
    inbox: mpsc::Receiver<LoopEvent>,
    loop_event_tx: mpsc::Sender<LoopEvent>,
    /// Sender for the session to emit events through
    event_tx: mpsc::Sender<SessionEvent>,
    tool_registry: std::sync::Arc<ToolRegistry>,
    auto_approval: std::sync::Arc<AutoApprovalPolicy>,
    pending_tool_decisions: HashMap<String, bool>,
}

enum ActiveOperation {
    Streaming(CancellationToken),
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
            agent: Some(Agent::new(model, system_prompt, tool_defs)),
            active_operation: None,
            pending_batch: None,
            inbox: cmd_rx,
            loop_event_tx: cmd_tx.clone(),
            event_tx,
            tool_registry,
            auto_approval: std::sync::Arc::new(AutoApprovalPolicy::new(auto_approved_tools)),
            pending_tool_decisions: HashMap::new(),
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
                        if self.active_operation.is_some() {
                            continue;
                        }
                        let Some(agent) = self.agent.take() else {
                            continue;
                        };

                        let event_tx = self.event_tx.clone();
                        let stream = match agent.send_message(message, move |event| {
                            let _ = event_tx.send(SessionEvent::StreamEvent(event));
                        }) {
                            Ok(stream) => stream,
                            Err(error) => {
                                self.agent = Some(error.agent());
                                continue;
                            }
                        };
                        self.start_stream(&rt, stream);
                    }
                    SessionCommand::Interrupt => {
                        self.pending_tool_decisions.clear();
                        match &self.active_operation {
                            Some(ActiveOperation::Streaming(cancellation)) => {
                                info!(session_id = %self.id, "Interrupting current stream");
                                cancellation.cancel();
                            }
                            Some(ActiveOperation::ExecutingTools(cancellation)) => {
                                info!(session_id = %self.id, "Interrupting tool execution");
                                cancellation.cancel();
                            }
                            None => {
                                warn!(session_id = %self.id, "Received interrupt command while idle");
                            }
                        }
                    }
                    SessionCommand::ToolDecision { id, approved } => {
                        self.handle_tool_decision(&rt, id, approved);
                    }
                },
                LoopEvent::AgentToolResults(agent, batch) => {
                    self.submit_tool_results_and_start_stream(&rt, agent, batch);
                }
                LoopEvent::StreamCompletion(agent) => match agent.status() {
                    AgentStatus::WaitingForUserMessage => {
                        info!(session_id = %self.id, "Session is awaiting user message");
                        self.agent = Some(agent);
                        self.active_operation = None;
                    }
                    AgentStatus::WaitingForToolResponses => {
                        debug!(session_id = %self.id, "LLM has requested tool calls");
                        let batch = agent
                            .pending_tools()
                            .expect("tool responses must be pending");
                        for call in batch.requested() {
                            if self.auto_approval.is_approved(&call.name) {
                                self.pending_tool_decisions.insert(call.id.clone(), true);
                            }
                        }
                        self.agent = Some(agent);
                        self.pending_batch = Some(batch);
                        self.active_operation = None;
                        self.dispatch_tools_if_decided(&rt);
                    }
                    AgentStatus::Interrupted => {
                        self.agent = Some(agent);
                        self.active_operation = None;
                    }
                    AgentStatus::Failed => {
                        self.agent = Some(agent);
                        self.active_operation = None;
                    }
                },
            }
        }

        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }

    fn handle_tool_decision(&mut self, rt: &Handle, id: String, approved: bool) {
        let waiting_for_tools = self
            .agent
            .as_ref()
            .is_some_and(|agent| agent.status() == AgentStatus::WaitingForToolResponses);
        if !waiting_for_tools || self.pending_batch.is_none() {
            self.pending_tool_decisions.insert(id, approved);
            return;
        }
        if !self
            .pending_batch
            .as_ref()
            .is_some_and(|batch| batch.requested().any(|call| call.id == id))
        {
            warn!(session_id = %self.id, tool_call_id = %id, "Ignoring decision for unknown tool call");
            return;
        }
        self.pending_tool_decisions.insert(id, approved);
        self.dispatch_tools_if_decided(rt);
    }

    fn dispatch_tools_if_decided(&mut self, rt: &Handle) {
        let Some(batch) = self.pending_batch.as_ref() else {
            return;
        };
        if !batch
            .requested()
            .all(|call| self.pending_tool_decisions.contains_key(&call.id))
        {
            return;
        }

        let agent = self.agent.take().expect("agent must be waiting for tools");
        let batch = self.pending_batch.take().expect("tool batch was checked");
        let decisions = std::mem::take(&mut self.pending_tool_decisions);
        let calls = batch.requested().cloned().collect::<Vec<_>>();
        let registry = self.tool_registry.clone();
        let loop_event_tx = self.loop_event_tx.clone();
        let cancellation = CancellationToken::new();
        self.active_operation = Some(ActiveOperation::ExecutingTools(cancellation.clone()));
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
            let _ = loop_event_tx.send(LoopEvent::AgentToolResults(agent, resolved));
        });
    }

    fn submit_tool_results_and_start_stream(
        &mut self,
        rt: &Handle,
        agent: Agent,
        batch: ToolBatch<Resolved>,
    ) {
        let event_tx = self.event_tx.clone();
        let stream = match agent.submit_tool_results(batch, move |event| {
            let _ = event_tx.send(SessionEvent::StreamEvent(event));
        }) {
            Ok(stream) => stream,
            Err(error) => {
                self.agent = Some(error.agent());
                self.active_operation = None;
                return;
            }
        };
        self.start_stream(rt, stream);
    }

    fn start_stream(&mut self, rt: &Handle, stream: AgentStream) {
        self.active_operation = Some(ActiveOperation::Streaming(stream.interrupt_handle()));
        let loop_event_tx = self.loop_event_tx.clone();
        let session_id = self.id;
        rt.spawn(async move {
            info!(%session_id, "Starting stream");
            let result = stream.await;
            info!(%session_id, "Stream completed");
            let _ = loop_event_tx.send(LoopEvent::StreamCompletion(result));
        });
    }
}
