//! Tool phase engine: runs the tool calls of one assistant turn until every
//! call has an answer.

pub(crate) mod check;
pub(crate) mod registry;
pub(crate) mod result;

use crate::harness::tool_engine::check::PolicyDecision;
use crate::harness::tool_engine::check::ToolSetup;
use crate::harness::tool_engine::result::ToolCallResult;
use crate::harness::tool_engine::result::ToolOutcome;
use crate::policy::Verdict;
use crate::tool::ToolContext;
use auger_driver::Resolved;
use auger_driver::ToolBatch;
use auger_driver::Unresolved;
use provider::ToolCallRequest;
use std::collections::HashMap;
use std::path::PathBuf;
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
    /// Waiting for the user to approve or deny. `cwd` is where the call was
    /// checked, and where it runs if approved.
    Undecided { cwd: PathBuf },
    Running,
    Done(ToolOutcome),
}

pub(crate) struct ToolPhaseEngine {
    batch: ToolBatch<Unresolved>,
    calls: HashMap<String, CallState>,
    setup: ToolSetup,
    rt: Handle,
    send: ToolMessageSender,
}

pub(crate) enum Phase {
    /// Some calls have no answer yet. `needs_consent` lists the calls that
    /// just started waiting for a user decision.
    Pending {
        engine: ToolPhaseEngine,
        needs_consent: Vec<ToolCallRequest>,
    },
    /// Every call has an answer.
    Done(ToolBatch<Resolved>),
    /// Every call has an answer, but some were denied or interrupted. The
    /// results wait for the next user message.
    Withheld(ToolBatch<Resolved>),
}

/// The phase after policy checked every call of a new batch.
pub(crate) struct Started {
    /// Calls the policy allowed or denied without asking.
    pub(crate) decisions: Vec<PolicyDecision>,
    /// Calls the policy denied.
    pub(crate) finished: Vec<ToolCallResult>,
    pub(crate) phase: Phase,
}

pub(crate) struct Step {
    /// The calls that finished; empty if the input was dropped.
    pub(crate) finished: Vec<ToolCallResult>,
    pub(crate) phase: Phase,
}

impl ToolPhaseEngine {
    /// Start a phase for `batch`. The policy settles what it can; the other
    /// calls wait for a user decision.
    pub(crate) fn start(
        batch: ToolBatch<Unresolved>,
        setup: ToolSetup,
        rt: &Handle,
        send: ToolMessageSender,
    ) -> Started {
        let calls = batch.calls().to_vec();
        let mut engine = Self {
            batch,
            calls: HashMap::new(),
            setup,
            rt: rt.clone(),
            send,
        };
        let mut needs_consent = Vec::new();
        let mut decisions = Vec::new();
        let mut finished = Vec::new();
        for call in calls {
            let cwd = engine.setup.workdir.get();
            let verdict = engine.setup.check(&call, &cwd);
            match &verdict {
                Verdict::Allow { .. } => engine.run(call.clone(), cwd),
                Verdict::Deny { node, reason } => {
                    let outcome = ToolOutcome::Denied {
                        reason: Some(format!("{reason} (policy: {node})")),
                    };
                    engine
                        .calls
                        .insert(call.id.clone(), CallState::Done(outcome.clone()));
                    finished.push(ToolCallResult {
                        tool_call_id: call.id.clone().into(),
                        outcome,
                    });
                }
                Verdict::AskUser { .. } | Verdict::Next => {
                    engine
                        .calls
                        .insert(call.id.clone(), CallState::Undecided { cwd });
                    needs_consent.push(call.clone());
                    continue;
                }
            }
            decisions.push(PolicyDecision {
                tool_call_id: call.id,
                verdict,
            });
        }
        // A denial withholds every result until the user replies, so asking
        // about the rest of the batch would only be noise.
        if !finished.is_empty() {
            finished.extend(engine.skip_undecided());
            needs_consent.clear();
        }
        let phase = match engine.into_phase() {
            Phase::Pending { engine, .. } => Phase::Pending {
                engine,
                needs_consent,
            },
            phase => phase,
        };
        Started {
            decisions,
            finished,
            phase,
        }
    }

    /// Apply a user decision. Dropped unless the call is undecided. A denial
    /// uses `message` as the reason.
    pub(crate) fn decide(mut self, call_id: &str, approved: bool, message: Option<String>) -> Step {
        let finished = match self.calls.get(call_id) {
            Some(CallState::Undecided { cwd }) if approved => {
                let cwd = cwd.clone();
                let call = self.batch.calls().iter().find(|c| c.id == call_id).cloned();
                if let Some(call) = call {
                    self.run(call, cwd);
                }
                Vec::new()
            }
            Some(CallState::Undecided { .. }) => {
                let outcome = ToolOutcome::Denied { reason: message };
                self.calls.insert(call_id.to_string(), CallState::Done(outcome.clone()));
                let mut finished = vec![ToolCallResult {
                    tool_call_id: call_id.to_string().into(),
                    outcome,
                }];
                finished.extend(self.skip_undecided());
                finished
            }
            _ => Vec::new(),
        };
        Step {
            finished,
            phase: self.into_phase(),
        }
    }

    /// Deny every call still waiting for a decision, after another call of
    /// the batch was denied. Running calls finish normally.
    fn skip_undecided(&mut self) -> Vec<ToolCallResult> {
        let mut skipped = Vec::new();
        for call in self.batch.calls() {
            let Some(state @ CallState::Undecided { .. }) = self.calls.get_mut(&call.id) else {
                continue;
            };
            let outcome = ToolOutcome::Denied {
                reason: Some("not run: another call in this batch was denied".to_string()),
            };
            *state = CallState::Done(outcome.clone());
            skipped.push(ToolCallResult {
                tool_call_id: call.id.clone().into(),
                outcome,
            });
        }
        skipped
    }

    /// Apply a message from a tool task.
    pub(crate) fn receive(mut self, msg: ToolMessage) -> Step {
        let finished = match self.calls.get_mut(&msg.call_id) {
            Some(state @ CallState::Running) => {
                *state = CallState::Done(msg.outcome.clone());
                vec![ToolCallResult {
                    tool_call_id: msg.call_id.into(),
                    outcome: msg.outcome,
                }]
            }
            _ => Vec::new(),
        };
        Step {
            finished,
            phase: self.into_phase(),
        }
    }

    /// Mark `call` running and spawn it. The task always sends exactly one
    /// message, also on error, timeout and panic.
    fn run(&mut self, call: ToolCallRequest, cwd: PathBuf) {
        let call_id = call.id.clone();
        self.calls.insert(call_id.clone(), CallState::Running);
        let registry = self.setup.registry.clone();
        let ctx = ToolContext::for_session(cwd, self.setup.workdir.clone());
        let task = self.rt.spawn(async move {
            tokio::time::timeout(TOOL_TIMEOUT, registry.invoke(ctx, call)).await
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
        if self
            .calls
            .values()
            .any(|state| !matches!(state, CallState::Done(_)))
        {
            return Phase::Pending {
                engine: self,
                needs_consent: Vec::new(),
            };
        }
        let withheld = self.calls.values().any(|state| {
            matches!(
                state,
                CallState::Done(ToolOutcome::Denied { .. } | ToolOutcome::Interrupted)
            )
        });
        let batch = self.batch.resolve(|call| {
            match self.calls.remove(&call.id) {
                Some(CallState::Done(outcome)) => outcome.into_text(),
                _ => unreachable!("every call is done"),
            }
        });
        if withheld {
            Phase::Withheld(batch)
        } else {
            Phase::Done(batch)
        }
    }
}
