//! Typestate session automaton, replacing `session_loop.rs`.
//!
//!
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use provider::{LlmModel, LlmThread, ToolResult};
use provider::thread::{ToolResultsPending, UserTurn};
use crate::session::SessionId;
use std::sync::mpsc;
use either::Either;
use tracing::{info, warn};
use crate::session::events::{SessionEvent, ToolCallResponse, UserAction, UserCommand, UserMessage};
use crate::tools::auto_approval::AutoApprovalPolicy;
use crate::tools::tool_call_batch::{Resolving, ToolCallBatch};
use crate::tools::tool_registry::ToolRegistry;

/// Shared services threaded through every state.
struct Ctx {
    id: SessionId,
    model: LlmModel,
    tools: ToolRegistry,
    policy: AutoApprovalPolicy,
    events: broadcast::Sender<SessionEvent>,
    /// Flag to interrupt the session loop
    cancel: Arc<AtomicBool>,
    /// Tokio runtime used for executing tools.
    rt: Handle,
}

/// Session is waiting for a user to send a message.
struct Ready {
    thread: LlmThread<UserTurn>,
}

/// Session is waiting for user input to approve tool calls.
struct Approving {
    thread: LlmThread<ToolResultsPending>,
    /// Batch of tool calls that still need to be approved.
    /// Any auto approved calls will already be stored in this batch.
    batch: ToolCallBatch<Resolving>,
}

struct Session<S> {
    ctx: Ctx,
    state: S,
}

/// Type-erased resting state held by the driver loop between commands.
enum Outcome {
    Ready(Session<Ready>),
    Approving(Session<Approving>),
    Dead,
}

/// Driver-level conversion: transitions return precisely-typed sessions via
/// `Either`; only the driver erases them for storage between commands.
impl From<Either<Session<Ready>, Session<Approving>>> for Outcome {
    fn from(parked: Either<Session<Ready>, Session<Approving>>) -> Self {
        match parked {
            Either::Left(s) => Outcome::Ready(s),
            Either::Right(s) => Outcome::Approving(s),
        }
    }
}

/// Driver loop: parks on the command channel, dispatches each command
/// against the current resting state, and holds the returned next state.
/// This is the only place where a `(state, command)` mismatch can exist.
fn run(initial: Outcome, rx: mpsc::Receiver<UserCommand>) {
    let mut state = initial;
    while let Ok(cmd) = rx.recv() {
        state = match (state, cmd) {
            (Outcome::Dead, _) => break,

            (Outcome::Ready(sess), UserCommand::Action(UserAction::SendMessage(msg))) => {
                todo!("sess.send(msg).into()")
            }

            (
                Outcome::Approving(sess),
                UserCommand::Action(UserAction::RespondToToolCall { response, tool_call_id, message }),
            ) => {
                todo!("sess.decide(&tool_call_id, response, message).into()")
            }

            (state, UserCommand::Snapshot { reply }) => {
                todo!("reply with the current thread's messages; return `state` unchanged")
            }

            // Command not legal in this state: reject visibly, keep the state.
            (state, cmd) => {
                todo!("emit a rejected event for `cmd`; return `state` unchanged")
            }
        };
    }
    info!("Session has been closed");
}

impl Session<Ready> {
    /// Append the user message and drive the turn cycle until the machine
    /// parks again: `Left` if the turn completed (or aborted), `Right` if
    /// gated tool calls await approval.
    fn send(self, msg: UserMessage) -> Either<Session<Ready>, Session<Approving>> {
        let _ = msg;
        todo!("step 5: hand off to run_turn_cycle")
    }
}

impl Session<Approving> {
    /// Resolve one pending tool call. `Right` while calls remain pending;
    /// once the batch completes, submits all results and re-enters the turn
    /// cycle, parking wherever it ends (`Left` or `Right`).
    fn decide(self, tool_call_id: &str, response: ToolCallResponse, message: Option<String>) -> Either<Session<Ready>, Session<Approving>> {
        let _ = (tool_call_id, response, message);
        todo!("step 6: resolve one call via the batch")
    }
}
