//! Restore support for auger-driver sessions.

use crate::{LlmStreamingFailed, LlmStreamingInterrupted, TypedAgent, WaitingForToolResponses, WaitingForUserMessage};
use provider::{LlmError, LlmModel, Message, StreamEvent, ToolDefinition};
use serde::{Deserialize, Serialize};

/// Driver state reconstructed by the persistence owner.
#[derive(Debug, Serialize, Deserialize)]
pub enum RestoreState {
    WaitingForUserMessage {
        messages: Vec<Message>,
    },
    WaitingForToolResponses {
        messages: Vec<Message>,
    },
    Interrupted {
        messages: Vec<Message>,
        events: Vec<StreamEvent>,
    },
    Failed {
        messages: Vec<Message>,
        events: Vec<StreamEvent>,
        error: LlmError,
    },
}

/// An agent restored from persistent state.
pub enum RestoredAgent {
    WaitingForUserMessage(TypedAgent<WaitingForUserMessage>),
    WaitingForToolResponses(TypedAgent<WaitingForToolResponses>),
    Interrupted(TypedAgent<LlmStreamingInterrupted>),
    Failed(TypedAgent<LlmStreamingFailed>),
}

/// Restore an agent into the state selected by the persistence owner.
pub fn restore(
    model: LlmModel,
    tools: Vec<ToolDefinition>,
    state: RestoreState,
) -> RestoredAgent {
    macro_rules! agent {
        ($messages:expr, $state:expr) => {
            TypedAgent {
                model,
                messages: $messages,
                tools,
                state: $state,
            }
        };
    }

    match state {
        RestoreState::WaitingForUserMessage { messages } => {
            RestoredAgent::WaitingForUserMessage(agent!(messages, WaitingForUserMessage))
        }
        RestoreState::WaitingForToolResponses { messages } => {
            RestoredAgent::WaitingForToolResponses(agent!(messages, WaitingForToolResponses))
        }
        RestoreState::Interrupted { messages, events } => {
            RestoredAgent::Interrupted(agent!(messages, LlmStreamingInterrupted::new(events)))
        }
        RestoreState::Failed { messages, events, error } => {
            RestoredAgent::Failed(agent!(messages, LlmStreamingFailed::new(events, error)))
        }
    }
}
