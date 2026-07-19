use auger_driver::{LlmStreamingFailed, LlmStreamingInterrupted, RestoredAgent, StreamResult, TypedAgent, WaitingForToolResponses, WaitingForUserMessage};
use tokio_util::sync::CancellationToken;
use provider::UserPrompt;
use crate::tools::tool_decisions::{Resolving, UserToolDecisions};

/// States which a session can be restored from
pub(crate) enum RestorableState {
    /// The session is waiting for a user message
    WaitingForUserMessage {
        agent: TypedAgent<WaitingForUserMessage>,
    },
    /// Session is waiting for consent for tool calls
    NeedToolConsent {
        agent: TypedAgent<WaitingForToolResponses>,
        user_tool_decisions: UserToolDecisions<Resolving>,
    },
    /// LLM streaming was interrupted, retaining the partial response.
    StreamingInterrupted {
        agent: TypedAgent<LlmStreamingInterrupted>,
    },
    /// LLM streaming failed, retaining the partial response.
    StreamingFailed {
        agent: TypedAgent<LlmStreamingFailed>,
    },
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
        _agent: TypedAgent<WaitingForToolResponses>,
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
            StreamResult::WaitingForToolResponses(agent) => Self::HasToolCalls { _agent: agent },
        }
    }
}

impl From<RestoredAgent> for HarnessState {
    fn from(agent: RestoredAgent) -> Self {
        match agent {
            RestoredAgent::WaitingForUserMessage(agent) => HarnessState::WaitingForUserMessage { agent },
            RestoredAgent::WaitingForToolResponses(agent) => HarnessState::HasToolCalls { _agent: agent },
            RestoredAgent::Interrupted(agent) => HarnessState::StreamingInterrupted { agent },
            RestoredAgent::Failed(agent) => HarnessState::StreamingFailed { agent },
        }
    }
}