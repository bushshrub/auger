use crate::types::{ToolCall, ToolDefinition};

#[derive(Debug, Clone)]
pub enum Message {
    System(String),
    User(String), // TODO: in the future we will need to support images
    Assistant {
        content: String,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// A request to get a response from the clanker

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    /// Full conversation history and the new message the user asks
    ///
    /// Provider implementations should perform caching/state management as needed.
    pub messages: Vec<Message>,
    /// Tools that are available for the clanker to call.
    pub tools: Vec<ToolDefinition>,
}


