use std::fmt::Display;
use agent_tools::Tool;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(crate) mod events;
pub(crate) mod handle;

pub(crate) mod session_loop;
mod tool_registry;
mod tool_call_batch;

/// The status of a session
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Running,
    AwaitingApproval,
}

pub type SessionId = Uuid;


#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
pub struct ReadToken (Uuid);

impl Display for ReadToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<ReadToken> for String {
    fn from(token: ReadToken) -> Self {
        token.0.to_string()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
pub struct WriteToken (Uuid);

impl Display for WriteToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<WriteToken> for String {
    fn from(token: WriteToken) -> Self {
        token.0.to_string()
    }
}

#[derive(Debug)]
pub enum SessionError {
    BadToken,
    Closed
}
