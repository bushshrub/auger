use crate::paths::is_secret;
use crate::paths::resolve;
use auger_core::PolicyCall;
use auger_core::PolicyNode;
use auger_core::Verdict;

/// Asks before any file tool touches something that looks like credentials,
/// even inside the workspace.
pub struct SecretsGuard;

impl PolicyNode for SecretsGuard {
    fn check(&self, call: &PolicyCall<'_>) -> Verdict {
        let Some(access) = call.path else {
            return Verdict::Next;
        };
        let resolved = resolve(call.cwd, &access.path);
        if is_secret(&access.path) || resolved.is_some_and(|p| is_secret(&p)) {
            return Verdict::AskUser {
                node: "secrets-guard",
                reason: format!("{} may hold credentials", access.path.display()),
            };
        }
        Verdict::Next
    }
}
