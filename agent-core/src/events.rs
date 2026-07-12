//! Events and command types for a session

use crate::tools::tool_decisions::{Resolving, ToolAuthorization, UserToolDecisions};
use auger_driver::{LlmStreaming, LlmStreamingFailed, LlmStreamingInterrupted, ReadyToStream, Resolved, StreamResult, ToolBatch, TypedAgent, WaitingForToolResponses, WaitingForUserMessage};
use provider::UserPrompt;
use tokio_util::sync::CancellationToken;

/// User sent commands to the session
#[derive(Clone, Debug)]
pub enum SessionCommand {
    /// Send a message
    SendMessage(UserPrompt),
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
    /// The session is ready to stream
    ReadyToStream { agent: TypedAgent<ReadyToStream> },
    /// LLM streaming is in progress
    Streaming { cancel: CancellationToken },
    /// Trying to interrupt the stream.
    InterruptingStream,
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
    /// All tools have a decision and we are ready to run tools
    ReadyToRunTools {
        agent: TypedAgent<WaitingForToolResponses>,
        authorization: ToolAuthorization,
    },
    /// Tool call execution is in progress
    ToolCallsAreRunning { agent: TypedAgent<WaitingForToolResponses>,  cancel: CancellationToken },
    /// Tool calls are being interrupted
    InterruptingToolCalls,
    /// Tool calls have been executed and have responses.
    ToolResultsReady {
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
