//! Events and command types for a session

use crate::session::SessionRecord;
use crate::tools::tool_execution::ToolCallResult;
use crate::tools::tool_execution::ToolExecutionCompleted;
use auger_driver::Resolved;
use auger_driver::Resolving;
use auger_driver::StreamResult;
use auger_driver::ToolBatch;
use provider::UserPrompt;
use std::sync::mpsc;

/// User sent commands to the session
#[derive(Clone, Debug)]
pub enum SessionCommand {
    /// Send a message
    SendMessage(UserPrompt),
    /// Stop the session.
    Stop { reply_tx: mpsc::Sender<()> },
    /// Clone the recorded session trace without changing session state.
    Snapshot {
        reply_tx: mpsc::Sender<SessionRecord>,
    },
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
    /// A provider event emitted while the LLM is streaming.
    StreamEvent(provider::StreamEvent),
    /// Tool calls that require a user approval or denial decision.
    ToolConsentRequired {
        tool_calls: Vec<provider::ToolCallRequest>,
    },
    /// A tool call finished executing and produced a result.
    ToolCallResult(ToolCallResult),
    /// The in-flight LLM stream was interrupted; the session is waiting for
    /// user input with the partial response retained.
    Interrupted,
    /// The LLM stream failed; the session is waiting for a new user message.
    StreamError { error: String },
    /// The session has stopped and will not emit further events.
    Closed,
}

pub(crate) enum LoopMessage {
    /// User commands
    Cmd(SessionCommand),
    /// A streaming future completed.
    StreamResult(StreamResult),
    /// A tool batch has executed and returned its results
    ToolBatchExecutionResult {
        batch: ToolBatch<Resolving>,
        results: ToolExecutionCompleted,
    },
}
