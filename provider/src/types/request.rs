use crate::LlmResponse;
use crate::types::{ToolCall, ToolDefinition};

#[derive(Debug, Clone)]
pub enum Message {
    /// The system prompt
    System(String),
    User(String), // TODO: in the future we will need to support images
    /// A message from the model.
    Assistant {
        reasoning: Option<String>,
        content: String,
        tool_calls: Vec<ToolCall>,
    },
    /// Result of a tool call. Use this to send the result of a tool call back to the model.
    Tool {
        tool_call_id: String, // TODO: Crappy type. should be custom type.
        content: String,
    },
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    /// The ID of the tool call that this is a result for
    pub tool_call_id: String,
    /// The output of the tool call. Generally this is a JSON string, but it could be something else
    pub content: String,
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
    // TODO: add option to enforce tool use somehow.
    pub tools: Vec<ToolDefinition>,
}

impl LlmRequest {
    pub fn new(model: String, messages: Vec<Message>, tools: Vec<ToolDefinition>) -> Self {
        Self { model, messages, tools }
    }

    pub fn new_with_tool_results(model: String, messages: Vec<Message>, tools: Vec<ToolDefinition>, tool_results: Vec<ToolResult>) -> Self {
        let mut messages_with_tool_results = messages;
        for tool_result in tool_results {
            messages_with_tool_results.push(Message::Tool {
                tool_call_id: tool_result.tool_call_id,
                content: tool_result.content,
            });
        }
        Self { model, messages: messages_with_tool_results, tools }
    }
}


impl From<LlmResponse> for Message {
    fn from(response: LlmResponse) -> Self {
        let tool_calls = response.tool_calls.unwrap_or_default();
        Message::Assistant {
            reasoning: None,
            content: response.content,
            tool_calls,
        }
    }
}

impl From<ToolResult> for Message {
    fn from(tool_result: ToolResult) -> Self {
        Message::Tool {
            tool_call_id: tool_result.tool_call_id,
            content: tool_result.content,
        }
    }
}
