//! Runs the approval policy for one tool call.

use crate::engine::registry::ToolRegistry;
use crate::fact::Journal;
use crate::policy::PolicyCall;
use crate::policy::PolicyNode;
use crate::policy::Verdict;
use crate::tool::Tool;
use crate::tool::context::WorkDir;
use provider::ToolCallRequest;
use provider::ToolDefinition;
use std::path::PathBuf;
use std::sync::Arc;

/// Everything a tool phase needs from its session: the tools, the policy,
/// the working directory the calls share, and where facts go.
#[derive(Clone)]
pub struct ToolSetup {
    pub(crate) registry: Arc<ToolRegistry>,
    pub(crate) policy: Arc<dyn PolicyNode>,
    pub(crate) workspace: PathBuf,
    pub(crate) workdir: WorkDir,
    pub(crate) journal: Journal,
}

impl ToolSetup {
    /// Calls start in `cwd`, which is `workspace` for a new session.
    pub fn new(
        tools: Vec<Box<dyn Tool>>,
        policy: Arc<dyn PolicyNode>,
        workspace: PathBuf,
        cwd: PathBuf,
        journal: Journal,
    ) -> Self {
        let mut registry = ToolRegistry::new();
        for tool in tools {
            registry.register(tool);
        }
        Self {
            registry: Arc::new(registry),
            policy,
            workspace,
            workdir: WorkDir::journaled(cwd, journal.clone()),
            journal,
        }
    }

    /// The tool definitions to give the model.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.registry.list_for_clanker()
    }

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
