use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Image {
    Url(String),
    Base64(String)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserMessage {
    pub(crate) msg: String,
    pub(crate) images: Vec<Image>
}

impl UserMessage {
    pub fn new(msg: String) -> Self {
        Self { msg, images: vec![] }
    }
}


/// Commands that the user send to the harness.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum UserCmd {
    SendMessage(UserMessage),
    ApproveToolCall { tool_call_id: String },
    DenyToolCall { tool_call_id: String },
    // Snapshot, // TODO: Conversation snapshot
}

/// Events
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SessionEvent {
    /// The user sent a message
    UserMessage { content: UserMessage },
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
    ToolCallResult {
        id: String,
        result: String,
    },
    ToolCallDenied {
        id: String,
        reason: String,
    },
    ToolCallError {
        id: String,
        // TODO: bad type
        error: String,
    },
    /// The agent has finished responding and will not send any more events.
    Done
}