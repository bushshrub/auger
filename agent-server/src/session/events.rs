use serde::{Deserialize, Serialize};

use crate::conversation::UserContent;

/// Commands that the user send to the harness.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Cmd {
    SendMessage(Vec<UserContent>),
    ApproveToolCall { tool_call_id: String },
    DenyToolCall { tool_call_id: String },
    // Snapshot, // TODO: Conversation snapshot
}

/// Events
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum SessionEvent {
    /// The user sent a message
    UserMessage { content: Vec<UserContent> },
    // todo: split clanker events off into separate enum
    Reasoning { delta: String },
    Content { delta: String },
    /// Fully complete tool call request from the clanker.
    // TODO: Support deltas and merge the deltas at the end.
    ToolCallRequest {
        id: String,
        name: String,
        arguments: String,
    },
    /// The agent has finished responding and will not send any more events.
    Done
}

impl From<Vec<UserContent>> for SessionEvent {
    fn from(value: Vec<UserContent>) -> Self {
        SessionEvent::UserMessage { content: value }
    }
}