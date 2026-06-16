pub mod read;
pub mod web_fetch;

use async_trait::async_trait;
use thiserror::Error;

pub use read::ReadFile;
// pub use web_fetch::WebFetch;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Execution failed: {0}")]
    Execution(String),
}

/// Details of a tool that the clanker should know about.
pub struct ToolDetails {
    /// Name of the tool, will be given to the clanker.
    pub name: &'static str,
    /// Description of the tool, will be given to the clanker.
    pub description: &'static str,
}

pub struct JsonSchema(serde_json::Value);

#[async_trait]
pub trait Tool: Send + Sync {
    fn details(&self) -> ToolDetails;
    /// JSON Schema describing the tool's arguments.
    fn parameters(&self) -> JsonSchema;
    async fn call(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError>;
}
