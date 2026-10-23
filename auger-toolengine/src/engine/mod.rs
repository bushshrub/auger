//! Tool phase engine: runs the tool calls of one assistant turn until every
//! call has an answer.

pub(crate) mod check;
pub(crate) mod registry;
pub(crate) mod result;

use crate::engine::check::ToolSetup;
use crate::engine::result::ToolCallResult;
use crate::engine::result::ToolOutcome;
use crate::fact::DecidedBy;
use crate::fact::Decision;
use crate::fact::ToolFact;
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

const OUTCOME_UNKNOWN: &str = "auger stopped while this call was running, so its outcome is \
                               unknown. Check its effects before running it again.";

/// Delivers a [`ToolMessage`] back to whoever owns the engine.
pub type ToolMessageSender = Arc<dyn Fn(ToolMessage) + Send + Sync>;

/// A message from a spawned tool task. Opaque outside the engine.
pub struct ToolMessage {
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

pub struct ToolPhaseEngine {
    batch: ToolBatch<Unresolved>,
    calls: HashMap<String, CallState>,
    setup: ToolSetup,
    rt: Handle,
    send: ToolMessageSender,
}

pub enum Phase {
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

impl ToolPhaseEngine {
    /// Start a phase for `batch`. The policy settles what it can; the other
    /// calls wait for a user decision.
    pub fn start(
        batch: ToolBatch<Unresolved>,
        setup: ToolSetup,
        rt: &Handle,
        send: ToolMessageSender,
    ) -> Phase {
        Self::resume(batch, setup, rt, send, &[])
    }

    /// Continue a phase that was cut off, given the facts journaled for this
    /// batch. Finished calls keep their outcome. A call that started but never
    /// finished is not run again: its effects are unknown, so the model is
    /// told so. Every other call starts over.
    pub fn resume(
        batch: ToolBatch<Unresolved>,
        setup: ToolSetup,
        rt: &Handle,
        send: ToolMessageSender,
        facts: &[ToolFact],
    ) -> Phase {
        let calls = batch.calls().to_vec();
        let mut engine = Self {
            batch,
            calls: HashMap::new(),
            setup,
            rt: rt.clone(),
            send,
        };
        let mut needs_consent = Vec::new();
        for call in calls {
            if let Some(outcome) = finished_outcome(facts, &call.id) {
                engine.calls.insert(call.id, CallState::Done(outcome));
                continue;
            }
            if started(facts, &call.id) {
                engine.finish(&call.id, ToolOutcome::error(OUTCOME_UNKNOWN));
                continue;
            }
            let cwd = engine.setup.workdir.get();
            match engine.setup.check(&call, &cwd) {
                Verdict::Allow { node } => {
                    engine.decided(&call.id, Decision::Allowed {
                        by: DecidedBy::Policy(node.to_string()),
                    });
                    engine.run(call, cwd);
                }
                Verdict::Deny { node, reason } => {
                    let reason = format!("{reason} (policy: {node})");
                    engine.decided(&call.id, Decision::Denied {
                        by: DecidedBy::Policy(node.to_string()),
                        reason: Some(reason.clone()),
                    });
                    engine.finish(&call.id, ToolOutcome::Denied {
                        reason: Some(reason),
                    });
                }
                Verdict::AskUser { .. } | Verdict::Next => {
                    engine.calls.insert(call.id.clone(), CallState::Undecided { cwd });
                    needs_consent.push(call);
                }
            }
        }
        // A denial withholds every result until the user replies, so asking
        // about the rest of the batch would only be noise.
        if engine.any_denied() {
            engine.skip_undecided();
            needs_consent.clear();
        }
        match engine.into_phase() {
            Phase::Pending { engine, .. } => Phase::Pending {
                engine,
                needs_consent,
            },
            phase => phase,
        }
    }

    /// Apply a user decision. Dropped unless the call is undecided. A denial
    /// uses `message` as the reason.
    pub fn decide(mut self, call_id: &str, approved: bool, message: Option<String>) -> Phase {
        match self.calls.get(call_id) {
            Some(CallState::Undecided { cwd }) if approved => {
                let cwd = cwd.clone();
                let call = self.batch.calls().iter().find(|c| c.id == call_id).cloned();
                if let Some(call) = call {
                    self.decided(call_id, Decision::Allowed { by: DecidedBy::User });
                    self.run(call, cwd);
                }
            }
            Some(CallState::Undecided { .. }) => {
                self.decided(call_id, Decision::Denied {
                    by: DecidedBy::User,
                    reason: message.clone(),
                });
                self.finish(call_id, ToolOutcome::Denied { reason: message });
                self.skip_undecided();
            }
            _ => {}
        }
        self.into_phase()
    }

    /// Apply a message from a tool task.
    pub fn receive(mut self, msg: ToolMessage) -> Phase {
        if matches!(self.calls.get(&msg.call_id), Some(CallState::Running)) {
            self.finish(&msg.call_id, msg.outcome);
        }
        self.into_phase()
    }

    fn decided(&self, call_id: &str, decision: Decision) {
        let _ = (self.setup.journal)(ToolFact::Decided {
            tool_call_id: call_id.to_string(),
            decision,
        });
    }

    /// Settle `call_id` and journal its outcome.
    fn finish(&mut self, call_id: &str, outcome: ToolOutcome) {
        self.calls
            .insert(call_id.to_string(), CallState::Done(outcome.clone()));
        let _ = (self.setup.journal)(ToolFact::Finished(ToolCallResult {
            tool_call_id: call_id.to_string().into(),
            outcome,
        }));
    }

    fn any_denied(&self) -> bool {
        self.calls
            .values()
            .any(|state| matches!(state, CallState::Done(ToolOutcome::Denied { .. })))
    }

    /// Deny every call still waiting for a decision, after another call of
    /// the batch was denied. Running calls finish normally.
    fn skip_undecided(&mut self) {
        let undecided: Vec<String> = self
            .batch
            .calls()
            .iter()
            .filter(|call| matches!(self.calls.get(&call.id), Some(CallState::Undecided { .. })))
            .map(|call| call.id.clone())
            .collect();
        for call_id in undecided {
            self.finish(&call_id, ToolOutcome::Denied {
                reason: Some("not run: another call in this batch was denied".to_string()),
            });
        }
    }

    /// Journal that `call` starts, then spawn it. The task always sends
    /// exactly one message, also on error, timeout and panic. A call whose
    /// start cannot be journaled does not run.
    fn run(&mut self, call: ToolCallRequest, cwd: PathBuf) {
        let call_id = call.id.clone();
        let started = (self.setup.journal)(ToolFact::Started {
            tool_call_id: call_id.clone(),
            cwd: cwd.clone(),
        });
        if let Err(error) = started {
            self.finish(&call_id, ToolOutcome::error(format!("not run: {error}")));
            return;
        }
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

fn finished_outcome(facts: &[ToolFact], call_id: &str) -> Option<ToolOutcome> {
    facts.iter().find_map(|fact| match fact {
        ToolFact::Finished(result) if String::from(result.tool_call_id()) == call_id => {
            Some(result.outcome().clone())
        }
        _ => None,
    })
}

fn started(facts: &[ToolFact], call_id: &str) -> bool {
    facts.iter().any(|fact| {
        matches!(fact, ToolFact::Started { tool_call_id, .. } if tool_call_id == call_id)
    })
}
