//! Facts the tool engine reports while it runs a batch.
//!
//! The engine knows nothing about where facts go. Its owner supplies a
//! [`Journal`]; a journal that persists facts lets a crashed session resume
//! its tool phase without running a call twice (see
//! [`crate::ToolPhaseEngine::resume`]).

use crate::engine::result::ToolCallResult;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ToolFact {
    /// The policy or the user decided a call.
    Decided {
        tool_call_id: String,
        decision: Decision,
    },
    /// The call is about to run. Journaled before the tool starts, so a
    /// resumed session knows the call may have had effects.
    Started { tool_call_id: String, cwd: PathBuf },
    /// The call has its final outcome.
    Finished(ToolCallResult),
    /// A tool moved the session's working directory.
    CwdChanged { cwd: PathBuf },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allowed { by: DecidedBy },
    Denied { by: DecidedBy, reason: Option<String> },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum DecidedBy {
    /// A policy node; holds its name.
    Policy(String),
    User,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("failed to journal tool fact: {0}")]
pub struct JournalError(pub String);

/// Receives every [`ToolFact`] in order. Returning `Ok` for
/// [`ToolFact::Started`] means the fact is as durable as the owner needs
/// before the tool runs; an error keeps the call from running.
pub type Journal = Arc<dyn Fn(ToolFact) -> Result<(), JournalError> + Send + Sync>;
