use crate::paths::inside_workspace;
use auger_core::PolicyCall;
use auger_core::PolicyNode;
use auger_core::Verdict;

/// Allows file reads inside the workspace.
pub struct WorkspaceRead;

impl PolicyNode for WorkspaceRead {
    fn check(&self, call: &PolicyCall<'_>) -> Verdict {
        match call.path {
            Some(access)
                if !access.write && inside_workspace(call.workspace, call.cwd, &access.path) =>
            {
                Verdict::Allow {
                    node: "workspace-read",
                }
            }
            _ => Verdict::Next,
        }
    }
}
