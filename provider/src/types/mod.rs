mod request;
mod response;
mod tool;

use serde::{Deserialize, Serialize};
pub use request::*;
pub use response::*;
pub use tool::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        response: AssistantResponse
    },
}
/// Response from the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantResponse {
    pub reasoning: Option<String>,
    pub content: String,
    pub tool_calls: Vec<ToolCallRequest>,
}

impl From<AssistantResponse> for Message {
    fn from(response: AssistantResponse) -> Self {
        Message::Assistant { response }
    }
}

impl From<CompletedLlmResponse> for Message {
    fn from(response: CompletedLlmResponse) -> Self {
        let tool_calls = response.tool_calls.unwrap_or_default();
        Message::Assistant {
            response: AssistantResponse {
                reasoning: response.reasoning,
                content: response.content,
                tool_calls
            }
        }
    }
}
