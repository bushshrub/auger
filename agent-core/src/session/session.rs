use mpsc::Receiver;
use std::env::current_dir;
use crate::SystemPrompt;
use crate::events::{LoopMessage, SessionCommand, SessionEvent};
use crate::tools::auto_approval::AutoApprovalPolicies;
use crate::tools::tool_decisions::{ToolAuthorization, UserToolDecisions};
use crate::tools::tool_execution::ToolExecution;
use crate::tools::tool_registry::ToolRegistry;
use agent_tools::Tool;
use auger_driver::{restore, RestoreState, RestoredAgent, StreamResult, TypedAgent, WaitingForUserMessage};
use provider::{LlmError, LlmModel, Message, ToolDefinition, UserPrompt};
use std::fmt;
use std::sync::{mpsc, Arc};
use std::sync::mpsc::Sender;
use chrono::{DateTime, Utc};
use either::Either;
use getset::CopyGetters;
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tracing::{debug, error, info, warn};
use crate::session::history::{AssistantStatus, AuthorizationSource, EventId, ModelInfo, RecordableEvent, RecordableTurn, SessionRecord};
use crate::session::states::HarnessState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
#[derive(Clone, CopyGetters)]
pub struct SessionHandle {
    #[get_copy = "pub"]
    id: SessionId,
    loop_event_tx: Sender<LoopMessage>,

    // TODO: This REALLY does not belong here.
    // But SessionRecord is owned by Session...
    #[get_copy = "pub"]
    created_at: DateTime<Utc>
}

impl SessionHandle {
    fn new(id: SessionId, command_tx: Sender<LoopMessage>, created_at: DateTime<Utc>) -> Self {
        Self {
            id,
            loop_event_tx: command_tx,
            created_at,
        }
    }

    pub fn send_command(&self, cmd: SessionCommand) -> Result<(), ()> {
        self.loop_event_tx
            .send(LoopMessage::Cmd(cmd))
            .map_err(|_| ())
    }

}



pub struct Session {
    id: SessionId,
    /// Receiver to receive session commands and agent events from
    cmd_rx: Receiver<LoopMessage>,
    harness_internal_event_tx: Sender<LoopMessage>,
    /// Sender for the session to emit events through
    event_tx: Sender<SessionEvent>,
    tool_registry: Arc<ToolRegistry>,
    auto_approval_policies: Arc<AutoApprovalPolicies>,
    record: SessionRecord,
}

impl Session {
    pub fn start(
        model: LlmModel,
        system_prompt: SystemPrompt,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approval_policies: impl Into<AutoApprovalPolicies>
    ) -> (SessionHandle, Receiver<SessionEvent>) {
        let id = SessionId::new();
        let model_name = model.name().to_string();
        Self::start_from(
            model,
            // TODO: modelinfo
            SessionRecord::new(id, current_dir().expect("no cwd"), ModelInfo::new("to-be-added".to_string(), model_name)),
            system_prompt,
            rt,
            tools,
            auto_approval_policies.into(),
        )
    }

    /// Restore a session from a SessionRecord
    pub fn start_from(
        model: LlmModel,
        record: SessionRecord,
        system_prompt: SystemPrompt,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approval_policies: impl Into<AutoApprovalPolicies>,
    ) -> (SessionHandle, Receiver<SessionEvent>) {
        Self::spawn(rt, system_prompt, record, model, tools, auto_approval_policies.into())
    }

    fn create_initial_agent(system_prompt: SystemPrompt, record: &SessionRecord, model: LlmModel, tools: Vec<ToolDefinition>) -> RestoredAgent {
        let last_turn = record.get_previous_turn();
        let restore_state = match last_turn {
            Some(turn) => {
                match turn.turn() {
                    RecordableTurn::InputMessage { content } => {
                        panic!("Can't start on a user turn")
                    }
                    RecordableTurn::AssistantMessage { status, content } => {
                        match status {
                            AssistantStatus::Completed => {
                                let mut messages = record.as_messages();
                                messages.insert(0, system_prompt.into());
                                RestoreState::from_messages(messages)
                            }
                            AssistantStatus::Interrupted => {
                                let mut messages = record.as_messages();
                                messages.insert(0, system_prompt.into());
                                RestoreState::Interrupted {
                                    messages,
                                    events: Vec::new() // TODO: insert actual interrupted partial message
                                }
                            }
                            AssistantStatus::Failed => {
                                let mut messages = record.as_messages();
                                messages.insert(0, system_prompt.into());
                                RestoreState::Failed {
                                    messages,
                                    events: Vec::new(),
                                    error: LlmError { message: "fake error".to_string() } // TODO: real error
                                }
                            }
                        }
                    }
                }
            }
            None => {
                RestoreState::WaitingForUserMessage {
                    messages: vec![system_prompt.into()],
                }
            }
        };
        restore(model, tools, restore_state)
    }


