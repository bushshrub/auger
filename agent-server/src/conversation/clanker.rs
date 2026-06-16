pub struct ClankerTurn {
    content: Option<String>,
    tool_calls: Vec<ToolCall>,
}

pub struct ToolCall {
    id: String,
    name: String,
    args: serde_json::Value,
}

pub enum ToolStatus {
    /// The tool call is pending approval
    Pending,
    /// Denied by the user (or autoclassifier?)
    Denied,
    /// Approved by the user (or autoclassifier?)
    Approved { result: ToolResult },
}

pub enum ToolResult {
    Success(serde_json::Value),
    Error(String)
}
