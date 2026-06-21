//! Request and response types for the agent server API
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub(crate) struct CreateSessionRequest {
    pub(crate) model: Option<String>,
}

/// A request to send a message in an existing session
#[derive(Deserialize, Debug)]
pub(crate) struct UserInputRequest {
    pub(crate) input: String
}

/// Whether the user approves or denies the tool use
#[derive(Deserialize, Debug)]
pub(crate) struct ApproveRequest {
    pub(crate) tool_call_id: String,
    pub(crate)  approved: bool,
    pub(crate)  message: Option<String>,
}