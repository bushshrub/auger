//! Restore support for auger-driver sessions.

use getset::Getters;
use crate::LlmStreamingInterrupted;
use crate::TypedAgent;
use crate::WaitingForToolResponses;
use crate::WaitingForUserMessage;
use provider::{AssistantResponse, LlmError, PartialLlmResponse};
use provider::LlmModel;
use provider::Message;
use provider::StreamEvent;
use provider::ToolDefinition;
use serde::Deserialize;
use serde::Serialize;
use crate::agent::Entry;

/// Driver state reconstructed by the persistence owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreState {
    entries: Vec<Entry>,
    tail: Tail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Tail {
    Settled,
    Incomplete { partial: Option<AssistantResponse> }
}


/// An agent restored from persistent state.
pub enum RestoredAgent {
    WaitingForUserMessage(TypedAgent<WaitingForUserMessage>),
    WaitingForToolResponses(TypedAgent<WaitingForToolResponses>),
    Interrupted(TypedAgent<LlmStreamingInterrupted>),
}

/// Restore an agent into the state selected by the persistence owner.
pub fn restore(model: LlmModel, system_prompt: String, tools: Vec<ToolDefinition>, state: RestoreState) -> RestoredAgent {
    todo!()
}