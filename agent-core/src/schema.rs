use getset::Getters;
use serde::{Deserialize, Serialize};
use auger_driver::ToolCallId;
use crate::tools::tool_execution::ToolData;

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct InputToolResult {
    tool_call_id: ToolCallId,
    // TODO: Persist the tool result status.
    content: Vec<ToolData>,
}

impl InputToolResult {
    pub fn new(tool_call_id: ToolCallId, content: Vec<ToolData>) -> Self {
        Self {
            tool_call_id,
            content,
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct TextData {
    text: String,
}

impl TextData {
    pub fn new(text: String) -> Self {
        Self { text }
    }
}