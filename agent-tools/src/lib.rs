pub mod dummy;
pub mod edit;
pub mod fetch_content;
pub mod glob;
pub mod grep;
pub mod list_files;
pub(crate) mod rate_limiter;
pub mod read;
pub mod shell;
pub mod todo;
pub mod web_fetch;
pub mod web_fetch_text;
pub mod web_search;
pub mod write;

use std::fmt::Display;
use async_trait::async_trait;
use thiserror::Error;

pub use dummy::Dummy;
pub use edit::EditFile;
pub use fetch_content::FetchContent;
pub use glob::Glob;
pub use grep::Grep;
pub use list_files::ListFiles;
pub use read::ReadFile;
pub use shell::Shell;
pub use todo::TodoList;
pub use web_fetch::WebFetch;
pub use web_fetch_text::WebFetchText;
pub use web_search::WebSearch;
pub use write::WriteFile;

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

#[derive(Debug, Clone)]
pub enum ToolCallResultKind {
    Success,
    DeniedByUser,
    Error
}

/// Result of a tool call which can be sent back to the model.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    kind: ToolCallResultKind,
    msg: String,
}

impl ToolCallResult {
    pub fn success(result: String) -> Self {
        Self { kind: ToolCallResultKind::Success, msg: result }
    }

    pub fn denied_by_user(why: String) -> Self {
        Self { kind: ToolCallResultKind::DeniedByUser, msg: why }
    }

    pub fn error(error: String) -> Self {
        Self { kind: ToolCallResultKind::Error, msg: error }
    }
}



pub struct JsonSchema(pub serde_json::Value);

#[async_trait]
pub trait Tool: Send + Sync {
    /// High level description of the tool for the clanker.
    fn details(&self) -> ToolDetails;
    /// JSON Schema describing the tool's arguments.
    fn parameters(&self) -> JsonSchema;
    /// Invoke the tool with the given arguments.
    ///
    /// Implementors should try to heal the tool call as much as possible.
    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError>;
}
