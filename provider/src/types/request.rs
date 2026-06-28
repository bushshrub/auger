use crate::types::{Message, ToolDefinition};

#[derive(Debug, Clone)]
pub struct UserPrompt {
    message: String,
}

impl UserPrompt {
    pub fn new(message: String) -> Self {
        Self { message }
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
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// The ID of the tool call that this is a result for
    tool_call_id: String,
    /// The output of the tool call.
    content: String,
}

impl ToolResult {
    pub fn new(tool_call_id: String, content: String) -> Self {
        Self {
            tool_call_id,
            content,
        }
    }

    pub fn id(&self) -> &str {
        &self.tool_call_id
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

/// A request to get a response from the clanker

#[derive(Debug, Clone)]
pub struct LlmRequest {
    model: String,
    /// Full conversation history and the new message the user asks
    ///
    /// Provider implementations should perform caching/state management as needed.
    messages: Vec<Message>,
    /// Tools that are available for the clanker to call.
    // TODO: add option to enforce tool use somehow.
    tools: Vec<ToolDefinition>,
}

impl LlmRequest {
    pub fn new(model: String, messages: Vec<Message>, tools: Vec<ToolDefinition>) -> Self {
        Self {
            model,
            messages,
            tools,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }
}
