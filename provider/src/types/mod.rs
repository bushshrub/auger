mod request;
mod response;
mod tool;

use getset::Getters;
pub use request::*;
pub use response::*;
use serde::{de, Deserialize, Deserializer};
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
#[derive(Debug, Clone, Serialize, Getters)]
pub struct AssistantResponse {
    #[get = "pub"]
    blocks: Vec<Block>
}

impl AssistantResponse {
    pub fn new(blocks: Vec<Block>) -> Option<Self> {
        if blocks.is_empty() {
            None
        } else {
            Some(Self { blocks })
        }
    }
    pub fn tool_calls(&self) -> Vec<ToolCallRequest> {
        self.blocks.iter().filter_map(|b| match b {
            Block::ToolCall(tc) => Some(tc.clone()),
            _ => None,
        }).collect()
    }
}

impl<'de> Deserialize<'de> for AssistantResponse {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let blocks = Vec::<Block>::deserialize(d)?;
        Self::new(blocks).ok_or_else(|| de::Error::custom("assistant response must be non-empty"))
    }
}

impl From<AssistantResponse> for Message {
    fn from(response: AssistantResponse) -> Self {
        Self::Assistant { response }
    }
}
