use crate::harness::tool_engine::ToolPhaseEngine;
use auger_driver::LlmStreamingInterrupted;
use auger_driver::Resolved;
use auger_driver::ToolBatch;
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
        engine: ToolPhaseEngine,
    },
    /// The tool phase ended with a denied or interrupted call. The results
    /// go back to the model with the next user message.
    ToolResultsWithheld {
        agent: TypedAgent<WaitingForToolResponses>,
        batch: ToolBatch<Resolved>,
    },
}
