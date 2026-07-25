mod request;
mod response;
mod tool;

pub use request::*;
pub use response::*;
use serde::Deserialize;
use serde::Serialize;
pub use tool::*;

/// Sink for events emitted during an LLM stream.
pub type EventSink<'a> = &'a mut (dyn FnMut(StreamEvent) + Send + 'a);

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
    Assistant { response: AssistantResponse },
}

/// auger's wire type for responses from the Assistant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantResponse {
    pub blocks: Vec<Block>
}

impl AssistantResponse {
    pub fn tool_calls(&self) -> Vec<ToolCallRequest> {
        self.blocks.iter().filter_map(|b| match b {
            Block::ToolCall(tc) => Some(tc.clone()),
            _ => None,
        }).collect()
    }
}

impl From<AssistantResponse> for Message {
    fn from(response: AssistantResponse) -> Self {
        Self::Assistant { response }
    }
}
