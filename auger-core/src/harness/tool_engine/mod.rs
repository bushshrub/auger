//! Tool phase engine: runs the tool calls of one assistant turn until every
//! call has an answer.

pub(crate) mod registry;
pub(crate) mod result;

use crate::harness::tool_engine::registry::ToolRegistry;
use crate::harness::tool_engine::result::ToolCallResult;
use crate::harness::tool_engine::result::ToolOutcome;
use auger_driver::Resolved;
use auger_driver::ToolBatch;
use auger_driver::Unresolved;
use provider::ToolCallRequest;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;

const TOOL_TIMEOUT: Duration = Duration::from_secs(300);

/// Delivers a [`ToolMessage`] back to whoever owns the engine.
pub(crate) type ToolMessageSender = Arc<dyn Fn(ToolMessage) + Send + Sync>;

/// A message from a spawned tool task. Opaque outside the engine.
pub(crate) struct ToolMessage {
    call_id: String,
    outcome: ToolOutcome,
}

enum CallState {
    Running,
    Done(ToolOutcome),
}

pub(crate) struct ToolPhaseEngine {
    batch: ToolBatch<Unresolved>,
    calls: HashMap<String, CallState>,
    registry: Arc<ToolRegistry>,
    rt: Handle,
    send: ToolMessageSender,
}

pub(crate) enum Phase {
    /// Some calls have no answer yet.
    Pending(ToolPhaseEngine),
    /// Every call has an answer.
    Done(ToolBatch<Resolved>),
}

pub(crate) struct Received {
    /// The call that finished, if the message was not dropped.
    pub(crate) finished: Option<ToolCallResult>,
    pub(crate) phase: Phase,
}

impl ToolPhaseEngine {
    /// Spawn every call in `batch`.
    pub(crate) fn start(
        batch: ToolBatch<Unresolved>,
        registry: Arc<ToolRegistry>,
        rt: &Handle,
        send: ToolMessageSender,
    ) -> Phase {
        let calls = batch.calls().to_vec();
        let mut engine = Self {
            batch,
            calls: HashMap::new(),
            registry,
            rt: rt.clone(),
            send,
        };
        for call in calls {
            engine.run(call);
        }
        engine.into_phase()
    }

    /// Apply a message from a tool task.
    pub(crate) fn receive(mut self, msg: ToolMessage) -> Received {
        let finished = match self.calls.get_mut(&msg.call_id) {
            Some(state @ CallState::Running) => {
                *state = CallState::Done(msg.outcome.clone());
                Some(ToolCallResult {
                    tool_call_id: msg.call_id.into(),
                    outcome: msg.outcome,
                })
            }
            _ => None,
        };
        Received {
            finished,
            phase: self.into_phase(),
        }
    }

    /// Mark `call` running and spawn it. The task always sends exactly one
    /// message, also on error, timeout and panic.
    fn run(&mut self, call: ToolCallRequest) {
        let call_id = call.id.clone();
        self.calls.insert(call_id.clone(), CallState::Running);
        let registry = self.registry.clone();
        let task = self.rt.spawn(async move {
            tokio::time::timeout(TOOL_TIMEOUT, registry.invoke(call)).await
        });
        let send = self.send.clone();
        self.rt.spawn(async move {
            let outcome = match task.await {
                Ok(Ok(Ok(output))) => ToolOutcome::success(output),
                Ok(Ok(Err(issue))) => ToolOutcome::error(issue.to_string()),
                Ok(Err(_)) => {
                    ToolOutcome::error(format!("timed out after {}s", TOOL_TIMEOUT.as_secs()))
                }
                Err(_) => ToolOutcome::error("the tool panicked"),
            };
            send(ToolMessage { call_id, outcome });
        });
    }

    fn into_phase(mut self) -> Phase {
        if self.calls.values().any(|state| matches!(state, CallState::Running)) {
            return Phase::Pending(self);
        }
        Phase::Done(self.batch.resolve(|call| {
            match self.calls.remove(&call.id) {
                Some(CallState::Done(outcome)) => outcome.into_text(),
                _ => unreachable!("every call is done"),
            }
        }))
    }
}
