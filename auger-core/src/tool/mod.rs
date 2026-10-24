use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Execution failed: {0}")]
    Execution(String),
}

/// Details of a tool that the agent should know about.
pub struct ToolDetails {
    /// Name of the tool, will be given to the agent.
    pub name: String,
    /// Description of the tool, will be given to the agent.
    pub description: String,
}

pub struct JsonSchema(pub serde_json::Value);

#[async_trait]
pub trait Tool: Send + Sync {
    /// High level description of the tool for the agent.
    fn details(&self) -> ToolDetails;
    /// JSON Schema describing the tool's arguments.
    fn parameters(&self) -> JsonSchema;
    /// Invoke the tool with the given arguments.
    ///
    /// Implementors should try to heal the tool call as much as possible.
    async fn call(&self, args: serde_json::Value) -> Result<String, ToolError>;
}
