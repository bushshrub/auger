//! Typestate session automaton.
//!
//! Each session runs on its own OS thread; async is confined to `block_on`
//! at the provider/tool boundary. The driver (`Session::run`) only ever
//! holds a resting state (`Parked`); the active states (`LlmTurn`,
//! `AgentTurn`) run inline and poll the command channel at checkpoints
//! (between stream events, between tool calls) so `Snapshot` and
//! `Interrupt` are answered mid-turn.
//!
//! See `session-state.mmd` for the state diagram.

use crate::session::SessionId;
use crate::session::events::{
    ClankerEvent, LifecycleEvent, SessionEvent, StateKind, ToolCallEvent, UserAction, UserCommand,
};
use crate::session::handle::SessionHandle;
use crate::system_prompt::SystemPrompt;
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::tool_call_batch::{Complete, Resolving, ToolCallBatch};
use crate::tools::tool_registry::ToolRegistry;
use either::Either;
use futures::StreamExt;
use provider::LlmModel;
use provider::thread::{ClankerTurn, ToolResultsPending, UserTurn};
use provider::{ClankerMessage, LlmResponse, LlmThread, Message, StreamEvent, UserPrompt};
use std::sync::mpsc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tracing::{error, info};
use uuid::Uuid;

/// Shared services threaded through every state.
struct Ctx {
    id: SessionId,
    model: LlmModel,
    tools: ToolRegistry,
    policy: AutoApprovalPolicy,
    /// Event channel. Session should send events through the channel.
    events: broadcast::Sender<SessionEvent>,
    /// Tokio runtime handle for the async provider/tool boundary.
    rt: Handle,
}

impl Ctx {
    /// Emit a session event; a full/lagging channel is not our problem.
    fn emit(&self, event: impl Into<SessionEvent>) {
        let _ = self.events.send(event.into());
    }
}

// ── States ──────────────────────────────────────────────────────────────

/// Session is waiting for a user to send a message.
pub struct WaitingForUserMessage {
    thread: LlmThread<UserTurn>,
}

/// Session is waiting for user input to approve tool calls.
struct WaitingForUserToolApproval {
    thread: LlmThread<ToolResultsPending>,
    /// Batch of tool calls that still need to be approved.
    /// Note that any tool calls which the harness was able
    /// to execute autonomously will have their responses already added here.
    batch: ToolCallBatch<Resolving>,
}

/// The turn was interrupted during an agent turn.
/// Note that if interrupted during an agent turn,
/// all tool calls will be resolved as interrupted.
///
struct Interrupted {
    thread: LlmThread<ToolResultsPending>,
    batch: ToolCallBatch<Complete>,
}

/// Session is executing tool calls.
/// This is a transient state: the session will return to LlmTurn after
/// all tool calls finish. If the user interrupts, it will switch
/// to the interrupted state instead.
struct AgentTurn {
    thread: LlmThread<ToolResultsPending>,
}

/// The state in which the LLM is streaming a response. Transient.
struct LlmTurn {
    thread: LlmThread<ClankerTurn>,
}

/// Represents an active session.
pub struct Session<S> {
    ctx: Ctx,
    state: S,
}

/// Parked session state that requires user action
enum Parked {
    Ready(Session<WaitingForUserMessage>),
    Approving(Session<WaitingForUserToolApproval>),
    Interrupted(Session<Interrupted>),
    /// Unrecoverable; the driver exits.
    Dead,
}

// ── Entry points ────────────────────────────────────────────────────────

impl Session<WaitingForUserMessage> {
    /// Begin a new auger session.
    ///
    /// This creates the session state machine.
    pub fn new(
        prompt: SystemPrompt,
        model: LlmModel,
        events: broadcast::Sender<SessionEvent>,
        rt: Handle,
    ) -> Self {
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(builtin_tools::Dummy {}));
        tools.register(Box::new(builtin_tools::ReadFile {}));
        tools.register(Box::new(builtin_tools::ListFiles {}));
        tools.register(Box::new(builtin_tools::Grep {}));
        tools.register(Box::new(builtin_tools::Glob {}));
        tools.register(Box::new(builtin_tools::WriteFile {}));
        tools.register(Box::new(builtin_tools::EditFile {}));
        tools.register(Box::new(builtin_tools::Shell {}));
        tools.register(Box::new(builtin_tools::WebSearch::new()));
        tools.register(Box::new(builtin_tools::FetchContent::new()));
        tools.register(Box::new(builtin_tools::WebFetch::new()));
        tools.register(Box::new(builtin_tools::WebFetchText::new()));
        tools.register(Box::new(builtin_tools::TodoList::new()));

