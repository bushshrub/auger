use serde::{Deserialize, Serialize};

use crate::conversation::UserContent;

#[derive(Serialize, Deserialize)]
pub(crate) enum Cmd {
    SendMessage(Vec<UserContent>),
    ApproveToolCall { tool_call_id: String },
    DenyToolCall { tool_call_id: String },
    // Snapshot, // TODO: Conversation snapshot
}

#[derive(Serialize, Deserialize)]
pub(crate) enum AgentEvent {
    UserMessage { content: Vec<UserContent> },
    Reasoning { delta: String },
    Content { delta: String },
}

impl From<Vec<UserContent>> for AgentEvent {
    fn from(value: Vec<UserContent>) -> Self {
        AgentEvent::UserMessage { content: value }
    }
}