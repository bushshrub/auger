use crate::SystemPrompt;
use crate::events::{LoopEvent, SessionCommand, SessionEvent};
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::tool_registry::ToolRegistry;
use agent_tools::Tool;
use auger_driver::{Agent, AgentStatus, Resolved, Resolving, ToolBatch};
use provider::LlmModel;
use provider::UserPrompt;
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
                LoopEvent::UserToolResult { .. } => {
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
    activity: Activity,
    pending_batch: Option<ToolBatch<Resolving>>,
    /// Receiver to receive session commands and agent events from
    inbox: mpsc::Receiver<LoopEvent>,
    loop_event_tx: mpsc::Sender<LoopEvent>,
    /// Sender for the session to emit events through
    event_tx: mpsc::Sender<SessionEvent>,
    tool_registry: std::sync::Arc<ToolRegistry>,
    auto_approval: std::sync::Arc<AutoApprovalPolicy>,
    pending_tool_decisions: Vec<(String, bool)>,
}

pub(crate) enum Activity {
    Idle,
    Streaming(CancellationToken),
    ExecutingTools,
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
            activity: Activity::Idle,
            pending_batch: None,
            inbox: cmd_rx,
            loop_event_tx: cmd_tx.clone(),
            event_tx,
            tool_registry,
            auto_approval: std::sync::Arc::new(AutoApprovalPolicy::new(auto_approved_tools)),
            pending_tool_decisions: Vec::new(),
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
                        if !matches!(self.activity, Activity::Idle) {
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
                        let cancellation = stream.interrupt_handle();
                        self.activity = Activity::Streaming(cancellation);
                        let loop_event_tx = self.loop_event_tx.clone();
                        rt.spawn(async move {
                            info!(session_id = %self.id, "Starting stream");
                            let result = stream.await;
                            info!(session_id = %self.id, "Stream completed");
                            let _ = loop_event_tx.send(LoopEvent::StreamCompletion(result));
                        });
                    }
                    SessionCommand::Interrupt => {
                        if let Activity::Streaming(cancellation) = &self.activity {
                            info!(session_id = %self.id, "Interrupting current stream");
                            cancellation.cancel();
                        } else {
                            warn!(session_id = %self.id, "Received interrupt command while not streaming");
                        }
                    }
                    SessionCommand::ToolDecision { id, approved } => {
                        self.handle_tool_decision(rt.clone(), id, approved);
                    }
                },
                LoopEvent::UserToolResult {
                    agent,
                    batch,
                    result,
                } => match batch.add_result(result.id().to_string(), result) {
                    Ok(either::Either::Left(batch)) => {
                        self.agent = Some(agent);
                        self.pending_batch = Some(batch);
                        self.activity = Activity::Idle;
                    }
                    Ok(either::Either::Right(resolved)) => {
                        self.start_stream_after_tools(rt.clone(), agent, resolved);
                    }
                    Err(error) => {
                        warn!(session_id = %self.id, %error, "Ignoring stale tool result")
                    }
                },
                LoopEvent::AgentToolResults(agent, batch) => {
                    let event_tx = self.event_tx.clone();
                    let stream = match agent.submit_tool_results(batch, move |event| {
                        let _ = event_tx.send(SessionEvent::StreamEvent(event));
                    }) {
                        Ok(stream) => stream,
                        Err(error) => {
                            self.agent = Some(error.agent());
                            self.activity = Activity::Idle;
                            continue;
                        }
                    };
                    let cancellation = stream.interrupt_handle();
                    self.activity = Activity::Streaming(cancellation);
                    let loop_event_tx = self.loop_event_tx.clone();
                    rt.spawn(async move {
                        info!(session_id = %self.id, "Starting stream after tool results");
                        let result = stream.await;
                        let _ = loop_event_tx.send(LoopEvent::StreamCompletion(result));
                    });
                }
                LoopEvent::StreamCompletion(agent) => match agent.status() {
                    AgentStatus::WaitingForUserMessage => {
                        info!(session_id = %self.id, "Session is awaiting user message");
                        self.agent = Some(agent);
                        self.activity = Activity::Idle;
                    }
                    AgentStatus::WaitingForToolResponses => {
                        debug!(session_id = %self.id, "LLM has requested tool calls");
                        let batch = agent
                            .pending_tools()
                            .expect("tool responses must be pending");
                        let approved = self
                            .auto_approval
                            .approves_all(batch.requested().map(|call| call.name.as_str()));
                        if approved {
                            info!(session_id = %self.id, "All tool results can be ran automatically");
                            let loop_event_tx = self.loop_event_tx.clone();
                            let registry = self.tool_registry.clone();
                            self.activity = Activity::ExecutingTools;
                            rt.spawn(async move {
                                let calls = batch.requested().cloned().collect::<Vec<_>>();
                                let results =
                                    futures::future::join_all(calls.into_iter().map(|call| {
                                        let registry = registry.clone();
                                        async move {
                                            info!(
                                                tool_call_id = %call.id,
                                                tool_name = %call.name,
                                                "Invoking tool"
                                            );
                                            let result = match registry.invoke(call.clone()).await {
                                                Ok(result) => provider::ToolResult::new(
                                                    call.id.clone(),
                                                    result.to_string(),
                                                ),
                                                Err(error) => provider::ToolResult::new(
                                                    call.id.clone(),
                                                    error.to_string(),
                                                ),
                                            };
                                            info!(
                                                tool_call_id = %call.id,
                                                tool_name = %call.name,
                                                "Tool invocation finished"
                                            );
                                            result
                                        }
                                    }))
                                    .await;

                                let resolved: ToolBatch<Resolved> = batch
                                    .resolve_all(results)
                                    .expect("auto-approved batch must resolve");
                                let _ = loop_event_tx
                                    .send(LoopEvent::AgentToolResults(agent, resolved));
                            });
                        } else {
                            self.agent = Some(agent);
                            self.pending_batch = Some(batch);
                            self.activity = Activity::Idle;
                            let decisions = std::mem::take(&mut self.pending_tool_decisions);
                            for (id, approved) in decisions {
                                self.handle_tool_decision(rt.clone(), id, approved);
                            }
                        }
                    }
                    AgentStatus::Interrupted => {
                        self.agent = Some(agent);
                        self.activity = Activity::Idle;
                    }
                    AgentStatus::Failed => {
                        self.agent = Some(agent);
                        self.activity = Activity::Idle;
                    }
                },
            }
        }

        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }

    fn handle_tool_decision(&mut self, rt: Handle, id: String, approved: bool) {
        let waiting_for_tools = self
            .agent
            .as_ref()
            .is_some_and(|agent| agent.status() == AgentStatus::WaitingForToolResponses);
        if !waiting_for_tools || self.pending_batch.is_none() {
            self.pending_tool_decisions.push((id, approved));
            return;
        }
        let agent = self.agent.take().expect("agent state was checked");
        let batch = self.pending_batch.take().expect("tool batch was checked");

        if approved {
            let Some(call) = batch.requested().find(|call| call.id == id).cloned() else {
                self.agent = Some(agent);
                self.pending_batch = Some(batch);
                return;
            };
            let registry = self.tool_registry.clone();
            let loop_event_tx = self.loop_event_tx.clone();
            self.activity = Activity::ExecutingTools;
            rt.spawn(async move {
                let content = match registry.invoke(call.clone()).await {
                    Ok(result) => result.to_string(),
                    Err(error) => error.to_string(),
                };
                let _ = loop_event_tx.send(LoopEvent::UserToolResult {
                    agent,
                    batch,
                    result: provider::ToolResult::new(call.id, content),
                });
            });
        } else {
            if !batch.requested().any(|call| call.id == id) {
                self.agent = Some(agent);
                self.pending_batch = Some(batch);
                return;
            }
            let result =
                provider::ToolResult::new(id.clone(), "Tool call denied by user".to_string());
            match batch.add_result(id, result) {
                Ok(either::Either::Left(batch)) => {
                    self.agent = Some(agent);
                    self.pending_batch = Some(batch);
                }
                Ok(either::Either::Right(resolved)) => {
                    self.start_stream_after_tools(rt, agent, resolved)
                }
                Err(error) => {
                    warn!(session_id = %self.id, %error, "Ignoring invalid tool decision");
                }
            }
        }
    }

    fn start_stream_after_tools(&mut self, rt: Handle, agent: Agent, batch: ToolBatch<Resolved>) {
        let event_tx = self.event_tx.clone();
        let stream = match agent.submit_tool_results(batch, move |event| {
            let _ = event_tx.send(SessionEvent::StreamEvent(event));
        }) {
            Ok(stream) => stream,
            Err(error) => {
                self.agent = Some(error.agent());
                self.activity = Activity::Idle;
                return;
            }
        };
        let cancellation = stream.interrupt_handle();
        self.activity = Activity::Streaming(cancellation);
        let loop_event_tx = self.loop_event_tx.clone();
        rt.spawn(async move {
            let result = stream.await;
            let _ = loop_event_tx.send(LoopEvent::StreamCompletion(result));
        });
    }
}
