//! Request and response types for the agent server API
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use agent_core::{SessionHandle, UserMessage};

/// A session entry holds the handle plus its access tokens, which are owned by the server.
#[derive(Clone)]
pub(crate) struct SessionEntry {
    pub(crate) handle: SessionHandle,
    pub(crate) read_token: Uuid,
    pub(crate) write_token: Uuid,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CreateSessionRequest {
    pub(crate) model: Option<String>,
}

/// A request to send a message in an existing session
#[derive(Deserialize, Debug)]
pub(crate) struct UserInputRequest {
    pub(crate) input: String
}

impl From<UserInputRequest> for UserMessage {
    fn from(req: UserInputRequest) -> Self {
        UserMessage::new(req.input)
    }
}

/// Whether the user approves or denies the tool use
#[derive(Deserialize, Debug)]
pub(crate) struct ApproveRequest {
    pub(crate) tool_call_id: String,
    pub(crate)  approved: bool,
    pub(crate)  message: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct SnapshotToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SnapshotMessage {
    User { text: String },
    Assistant { reasoning: Option<String>, content: String, tool_calls: Vec<SnapshotToolCall> },
    Tool { tool_call_id: String, content: String },
}

impl SnapshotMessage {
    pub(crate) fn from_provider(msg: provider::Message) -> Option<Self> {
        match msg {
            provider::Message::System(_) => None,
            provider::Message::User(text) => Some(Self::User { text }),
            provider::Message::Assistant { reasoning, content, tool_calls } => Some(Self::Assistant {
                reasoning,
                content,
                tool_calls: tool_calls.into_iter().map(|tc| SnapshotToolCall {
                    id: tc.id,
                    name: tc.name,
                    arguments: tc.arguments,
                }).collect(),
            }),
            provider::Message::Tool { tool_call_id, content } => Some(Self::Tool { tool_call_id, content }),
        }
    }
}