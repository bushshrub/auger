use provider::ToolCallRequest;
use std::collections::HashMap;
use std::sync::Arc;

/// Decides whether a specific call to a tool can run without user consent.
pub trait AutoApprovalPolicy: Send + Sync {
    fn is_approved(&self, tool_call: &ToolCallRequest) -> bool;
}

// TODO: Introduce new finer-grained auto approval policy system.
// Needs more design. At a high level, based on layers.
// Each tool can decide whether a tool call can be auto approved, or it can delegate.
// In the future, extensions can also decide whether a tool call can be auto approved.
/// A layered collection of auto-approval policies, grouped by tool name.
#[derive(Clone, Default)]
pub struct AutoApprovalPolicies {
    by_tool: HashMap<String, Vec<Arc<dyn AutoApprovalPolicy>>>,
}

impl AutoApprovalPolicies {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds another policy layer for a tool.
    ///
    /// A call is auto-approved when any policy registered for that tool approves it.
    pub fn add<P>(&mut self, tool_name: impl Into<String>, policy: P)
    where
        P: AutoApprovalPolicy + 'static,
    {
        self.by_tool
            .entry(tool_name.into())
            .or_default()
            .push(Arc::new(policy));
    }

    /// Adds an unconditional policy for a tool.
    pub fn always_approve(&mut self, tool_name: impl Into<String>) {
        self.add(tool_name, AlwaysApprove);
    }

    pub(crate) fn is_approved(&self, tool_call: &ToolCallRequest) -> bool {
        self.by_tool
            .get(&tool_call.name)
            .is_some_and(|policies| policies.iter().any(|policy| policy.is_approved(tool_call)))
    }

    pub(crate) fn will_approve_all(&self, tool_calls: &[ToolCallRequest]) -> bool {
        tool_calls.iter().all(|call| self.is_approved(call))
    }

    pub(crate) fn ids_needing_consent(
        &self,
        requested_calls: &[ToolCallRequest],
    ) -> Vec<String> {
        requested_calls
            .iter()
            .filter(|call| !self.is_approved(call))
            .map(|call| call.id.clone())
            .collect()
    }
}

impl From<Vec<String>> for AutoApprovalPolicies {
    fn from(tool_names: Vec<String>) -> Self {
        let mut policies = Self::new();
        for tool_name in tool_names {
            policies.always_approve(tool_name);
        }
        policies
    }
}

struct AlwaysApprove;

impl AutoApprovalPolicy for AlwaysApprove {
    fn is_approved(&self, _tool_call: &ToolCallRequest) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ArgumentEquals(&'static str);

    impl AutoApprovalPolicy for ArgumentEquals {
        fn is_approved(&self, tool_call: &ToolCallRequest) -> bool {
            tool_call.arguments == self.0
        }
    }

    fn call(name: &str, arguments: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: format!("{name}-call"),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    #[test]
    fn policies_are_scoped_to_their_tool() {
        let mut policies = AutoApprovalPolicies::new();
        policies.add("shell", ArgumentEquals("safe"));

        assert!(policies.is_approved(&call("shell", "safe")));
        assert!(!policies.is_approved(&call("other", "safe")));
    }

    #[test]
    fn policy_layers_can_independently_approve_a_call() {
        let mut policies = AutoApprovalPolicies::new();
        policies.add("shell", ArgumentEquals("first"));
        policies.add("shell", ArgumentEquals("second"));

        assert!(policies.is_approved(&call("shell", "first")));
        assert!(policies.is_approved(&call("shell", "second")));
        assert!(!policies.is_approved(&call("shell", "third")));
    }
}
