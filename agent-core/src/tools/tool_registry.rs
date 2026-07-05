use std::collections::HashMap;
use thiserror::Error;
use agent_tools::{Tool, ToolCallResult, ToolError};
use provider::{ToolCallRequest, ToolDefinition};

#[derive(Debug, Error)]
pub(crate) enum ToolInvokeIssue {
    #[error("Tool '{0}' not found in registry")]
    ToolNotFound(String),
    #[error("Failed to parse tool arguments: {0}")]
    ArgumentParseError(String),
    #[error("Error invoking tool: {0}")]
    ToolError(ToolError)
}

// TODO: refactor tool registry into the agent-tools crate and other tool handling stuff.
pub(crate) struct ToolRegistry(HashMap<String, Box<dyn Tool>>);
impl ToolRegistry {
    pub(crate) fn new() -> Self {
        Self(HashMap::new())
    }

    pub(crate) fn register(&mut self, tool: Box<dyn Tool>) {
        self.0.insert(tool.details().name.to_string(), tool);
    }

    pub(crate) async fn call_tool(&self, tool_name: &str, args: String) -> Result<ToolCallResult, ToolInvokeIssue> {
        let tool = self.0.get(tool_name).ok_or(ToolInvokeIssue::ToolNotFound(tool_name.into()))?;
        // TODO: try to fix json if it's malformed, and provide better error messages
        let args_json = serde_json::from_str(&args).map_err(|e| ToolInvokeIssue::ArgumentParseError(e.to_string()))?;
        tool.call(args_json).await.map_err(ToolInvokeIssue::ToolError)
    }

    pub(crate) async fn invoke(&self, tc: ToolCallRequest) -> Result<ToolCallResult, ToolInvokeIssue> {
        self.call_tool(&tc.name, tc.arguments).await
    }

    // TODO: add option to compress and expose a single tool search?
    pub(crate) fn list_for_clanker(&self) -> Vec<ToolDefinition> {
        self.0.values().map(|tool| ToolDefinition {
            name: tool.details().name.to_string(),
            description: Some(tool.details().description.to_string()),
            parameters: tool.parameters().0,
        }).collect()
    }
}