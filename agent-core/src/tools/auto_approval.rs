use std::collections::HashSet;
use provider::ToolCallRequest;

pub(crate) struct AutoApprovalPolicy {
    approved_tools: HashSet<String>,
}

impl AutoApprovalPolicy {
    pub(crate) fn new(tool_names: impl IntoIterator<Item = String>) -> Self {
        Self {
            approved_tools: tool_names.into_iter().collect(),
        }
    }

    pub(crate) fn is_approved(&self, tool_name: &str) -> bool {
        self.approved_tools.contains(tool_name)
    }

    pub(crate) fn will_approve_all(&self, tool_names: impl IntoIterator<Item = String>) -> bool {
        tool_names.into_iter().all(|name| self.approved_tools.contains(&name))
    }

    /// Get the names of the tools which require user consent to run.
    pub(crate) fn tools_needing_consent(&self, tool_names: impl IntoIterator<Item = String>) -> Vec<String> {
        tool_names
            .into_iter()
            .filter(|name| !self.approved_tools.contains(name))
            .collect()
    }

    pub(crate) fn ids_needing_consent(&self, requested_calls: Vec<ToolCallRequest>) -> Vec<String> {
        requested_calls
            .into_iter()
            .filter(|call| !self.approved_tools.contains(&call.name))
            .map(|call| call.id)
            .collect()
    }
}
