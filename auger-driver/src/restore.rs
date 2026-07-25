//! Restore support for auger-driver sessions.

use crate::LlmStreamingFailed;
use crate::LlmStreamingInterrupted;
use crate::TypedAgent;
use crate::WaitingForToolResponses;
use crate::WaitingForUserMessage;
use provider::LlmError;
use provider::LlmModel;
use provider::Message;
use provider::StreamEvent;
use provider::ToolDefinition;
use serde::Deserialize;
use serde::Serialize;
use crate::agent::Entry;

/// Driver state reconstructed by the persistence owner.
#[derive(Debug, Serialize)]
pub enum RestoreState {
    WaitingForUserMessage {
        entries: Vec<Entry>,
    },
    WaitingForToolResponses {
        entries: Vec<Entry>,
    },
    Interrupted {
        entries: Vec<Entry>,
        events: Vec<StreamEvent>,
    },
    Failed {
        entries: Vec<Entry>,
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
pub fn restore(model: LlmModel, tools: Vec<ToolDefinition>, state: RestoreState) -> RestoredAgent {
    todo!()
}