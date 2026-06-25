use std::sync::mpsc::SyncSender;
use serde::{Deserialize, Serialize};
use provider::TokenUsage;

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


/// Actions that the user can take directly in a session
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UserAction {
    /// Send a message to the clanker
    SendMessage(UserMessage),
    /// Approve a tool call that was requested by the clanker
    ApproveToolCall { tool_call_id: String },
    /// Deny a tool call that was requested by the clanker
    DenyToolCall { tool_call_id: String },
}

#[derive(Clone, Debug)]
pub(crate) enum UserCommand {
    Action(UserAction),
    Snapshot { reply: SyncSender<Vec<provider::Message>> }
}

impl From<UserAction> for UserCommand {
    fn from(action: UserAction) -> Self {
        UserCommand::Action(action)
    }
}

/// Event caused by a clanker response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClankerEvent {
    ReasoningDelta { delta: String },
    ContentDelta { delta: String },
    ToolCallRequest {
        id: String,
        name: String,
        arguments: String,
    },
    Done {
        usage: Option<TokenUsage>,
        stop_reason: Option<String>,
    }
}

/// Event caused by a tool call returning.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolCallEvent {
    Result {
        id: String,
        result: String,
    },
    Error {
        id: String,
        // TODO: bad type
        error: String,
    },
    // TODO: this doesn't feel like a tool call event?
    AutoApproved { id: String, name: String, arguments: String }
}

/// Events
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SessionEvent {
    User(UserAction),
    Clanker(ClankerEvent),
    /// Event caused by a tool call returning.
    ToolCall(ToolCallEvent),
}

impl From<UserAction> for SessionEvent {
    fn from(action: UserAction) -> Self {
        SessionEvent::User(action)
    }
}

impl From<ClankerEvent> for SessionEvent {
    fn from(event: ClankerEvent) -> Self {
        SessionEvent::Clanker(event)
    }
}

impl From<ToolCallEvent> for SessionEvent {
    fn from(event: ToolCallEvent) -> Self {
        SessionEvent::ToolCall(event)
    }
}