//! Request and response types for the agent server API
use agent_core::SessionEvent;
use agent_core::SessionHandle;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::broadcast;
use uuid::Uuid;

/// A session entry holds the handle plus its access tokens, which are owned by
/// the server.
#[derive(Clone)]
pub(crate) struct SessionEntry {
    pub(crate) handle: SessionHandle,
    pub(crate) events: broadcast::Sender<SessionEvent>,
    pub(crate) model: String,
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
