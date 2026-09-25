use auger_driver::LlmStreamingInterrupted;
use auger_driver::RestoredAgent;
use auger_driver::StreamResult;
use auger_driver::TypedAgent;
use auger_driver::WaitingForToolResponses;
use auger_driver::WaitingForUserMessage;
use auger_driver::Prompt;
use tokio_util::sync::CancellationToken;

/// The current state that the harness is in, with additional data as needed
pub(crate) enum HarnessState {
    /// The session is waiting for a user message
    WaitingForUserMessage {
        agent: TypedAgent<WaitingForUserMessage>,
    },
    /// LLM streaming is in progress
    Streaming { cancel: CancellationToken },
    /// Trying to interrupt the stream.
    InterruptingStream { pending_message: Option<Prompt> },
    /// LLM streaming was interrupted, retaining the partial response.
    StreamingInterrupted {
        agent: TypedAgent<LlmStreamingInterrupted>,
    },
    /// LLM streaming failed, retaining the partial response.
    StreamingFailed {
        agent: TypedAgent<LlmStreamingInterrupted>,
    },
    /// The model called tools. Every call must be answered before the
    /// results go back to the model.
    ToolPhase {
        agent: TypedAgent<WaitingForToolResponses>,
    },
}

impl From<StreamResult> for HarnessState {
    fn from(result: StreamResult) -> Self {
        match result {
            StreamResult::Interrupted(agent) => Self::StreamingInterrupted { agent },
            StreamResult::Failed { agent, .. } => Self::StreamingFailed { agent },
            StreamResult::WaitingForUserMessage { agent, .. } => Self::WaitingForUserMessage { agent },
            StreamResult::WaitingForToolResponses { agent, .. } => Self::ToolPhase { agent },
        }
    }
}

impl From<RestoredAgent> for HarnessState {
    fn from(agent: RestoredAgent) -> Self {
        match agent {
            RestoredAgent::WaitingForUserMessage(agent) => {
                HarnessState::WaitingForUserMessage { agent }
            }
            RestoredAgent::WaitingForToolResponses(agent) => {
                HarnessState::ToolPhase { agent }
            }
            RestoredAgent::Interrupted(agent) => HarnessState::StreamingInterrupted { agent },
        }
    }
}
