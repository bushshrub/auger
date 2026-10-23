//! Runs the approval policy for one tool call.

use crate::harness::tool_engine::registry::ToolRegistry;
use crate::policy::PolicyCall;
use crate::policy::PolicyNode;
use crate::policy::Verdict;
use crate::tool::context::WorkDir;
use provider::ToolCallRequest;
use std::path::PathBuf;
use std::sync::Arc;

/// Everything the tool phase needs from the session.
#[derive(Clone)]
pub(crate) struct ToolSetup {
    pub(crate) registry: Arc<ToolRegistry>,
    pub(crate) policy: Arc<dyn PolicyNode>,
    pub(crate) workspace: PathBuf,
    pub(crate) workdir: WorkDir,
}

/// A verdict that settled a call without asking the user.
#[derive(Clone, Debug)]
pub(crate) struct PolicyDecision {
    pub(crate) tool_call_id: String,
    pub(crate) verdict: Verdict,
}

impl ToolSetup {
    /// Decide `call` in `cwd`. Never returns [`Verdict::Next`].
    pub(crate) fn check(&self, call: &ToolCallRequest, cwd: &PathBuf) -> Verdict {
        // An unknown tool or malformed arguments only produce an error for
        // the model, so there is nothing to ask the user about.
        let Some(tool) = self.registry.get(&call.name) else {
            return Verdict::Allow { node: "invalid-call" };
        };
        let Ok(args) = serde_json::from_str(&call.arguments) else {
            return Verdict::Allow { node: "invalid-call" };
        };
        let path = tool.path(&args);
        let command = tool.command(&args);
        let verdict = self.policy.check(&PolicyCall {
            tool: &call.name,
            path: path.as_ref(),
            command: command.as_deref(),
            cwd,
            workspace: &self.workspace,
        });
        match verdict {
            Verdict::Next => Verdict::AskUser {
                node: "default",
                reason: "no policy allows this call".to_string(),
            },
            verdict => verdict,
        }
    }
}