        let auto_approved_defaults = [
            "read_file",
            "list_files",
            "grep",
            "glob",
            "todo_list",
            "web_search",
        ];

        Session {
            ctx: Ctx {
                id: Uuid::new_v4(),
                model,
                tools,
                policy: AutoApprovalPolicy::new(
                    auto_approved_defaults.iter().map(|s| s.to_string()),
                ),
                events,
                rt,
            },
            state: WaitingForUserMessage {
                thread: LlmThread::new(prompt.into()),
            },
        }
    }

    /// Spawn the session on its own OS thread and return a handle to it.
    pub fn spawn(prompt: SystemPrompt, model: LlmModel) -> SessionHandle {
        let (cmds_tx, cmds_rx) = mpsc::channel();
        let (events_tx, _) = broadcast::channel(32);
        let model_name = model.name().to_string();

        let session = Session::new(prompt, model, events_tx.clone(), Handle::current());
        let id = session.ctx.id;

        std::thread::Builder::new()
            .name(format!("session-{id}"))
            .spawn(move || session.run(cmds_rx))
            .expect("failed to spawn session thread");
        SessionHandle::new(id, model_name, cmds_tx, events_tx)
    }

    /// The main agentic loop.
    ///
    /// # Design
    /// This utilizes a state machine to make things easy to reason about.
    /// The session is always in one of a few states, and the user can only
    /// send commands that are valid in that state. The session will emit
    /// events to the event channel as it transitions between states.
    fn run(self, rx: mpsc::Receiver<UserCommand>) {
        info!(session_id = %self.ctx.id, "Starting session");
        let mut state = Parked::Ready(self);
        loop {
            let Ok(cmd) = rx.recv() else { break }; // server hung up
            state = match (state, cmd) {
                // Session is ready and we received a user message
                (Parked::Ready(sess), UserCommand::Action(UserAction::SendMessage(msg))) => {
                    let Session { ctx, state } = sess;
                    let thread = state.thread.add_user_message(msg.into());
                    pump(ctx, thread, &rx)
                }
                // The session is waiting for the user to approve tool calls, and we received a response to one of those calls.
                (
                    Parked::Approving(sess),
                    UserCommand::Action(UserAction::RespondToToolCall {
                        response,
                        tool_call_id,
                        message,
                    }),
                ) => {
                    todo!(
                        "approve: run the call / deny: record reason; batch complete -> add_tool_results -> pump"
                    )
                }
                // The session is waiting for the user to approve tool calls, and the user wants to steer.
                (Parked::Approving(sess), UserCommand::Action(UserAction::SendMessage(msg))) => {
                    todo!("add_steering_message; stay Approving")
                }
                (Parked::Approving(sess), UserCommand::Action(UserAction::Interrupt)) => {
                    todo!("resolve remaining calls as interrupted -> Parked::Interrupted")
                }

                (Parked::Interrupted(sess), UserCommand::Action(UserAction::SendMessage(msg))) => {
                    todo!("add_steering_message -> add_tool_results(batch.drain()) -> pump")
                }

                (Parked::Dead, _) => break,
                (state, UserCommand::Snapshot { reply }) => {
                    todo!("reply with the current thread's messages; return `state` unchanged")
                }
                // Command not legal in this state: reject visibly, keep the state.
                (state, cmd) => {
                    todo!("emit LifecycleEvent::CommandRejected; return `state` unchanged")
                }
            };
            // Pump exits emit their own StateChanged (via the `Parked`
            // constructors); arms that transition without pumping emit
            // theirs inline.
            if matches!(state, Parked::Dead) {
                break;
            }
        }
        info!("Session has been closed");
        // TODO: emit LifecycleEvent::Closed.
    }
}

// ── Pump ────────────────────────────────────────────────────────────────
//
// The active states (`LlmTurn`, `AgentTurn`) never reach the driver: once
// a user message puts the thread in `ClankerTurn`, `pump` alternates
// `run_llm_turn` / `run_agent_turn` until the machine parks.

/// Outcome of streaming one model response.
enum LlmTurnOutcome {
    /// The reply had no tool calls; back to waiting for the user.
    Ready(LlmThread<UserTurn>),
    /// The reply requested tool calls.
    ToolsRequested(LlmThread<ToolResultsPending>),
    /// The user interrupted (or the provider failed) mid-generation; the
    /// partial reply, which had no tool calls, is recorded on the thread.
    Interrupted(LlmThread<UserTurn>),
    /// Interrupted mid-generation with tool calls in the partial reply.
    InterruptedWithCalls(LlmThread<ToolResultsPending>),
    /// The command channel hung up; unrecoverable.
    Dead,
}

