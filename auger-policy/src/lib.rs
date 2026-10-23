//! Approval policy nodes for auger's built-in tools.
//!
//! [`default_policy`] lets calls that only read inside the workspace run
//! without asking. Everything else falls through to the user.

mod bash_read_only;
mod paths;
mod secrets;
mod workspace_read;

pub use bash_read_only::BashReadOnly;
pub use secrets::SecretsGuard;
pub use workspace_read::WorkspaceRead;

use auger_toolengine::Chain;
use auger_toolengine::PolicyNode;

/// Secrets guard, then reads inside the workspace, then read-only bash.
pub fn default_policy() -> impl PolicyNode {
    Chain(SecretsGuard, Chain(WorkspaceRead, BashReadOnly))
}
