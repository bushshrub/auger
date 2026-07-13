//! Request and response types for the agent server API
use agent_core::{SessionEvent, SessionHandle, SessionOwner};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use tokio::sync::broadcast;
use uuid::Uuid;

/// A session entry holds the handle plus its access tokens, which are owned by the server.
#[derive(Clone)]
pub(crate) struct SessionEntry {
    pub(crate) handle: SessionHandle,
    pub(crate) owner: Arc<Mutex<Option<SessionOwner>>>,
    pub(crate) events: broadcast::Sender<SessionEvent>,
    pub(crate) model: String,
    pub(crate) created_at: u64,
    pub(crate) read_token: Uuid,
    pub(crate) write_token: Uuid,
    pub(crate) archived: Arc<AtomicBool>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CreateSessionRequest {
    pub(crate) model: Option<String>,
}

/// A request to send a message in an existing session
#[derive(Deserialize, Debug)]
pub(crate) struct UserInputRequest {
    pub(crate) input: String,
}

impl From<UserInputRequest> for provider::UserPrompt {
    fn from(req: UserInputRequest) -> Self {
        provider::UserPrompt::new(req.input)
    }
}

/// Whether the user approves or denies the tool use
#[derive(Deserialize, Debug)]
pub(crate) struct ApproveRequest {
    pub(crate) tool_call_id: String,
    pub(crate) approved: bool,
    pub(crate) message: Option<String>,
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
    User {
        text: String,
    },
    Assistant {
        reasoning: Option<String>,
        content: String,
        tool_calls: Vec<SnapshotToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

impl SnapshotMessage {
    pub(crate) fn from_provider(msg: provider::Message) -> Vec<Self> {
        match msg {
            provider::Message::System(_) => vec![],
            provider::Message::User {
                message,
                tool_call_results,
            } => {
                let mut messages = vec![Self::User {
                    text: message.message().to_string(),
                }];
                for tool_call_result in tool_call_results {
                    messages.push(Self::Tool {
                        tool_call_id: tool_call_result.id().to_string(),
                        content: tool_call_result.content().to_string(),
                    });
                }
                messages
            }
            provider::Message::Assistant {
                reasoning,
                content,
                tool_calls,
            } => vec![Self::Assistant {
                reasoning,
                content,
                tool_calls: tool_calls
                    .into_iter()
                    .map(|tc| SnapshotToolCall {
                        id: tc.id,
                        name: tc.name,
                        arguments: tc.arguments,
                    })
                    .collect(),
            }],
        }
    }
}
