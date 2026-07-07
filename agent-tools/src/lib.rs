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
    pub name: &'static str,
    /// Description of the tool, will be given to the agent.
    pub description: &'static str,
}

#[derive(Debug, Clone)]
pub enum ToolCallResultKind {
    Success,
    DeniedByUser,
    Error,
}

/// Result of a tool call which can be sent back to the model.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    kind: ToolCallResultKind,
    msg: String,
}

impl ToolCallResult {
    pub fn success(result: String) -> Self {
        Self {
            kind: ToolCallResultKind::Success,
            msg: result,
        }
    }

    pub fn denied_by_user(why: String) -> Self {
        Self {
            kind: ToolCallResultKind::DeniedByUser,
            msg: why,
        }
    }

    pub fn error(error: String) -> Self {
        Self {
            kind: ToolCallResultKind::Error,
            msg: error,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.kind, ToolCallResultKind::Error)
    }
}

impl std::fmt::Display for ToolCallResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
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
    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError>;
}
