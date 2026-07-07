mod request;
mod response;
mod tool;

pub use request::*;
pub use response::*;
pub use tool::*;

#[derive(Debug, Clone)]
pub enum Message {
    /// The system prompt
    System(String),
    /// A message from the user.
    /// This naming is kind of weird because the user is actually
    /// the agent rather than the actual person using the agent.
    /// However, this is apparently how LLM providers
    /// want it sooo yeah we'll stick to it.
    User {
        message: UserPrompt,
        tool_call_results: Vec<ToolResult>,
    }, // TODO: in the future we will need to support images
    /// A message from the model.
    Assistant {
        reasoning: Option<String>,
        content: String,
        tool_calls: Vec<ToolCallRequest>,
    },
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