    fn spawn(
        rt: Handle,
        system_prompt: SystemPrompt,
        record: SessionRecord,
        model: LlmModel,
        tools: Vec<Box<dyn Tool>>,
        auto_approval_policies: AutoApprovalPolicies,
    ) -> (SessionHandle, Receiver<SessionEvent>) {
        let id = record.session_id();
        let creation_time = record.created_at();
        let mut tool_registry = ToolRegistry::new();
        for tool in tools {
            tool_registry.register(tool);
        }
        let tool_registry = Arc::new(tool_registry);
        let llm_tools = tool_registry.list_for_clanker();
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let session = Self {
            id,
            cmd_rx,
            harness_internal_event_tx: cmd_tx.clone(),
            event_tx,
            tool_registry,
            auto_approval_policies: Arc::new(auto_approval_policies),
            record
        };
        let handle = SessionHandle::new(session.id, cmd_tx.clone(), creation_time);

        let initial_agent = Self::create_initial_agent(system_prompt, &session.record, model, llm_tools);

        std::thread::Builder::new()
            .name(format!("auger-session-{}", session.id.0))
            .spawn(move || session.run(rt, initial_agent))
            .expect("failed to spawn session thread");

        (handle, event_rx)
    }

    fn run(mut self, rt: Handle, init_agent: RestoredAgent) {
        info!(session_id = %self.id, "Session started");
        let mut curr_state = init_agent.into();
        'session_loop: while let Ok(msg) = self.cmd_rx.recv() {
            match msg {
                LoopMessage::Cmd(cmd) => {
                    match cmd {
                        SessionCommand::Stop { reply_tx } => {
                            let _ = reply_tx.send(());
                            break 'session_loop;
                        }
                        SessionCommand::Snapshot { reply_tx } => {
                            let _ = reply_tx.send(self.record.clone());
                        }
                        SessionCommand::SendMessage(prompt) => {
                            info!(session_id = %self.id, "Received user message {:?}", prompt);
                            let new_agent = match curr_state {
                                HarnessState::WaitingForUserMessage { agent } => {
                                    agent.add_message(prompt.clone())
                                }
                                HarnessState::StreamingInterrupted { agent } => {
                                    agent.add_message_to_continue(prompt.clone(), true)
                                }
                                HarnessState::StreamingFailed { agent } => {
                                    agent.add_message_to_continue(prompt.clone())
                                }
                                HarnessState::InterruptingStream { pending_message: None } => {
                                    curr_state = HarnessState::InterruptingStream {
                                        pending_message: Some(prompt),
                                    };
                                    return;
                                }
                                other => {
                                    warn!(session_id = %self.id, command = "send_message", "Ignoring command in invalid harness state");
                                    curr_state = other;
                                    return;
                                }
                            };

                            let event_tx = self.event_tx.clone();
                            let inbox_tx = self.harness_internal_event_tx.clone();
                            let stream_fut = new_agent.create_stream(move |event| {
                                let _ = event_tx.send(SessionEvent::StreamEvent(event));
                            });
                            let cancel = stream_fut.interrupt_handle();
                            let sess_id = self.id;

                            self.record.record_turn(RecordableTurn::user_prompt(prompt.into())).expect("previous turn to have been assistant/session start");
                            rt.spawn(async move {
                                info!(session_id=%sess_id, "Starting stream");
                                let res = stream_fut.await;
                                inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                            });
                            curr_state = HarnessState::Streaming { cancel };
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
                                        self.record.
                                            get_previous_turn_mut()
                                            .expect("there to be a previous turn")
                                            .record_tool_decision(id.clone().into(), approved, AuthorizationSource::User, message.clone())
                                            .expect("previous turn to be assistant");
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
                                    // invalid state - unrecoverable.
                                    panic!("stream returned interrupted while harness was still streaming")
                                }
                                StreamResult::Failed(agent) => {
                                    warn!(
                                        session_id = %self.id,
                                        error = %agent.error(),
                                        "LLM stream failed; waiting for a new user message"
                                    );
                                    self.record.record_turn(RecordableTurn::AssistantMessage {
                                        status: AssistantStatus::Failed,
                                        content: vec![],
                                    }).expect("previous turn was user");
                                    let _ = self.event_tx.send(SessionEvent::StreamError {
                                        error: agent.error().to_string(),
                                    });
                                    HarnessState::StreamingFailed { agent }
                                }
                                StreamResult::WaitingForToolResponses(agent) => {
                                    debug!(session_id = %self.id, "agent has called tools");
                                    let assistant_message = agent.messages().last().expect("there to be a last message").clone();
                                    let turn_id = self.record.
                                        record_turn(
                                            RecordableTurn::assistant_message(AssistantStatus::Completed, assistant_message)
                                            .expect("assistant message should be Assistant variant")
                                        )
                                        .expect("last turn to be user");

                                    let current_turn = self.record.get_turn_mut(&turn_id).expect("turn to exist");
                                    let tool_batch = agent.get_requested_tools();
                                    if self.auto_approval_policies.will_approve_all(&tool_batch) {
                                        info!(session_id=%self.id, "automatically running all tools");
                                        for call in &tool_batch {
                                            current_turn.
                                                record_tool_decision(call.id.clone().into(), true, AuthorizationSource::Policy, None)
                                                .expect("turn to be assistant");
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
                                                current_turn.
                                                    record_tool_decision(call.id.clone().into(), true, AuthorizationSource::Policy, None)
                                                    .expect("turn to be assistant");
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
                                    let assistant_message = agent.messages().last().expect("there to be a last message").clone();
                                    self.record.
                                        record_turn(
                                            RecordableTurn::assistant_message(AssistantStatus::Completed, assistant_message)
                                                .expect("assistant message should be Assistant variant")
                                        )
                                        .expect("last turn to be user");
                                    HarnessState::WaitingForUserMessage { agent }
                                }
                            }
                        }
                        HarnessState::InterruptingStream { pending_message } => match res {
                            StreamResult::Interrupted(agent) => {
                                match pending_message {
                                    Some(prompt) => {
                                        let event_tx = self.event_tx.clone();
                                        let new_agent = agent
                                            .add_message_to_continue(prompt.clone(), true);
                                        let inbox_tx = self.harness_internal_event_tx.clone();
                                        let stream_fut = new_agent.create_stream(move |event| {
                                            let _ = event_tx.send(SessionEvent::StreamEvent(event));
                                        });
                                        let cancel = stream_fut.interrupt_handle();
                                        self.record.record_turn(RecordableTurn::user_prompt(prompt.into())).expect("last turn was assistant");
                                        rt.spawn(async move {
                                            let res = stream_fut.await;
                                            inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                                        });
                                        HarnessState::Streaming { cancel }
                                    }
                                    None => {
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
                    let agent = match curr_state {
                        HarnessState::ToolCallsAreRunning { agent, cancel } => {
                            drop(cancel);
                            agent
                        }
                        HarnessState::InterruptingToolExecution { agent } => {
                            agent
                        }
                        other => {
                            curr_state = other;
                            return;
                        }
                    };

                    for result in tool_batch.results() {
                        self.record.get_previous_turn_mut().expect("for there to be a previous turn")
                            .record_tool_result(result.clone())
                            .expect("previous turn to be assistant");
                    }
                    // TODO: allow steering message to ride along
                    let new_agent = agent.add_all_tool_responses(None, tool_batch);
                    let event_tx = self.event_tx.clone();
                    let stream_fut = new_agent.create_stream(move |event| {
                        let _ = event_tx.send(SessionEvent::StreamEvent(event));
                    });
                    let cancel = stream_fut.interrupt_handle();
                    let inbox_tx = self.harness_internal_event_tx.clone();

                    rt.spawn(async move {
                        let res = stream_fut.await;
                        inbox_tx.send(LoopMessage::StreamResult(res)).expect("inbox_rx was dropped");
                    });
                    curr_state = HarnessState::Streaming { cancel };
                }
            }
        }


        info!(session_id = %self.id, "Session exited");
        let _ = self.event_tx.send(SessionEvent::Closed);
    }


}
