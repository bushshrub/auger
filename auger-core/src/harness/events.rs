//! Events and command types for a session

use crate::record::Record;
use auger_driver::Prompt;
use auger_driver::StreamResult;
use auger_toolengine::ToolMessage;
use std::sync::mpsc;

/// User sent commands to the session
#[derive(Clone, Debug)]
pub enum SessionCommand {
    /// Send a message
    SendMessage(Prompt),
    /// Stop the session.
    Stop { reply_tx: mpsc::Sender<()> },
    /// Interrupt the current activity on the stream
    Interrupt,
    /// Make a decision on a tool.
    ToolDecision {
        id: String,
        approved: bool,
        message: Option<String>,
    },
}

/// Events that occur during the session
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// A record, just appended to the session's sink. Folding these gives
    /// the same state as folding a stored trace.
    Record(Record),
    /// A provider event emitted while the LLM is streaming.
    StreamEvent(provider::StreamEvent),
    /// Tool calls that require a user approval or denial decision.
    ToolConsentRequired {
        tool_calls: Vec<provider::ToolCallRequest>,
    },
    /// The tool phase ended with a denied or interrupted call. The results go
    /// back to the model with the next user message; until then the session
    /// waits for input.
    ToolResultsWithheld,
    /// The session has stopped and will not emit further events.
    Closed,
}

pub(crate) enum LoopMessage {
    /// User commands
    Cmd(SessionCommand),
    /// A streaming future completed.
    StreamResult(StreamResult),
    /// A message from a tool task, forwarded to the tool phase engine.
    Tool(ToolMessage),
}