/// Outcome of executing the auto-approved calls of one tool batch.
enum AgentTurnOutcome {
    /// Every call resolved; results submitted, the model's turn again.
    Resubmit(LlmThread<ClankerTurn>),
    /// Gated calls remain after the auto-approved ones ran.
    NeedsApproval {
        thread: LlmThread<ToolResultsPending>,
        batch: ToolCallBatch<Resolving>,
    },
    /// Interrupted between calls; remaining calls resolved as interrupted,
    /// results not yet submitted.
    Interrupted {
        thread: LlmThread<ToolResultsPending>,
        batch: ToolCallBatch<Complete>,
    },
    /// The command channel hung up or a batch invariant broke; unrecoverable.
    Dead,
}

/// Drive the LlmTurn/AgentTurn cycle until the session parks.
fn pump(ctx: Ctx, mut thread: LlmThread<ClankerTurn>, rx: &mpsc::Receiver<UserCommand>) -> Parked {
    ctx.emit(LifecycleEvent::StateChanged {
        state: StateKind::Generating,
    });
    loop {
        let pending = match run_llm_turn(&ctx, thread, rx) {
            LlmTurnOutcome::Ready(thread) | LlmTurnOutcome::Interrupted(thread) => {
                ctx.emit(LifecycleEvent::StateChanged {
                    state: StateKind::Ready,
                });
                return Parked::Ready(Session {
                    ctx,
                    state: WaitingForUserMessage { thread },
                });
            }
            LlmTurnOutcome::InterruptedWithCalls(thread) => {
                let batch = ToolCallBatch::new_batch(thread.get_pending_tool_calls());
                for call in batch.requested() {
                    ctx.emit(ToolCallEvent::Interrupted {
                        id: call.id.clone(),
                    });
                }
                let batch = batch.interrupt_remaining();
                ctx.emit(LifecycleEvent::StateChanged {
                    state: StateKind::Interrupted,
                });
                return Parked::Interrupted(Session {
                    ctx,
                    state: Interrupted { thread, batch },
                });
            }
            LlmTurnOutcome::ToolsRequested(thread) => thread,
            LlmTurnOutcome::Dead => return Parked::Dead,
        };

        thread = match run_agent_turn(&ctx, pending, rx) {
            AgentTurnOutcome::Resubmit(thread) => thread,
            AgentTurnOutcome::NeedsApproval { thread, batch } => {
                ctx.emit(LifecycleEvent::StateChanged {
                    state: StateKind::AwaitingApproval,
                });
                return Parked::Approving(Session {
                    ctx,
                    state: WaitingForUserToolApproval { thread, batch },
                });
            }
            AgentTurnOutcome::Interrupted { thread, batch } => {
                ctx.emit(LifecycleEvent::StateChanged {
                    state: StateKind::Interrupted,
                });
                return Parked::Interrupted(Session {
                    ctx,
                    state: Interrupted { thread, batch },
                });
            }
            AgentTurnOutcome::Dead => return Parked::Dead,
        };
    }
}

