//! Approval policy for tool calls.
//!
//! A policy is a chain of nodes. Each node looks at the facets of a call (its
//! file, its command) and either answers or passes with [`Verdict::Next`].
//! The first answer wins; a call nobody answers asks the user.

use crate::tool::PathAccess;
use std::path::Path;

/// What a node sees of a call.
pub struct PolicyCall<'a> {
    pub tool: &'a str,
    pub path: Option<&'a PathAccess>,
    pub command: Option<&'a str>,
    /// The directory the call runs in.
    pub cwd: &'a Path,
    /// The session's workspace root.
    pub workspace: &'a Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No opinion; ask the next node.
    Next,
    /// Run without asking. `node` names who allowed it.
    Allow { node: &'static str },
    /// Do not run; the reason goes back to the model.
    Deny { node: &'static str, reason: String },
    /// Ask the user.
    AskUser { node: &'static str, reason: String },
}

pub trait PolicyNode: Send + Sync {
    fn check(&self, call: &PolicyCall<'_>) -> Verdict;
}

/// Runs `A`, then `B` if `A` passes.
pub struct Chain<A, B>(pub A, pub B);

impl<A: PolicyNode, B: PolicyNode> PolicyNode for Chain<A, B> {
    fn check(&self, call: &PolicyCall<'_>) -> Verdict {
        match self.0.check(call) {
            Verdict::Next => self.1.check(call),
            verdict => verdict,
        }
    }
}

/// Passes on everything, so every call asks the user.
pub struct AskAlways;

impl PolicyNode for AskAlways {
    fn check(&self, _call: &PolicyCall<'_>) -> Verdict {
        Verdict::Next
    }
}
