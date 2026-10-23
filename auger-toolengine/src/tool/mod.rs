pub(crate) mod context;

use async_trait::async_trait;
use std::path::PathBuf;
use thiserror::Error;

pub use context::ToolContext;

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

/// A file a call reads or writes. The path may be relative to the call's
/// working directory.
#[derive(Clone, Debug)]
pub struct PathAccess {
    pub path: PathBuf,
    pub write: bool,
}

#[async_trait]
pub trait Tool: Send + Sync {
    /// High level description of the tool for the agent.
    fn details(&self) -> ToolDetails;
    /// JSON Schema describing the tool's arguments.
    fn parameters(&self) -> JsonSchema;
    /// The file this call touches, for approval policy. `None` if the tool
    /// has no file facet or the arguments do not name one.
    fn path(&self, _args: &serde_json::Value) -> Option<PathAccess> {
        None
    }
    /// The shell command this call runs, for approval policy.
    fn command(&self, _args: &serde_json::Value) -> Option<String> {
        None
    }
    /// Invoke the tool with the given arguments.
    ///
    /// Implementors should try to heal the tool call as much as possible.
    async fn call(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<String, ToolError>;
}
