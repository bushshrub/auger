use crate::SystemPrompt;
use crate::events::{HarnessState, LoopMessage, SessionCommand, SessionEvent};
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::tool_decisions::{Resolved, Resolving, ToolAuthorization, UserToolDecisions};
use crate::tools::tool_registry::ToolRegistry;
use agent_tools::Tool;
use auger_driver::{StreamResult, TypedAgent, WaitingForUserMessage};
use futures::stream;
use provider::{LlmModel, LlmProvider, LlmResponse, LlmStream, StreamEvent, UserPrompt};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{mpsc, Arc};
use std::sync::mpsc::Sender;
use either::Either;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

struct DummyProvider;

impl LlmProvider for DummyProvider {
    fn complete<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _model: &'life1 str,
        _request: provider::LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, provider::LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async {
            Ok(LlmResponse::from(vec![StreamEvent::Done {
                usage: None,
                stop_reason: Some("stop".to_string()),
            }]))
        })
    }

    fn stream<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _model: &'life1 str,
        _request: provider::LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmStream, provider::LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async {
            Ok(LlmStream::new(stream::once(async {
                Ok(StreamEvent::Done {
                    usage: None,
                    stop_reason: Some("stop".to_string()),
                })
            })))
        })
    }
}

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
    loop_event_tx: mpsc::Sender<LoopMessage>,
    event_rx: mpsc::Receiver<SessionEvent>,
}

impl SessionHandle {
    fn new(
        id: SessionId,
        command_tx: mpsc::Sender<LoopMessage>,
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

    pub fn approve_tool_call(&self, id: impl Into<String>) -> Result<(), ()> {
        self.tool_decision(id, true, None)
    }

    pub fn deny_tool_call(&self, id: impl Into<String>) -> Result<(), ()> {
        self.tool_decision(id, false, Some("Denied by user".to_string()))
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

    /// Stop the session.
    pub fn stop(self) {
        todo!()
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
    auto_approval_policy: Arc<AutoApprovalPolicy>,
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
        let tool_registry = Arc::new(tool_registry);
        let tool_defs = tool_registry.list_for_clanker();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let session = Self {
            id: SessionId::new(),
            cmd_rx,
            harness_internal_event_tx: cmd_tx.clone(),
            event_tx,
            tool_registry,
            auto_approval_policy: Arc::new(AutoApprovalPolicy::new(auto_approved_tools)),
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

        let model = LlmModel::new(Arc::new(DummyProvider), "dummy");
        let tools = self.tool_registry.list_for_clanker();
        let init_agent = TypedAgent::<WaitingForUserMessage>::new(
            model,
            "You are a helpful coding agent.".to_string(),
            tools,
        );
        let mut curr_state = HarnessState::WaitingForUserMessage { agent: init_agent };
        for msg in self.cmd_rx.iter() {
            match msg {
                LoopMessage::Cmd(cmd) => {
                    match cmd {
                        SessionCommand::SendMessage(prompt) => {

                            curr_state = match curr_state {
                                HarnessState::WaitingForUserMessage { agent } => {
                                    let new_agent = agent.add_message(prompt);
                                    let inbox_tx = self.harness_internal_event_tx.clone();
                                    // todo: attach event handler function
                                    let stream_fut = new_agent.create_stream();
                                    let cancel = stream_fut.interrupt_handle();
                                    rt.spawn(async move {
                                        let res = stream_fut.await;
                                        inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                    });
                                    HarnessState::Streaming {
                                        cancel,
                                    }}
                                _ => curr_state
                            }

                        }
                        SessionCommand::Interrupt => {
                            curr_state = match curr_state {
                                HarnessState::Streaming { cancel } => {
                                    cancel.cancel();
                                    HarnessState::InterruptingStream
                                }
                                HarnessState::ToolCallsAreRunning { agent, cancel } => {
                                    cancel.cancel();
                                    // TODO: set all tool results to interrupting. should be a field in this variant.
                                    HarnessState::ToolResultsReady { agent }
                                }
                                _ => curr_state
                            }
                        }
                        SessionCommand::ToolDecision { id, approved, message } => {
                            curr_state = match curr_state {
                                HarnessState::NeedToolConsent { agent, user_tool_decisions } => {
                                    match user_tool_decisions.record_decision(id, approved, message) {
                                        Either::Left(not_all_decided) => {
                                            HarnessState::NeedToolConsent {
                                                agent,
                                                user_tool_decisions: not_all_decided
                                            }
                                        }
                                        Either::Right(all_decided) => {
                                            // TODO: rt spawn execution of tools
                                            HarnessState::ToolCallsAreRunning { agent, cancel: CancellationToken::new() }
                                        }
                                    }
                                }
                                _ => curr_state
                            }
                        }
                    }
                }
                LoopMessage::StreamResult(res) => {
                    curr_state = match curr_state {
                        // TODO: HarnessState::StreamInterrupting?
                        HarnessState::Streaming { cancel } => {
                            drop(cancel);
                            match res {
                                StreamResult::Interrupted(agent) => {

                                    // TODO: deal with this...
                                    panic!("stream interrupted but harness state was not in InterruptingStream state")
                                }
                                StreamResult::Failed(agent) => {
                                    // TODO: This will NOT append the partial response? Still TBD how to handle.
                                    let new_agent = make_typeagent_new();
                                    HarnessState::WaitingForUserMessage { agent: new_agent }
                                }
                                StreamResult::WaitingForToolResponses(agent) => {
                                    let tool_batch = agent.get_requested_tools();
                                    if self.auto_approval_policy.will_approve_all(tool_batch.iter().map(|t| t.name.clone())) {
                                        // TODO: rt spawn execution of all tools
                                        HarnessState::ToolCallsAreRunning { agent, cancel: CancellationToken::new() }
                                    } else {
                                        let unapproved = self.auto_approval_policy.ids_needing_consent(tool_batch);
                                        HarnessState::NeedToolConsent { agent, user_tool_decisions: UserToolDecisions::new_undecided(unapproved) }
                                    }
                                }
                                StreamResult::WaitingForUserMessage(agent) => {
                                    HarnessState::WaitingForUserMessage { agent }
                                }
                            }
                        }
                        _ => curr_state
                    };
                }
                LoopMessage::ToolBatchExecutionResult(tool_batch) => {
                    curr_state = match curr_state {
                        HarnessState::ToolCallsAreRunning { agent, cancel } => {
                            drop(cancel);
                            let new_agent = agent.add_all_tool_responses(tool_batch);
                            let stream_fut = new_agent.create_stream();
                            let cancel = stream_fut.interrupt_handle();
                            let inbox_tx = self.harness_internal_event_tx.clone();
                            rt.spawn(async move {
                                // TODO: Attach stream delta emit pipe.
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

}

// fake temp function to compile
fn make_typeagent_new() -> TypedAgent<WaitingForUserMessage> {
    todo!()
}
