//! Events and command types for a session

use crate::session::ThreadSnapshot;
use crate::tools::tool_decisions::{Resolving, UserToolDecisions};
use auger_driver::{LlmStreamingFailed, LlmStreamingInterrupted, Resolved, StreamResult, ToolBatch, TypedAgent, WaitingForToolResponses, WaitingForUserMessage};
use provider::UserPrompt;
use std::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// User sent commands to the session
#[derive(Clone, Debug)]
pub enum SessionCommand {
    /// Send a message
    SendMessage(UserPrompt),
    /// Stop the session.
    Stop,
    /// Clone the committed conversation thread without changing session state.
    Snapshot {
        reply_tx: mpsc::Sender<ThreadSnapshot>,
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
    ToolCallResult { id: String, result: String },
    /// A tool call failed, or was denied by the user.
    ToolCallError { id: String, error: String },
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
    ToolBatchExecutionResult(ToolBatch<Resolved>)
}

/// The current state that the harness is in, with additional data as needed
pub(crate) enum HarnessState {
    /// The session is waiting for a user message
    WaitingForUserMessage {
        agent: TypedAgent<WaitingForUserMessage>,
    },
    /// LLM streaming is in progress
    Streaming { cancel: CancellationToken },
    /// Trying to interrupt the stream.
    InterruptingStream {
        pending_message: Option<UserPrompt>,
    },
    /// LLM streaming was interrupted, retaining the partial response.
    StreamingInterrupted {
        agent: TypedAgent<LlmStreamingInterrupted>,
    },
    /// LLM streaming failed, retaining the partial response.
    StreamingFailed {
        agent: TypedAgent<LlmStreamingFailed>,
    },
    /// LLM streaming came back and there are tool calls
    HasToolCalls {
        agent: TypedAgent<WaitingForToolResponses>,
    },
    /// Tool call execution is in progress
    ToolCallsAreRunning { agent: TypedAgent<WaitingForToolResponses>,  cancel: CancellationToken },
    /// Tool execution is being interrupted.
    InterruptingToolExecution {
        agent: TypedAgent<WaitingForToolResponses>,
    },
    /// Session is waiting for consent for tool calls
    NeedToolConsent {
        agent: TypedAgent<WaitingForToolResponses>,
        user_tool_decisions: UserToolDecisions<Resolving>,
    },
}

impl From<StreamResult> for HarnessState {
    fn from(result: StreamResult) -> Self {
        match result {
            StreamResult::Interrupted(agent) => Self::StreamingInterrupted { agent },
            StreamResult::Failed(agent) => Self::StreamingFailed { agent },
            StreamResult::WaitingForUserMessage(agent) => Self::WaitingForUserMessage { agent },
            StreamResult::WaitingForToolResponses(agent) => Self::HasToolCalls { agent },
        }
    }
}