fn run_llm_turn(
    ctx: &Ctx,
    thread: LlmThread<ClankerTurn>,
    rx: &mpsc::Receiver<UserCommand>,
) -> LlmTurnOutcome {
    let request = thread.create_request(ctx.tools.list_for_clanker());
    let mut stream = match ctx.rt.block_on(ctx.model.stream(request)) {
        Ok(stream) => stream,
        Err(err) => {
            ctx.emit(LifecycleEvent::ProviderError {
                message: err.to_string(),
            });
            let response = LlmResponse::from(Vec::new());
            return match thread.add_clanker_reply(response.into()) {
                Either::Left(thread) => LlmTurnOutcome::Interrupted(thread),
                Either::Right(thread) => LlmTurnOutcome::InterruptedWithCalls(thread),
            };
        }
    };
    let mut events = Vec::new();

    loop {
        match rx.try_recv() {
            Ok(UserCommand::Snapshot { reply }) => {
                let _ = reply.send(thread.messages().to_vec());
            }
            Ok(UserCommand::Action(UserAction::Interrupt)) => {
                let response = LlmResponse::from(events);
                return match thread.add_clanker_reply(response.into()) {
                    Either::Left(thread) => LlmTurnOutcome::Interrupted(thread),
                    Either::Right(thread) => LlmTurnOutcome::InterruptedWithCalls(thread),
                };
            }
            Ok(cmd) => {
                ctx.emit(LifecycleEvent::CommandRejected {
                    reason: format!("command not allowed while generating: {cmd:?}"),
                });
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => return LlmTurnOutcome::Dead,
        }

        let Some(event) = ctx.rt.block_on(stream.next()) else {
            break;
        };
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                ctx.emit(LifecycleEvent::ProviderError {
                    message: err.to_string(),
                });
                let response = LlmResponse::from(events);
                return match thread.add_clanker_reply(response.into()) {
                    Either::Left(thread) => LlmTurnOutcome::Interrupted(thread),
                    Either::Right(thread) => LlmTurnOutcome::InterruptedWithCalls(thread),
                };
            }
        };

        match &event {
            StreamEvent::TextDelta(delta) => {
                ctx.emit(ClankerEvent::ContentDelta {
                    delta: delta.clone(),
                });
            }
            StreamEvent::ReasoningDelta(delta) => {
                ctx.emit(ClankerEvent::ReasoningDelta {
                    delta: delta.clone(),
                });
            }
            StreamEvent::ToolCall { .. } => {}
            StreamEvent::ToolCallComplete {
                id,
                name,
                arguments,
            } => {
                ctx.emit(ClankerEvent::ToolCallRequest {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                });
            }
            StreamEvent::Done { usage, stop_reason } => {
                ctx.emit(ClankerEvent::Done {
                    usage: usage.clone(),
                    stop_reason: stop_reason.clone(),
                });
            }
        }
        events.push(event);
    }

    let response = LlmResponse::from(events);
    match thread.add_clanker_reply(response.into()) {
        Either::Left(thread) => LlmTurnOutcome::Ready(thread),
        Either::Right(thread) => LlmTurnOutcome::ToolsRequested(thread),
    }
}

fn run_agent_turn(
    ctx: &Ctx,
    thread: LlmThread<ToolResultsPending>,
    rx: &mpsc::Receiver<UserCommand>,
) -> AgentTurnOutcome {
    let mut thread = thread;
    let mut batch = ToolCallBatch::new_batch(thread.get_pending_tool_calls());

    loop {
        match rx.try_recv() {
            Ok(UserCommand::Snapshot { reply }) => {
                let _ = reply.send(thread.messages().to_vec());
            }
            Ok(UserCommand::Action(UserAction::SendMessage(msg))) => {
                thread = thread.add_steering_message(msg.into());
            }
            Ok(UserCommand::Action(UserAction::Interrupt)) => {
                for call in batch.requested() {
                    ctx.emit(ToolCallEvent::Interrupted {
                        id: call.id.clone(),
                    });
                }
                return AgentTurnOutcome::Interrupted {
                    thread,
                    batch: batch.interrupt_remaining(),
                };
            }
            Ok(cmd) => {
                ctx.emit(LifecycleEvent::CommandRejected {
                    reason: format!("command not allowed while running tools: {cmd:?}"),
                });
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => return AgentTurnOutcome::Dead,
        }

        let approved_calls = batch
            .requested()
            .filter(|call| ctx.policy.is_approved(&call.name))
            .cloned()
            .collect::<Vec<_>>();
        if approved_calls.is_empty() {
            return AgentTurnOutcome::NeedsApproval { thread, batch };
        }

        for call in &approved_calls {
            ctx.emit(ToolCallEvent::AutoApproved {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
        }

        let results = ctx.rt.block_on(async {
            futures::future::join_all(approved_calls.into_iter().map(|call| async move {
                let result = match ctx.tools.invoke(call.clone()).await {
                    Ok(result) => result,
                    Err(err) => agent_tools::ToolCallResult::error(err.to_string()),
                };
                (call, result)
            }))
            .await
        });

        for (call, result) in results {
            if result.is_error() {
                ctx.emit(ToolCallEvent::Error {
                    id: call.id.clone(),
                    error: result.to_string(),
                });
            } else {
                ctx.emit(ToolCallEvent::Result {
                    id: call.id.clone(),
                    result: result.to_string(),
                });
            }

            match batch.resolve(&call.id, result) {
                Ok(Either::Left(next_batch)) => {
                    batch = next_batch;
                }
                Ok(Either::Right(complete)) => {
                    return match thread.add_tool_results(complete.drain()) {
                        Ok(thread) => AgentTurnOutcome::Resubmit(thread),
                        Err(err) => {
                            error!(error = %err, "failed to add tool results to thread");
                            AgentTurnOutcome::Dead
                        }
                    };
                }
                Err(err) => {
                    error!(error = %err, "failed to resolve tool call batch");
                    return AgentTurnOutcome::Dead;
                }
            }
        }
    }
}
