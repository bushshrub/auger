use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The name of the tool
    pub name: String,
    /// An optional description of the tool for the model.
    pub description: Option<String>,
    /// The parameters that the tool expects, as a JSON schema object.
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// The unique ID of this tool call, used for approval and result reporting
    pub id: String,
    /// The name of the tool to call, which should match one of the tools provided in the request
    pub name: String,
    /// The arguments to call the tool with. Generally this is a JSON string.
    pub arguments: String,
}
