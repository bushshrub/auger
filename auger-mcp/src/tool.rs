//! An MCP tool exposed as an auger tool.

use crate::server::Server;
use async_trait::async_trait;
use auger_toolengine::{JsonSchema, Tool, ToolContext, ToolDetails, ToolError};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) struct McpTool {
    pub(crate) server: Arc<Server>,
    /// The tool's name on the server.
    pub(crate) remote_name: String,
    /// The name the model sees: `mcp__<server>__<tool>`.
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) schema: Value,
}

#[async_trait]
impl Tool for McpTool {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: self.name.clone(),
            description: self.description.clone(),
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(self.schema.clone())
    }

    async fn call(&self, _ctx: &ToolContext, args: Value) -> Result<String, ToolError> {
        let arguments = if args.is_null() { json!({}) } else { args };
        let result = self
            .server
            .call_tool(&self.remote_name, arguments)
            .await
            .map_err(|error| ToolError::Execution(format!("MCP server `{}`: {error}", self.server.name)))?;
        let text = content_text(&result);
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let message = if text.is_empty() {
                "the MCP tool reported an error".to_string()
            } else {
                text
            };
            return Err(ToolError::Execution(message));
        }
        Ok(text)
    }
}

/// Text items joined by newlines. Other items are summarized, not sent.
pub(crate) fn content_text(result: &Value) -> String {
    let Some(items) = result.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    items
        .iter()
        .map(|item| match item.get("type").and_then(Value::as_str) {
            Some("text") => item
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            Some(kind) => format!("[{kind} omitted]"),
            None => "[unknown content omitted]".to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
