//! Approval policy node for MCP tools named in the server allow lists.

use auger_toolengine::{PolicyCall, PolicyNode, Verdict};
use std::collections::HashSet;

/// Allows the MCP tools that were configured to run without asking. Other
/// MCP calls pass, so the user is asked.
pub struct McpAllow {
    allowed: HashSet<String>,
}

impl McpAllow {
    pub(crate) fn new(allowed: HashSet<String>) -> Self {
        Self { allowed }
    }
}

impl PolicyNode for McpAllow {
    fn check(&self, call: &PolicyCall<'_>) -> Verdict {
        if self.allowed.contains(call.tool) {
            Verdict::Allow { node: "mcp-allow" }
        } else {
            Verdict::Next
        }
    }
}
