//! Events and command types for a session

use provider::UserPrompt;
use crate::tools::tool_call_batch::ToolCallId;

/// User sent commands to the session
#[derive(Clone, Debug)]
pub enum SessionCommand {
    /// Send a message
    SendMessage(UserPrompt),
    /// Interrupt the current activity on the stream
    Interrupt,
    ApproveToolCall {
        id: ToolCallId,
    },
    DenyToolCall {
        id: ToolCallId
    }
}

/// Events that occur during the session
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// The session has stopped and will not emit further events.
    Closed,
}
