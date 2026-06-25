use std::collections::HashSet;

pub(crate) struct AutoApprovalPolicy {
    approved_tools: HashSet<String>,
}

impl AutoApprovalPolicy {
    pub(crate) fn new(tool_names: impl IntoIterator<Item = String>) -> Self {
        Self { approved_tools: tool_names.into_iter().collect() }
    }

    pub(crate) fn is_approved(&self, tool_name: &str) -> bool {
        self.approved_tools.contains(tool_name)
    }
}