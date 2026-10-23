use crate::paths::inside_workspace;
use auger_toolengine::PolicyCall;
use auger_toolengine::PolicyNode;
use auger_toolengine::Verdict;

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
