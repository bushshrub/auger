use getset::Getters;
use serde::{Deserialize, Serialize};
use crate::types::{Message, ToolDefinition};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserPrompt {
    message: String,
}

impl UserPrompt {
    pub fn new(message: String) -> Self {
        Self { message }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<UserPrompt> for Message {
    fn from(prompt: UserPrompt) -> Self {
        Message::User {
            message: prompt,
            tool_call_results: Vec::new(),
        }
    }
}

/// A result from a tool call which can be sent back to the model.
#[derive(Debug, Clone, Deserialize, Serialize, Getters)]
pub struct ToolResult {
    /// The ID of the tool call that this is a result for
    #[get = "pub"]
    tool_call_id: String,
    /// The output of the tool call.
    #[get = "pub"]
    content: String,
}

impl ToolResult {
    pub fn new(tool_call_id: String, content: String) -> Self {
        Self {
            tool_call_id,
            content,
        }
    }

}

/// A request to get a response from the clanker

#[derive(Debug, Clone)]
pub struct LlmRequest {
    /// Full conversation history and the new message the user asks
    ///
    /// Provider implementations should perform caching/state management as needed.
    messages: Vec<Message>,
    /// Tools that are available for the clanker to call.
    // TODO: add option to enforce tool use somehow.
    tools: Vec<ToolDefinition>,
}

impl LlmRequest {
    pub fn new(messages: Vec<Message>, tools: Vec<ToolDefinition>) -> Self {
        Self { messages, tools }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }
}
