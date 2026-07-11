//! Events and command types for a session

use auger_driver::StreamResult;
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
    /// A provider event emitted while the LLM is streaming.
    StreamEvent(provider::StreamEvent),
    /// The session has stopped and will not emit further events.
    Closed,
}

pub(crate) enum LoopEvent {
    Cmd(SessionCommand),
    StreamCompletion(StreamResult)
}
