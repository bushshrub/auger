//! Persistence for auger-driver sessions
use provider::{LlmError, LlmModel, Message, StreamEvent, ToolDefinition};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use crate::{LlmStreamingFailed, LlmStreamingInterrupted, TypedAgent, WaitingForToolResponses, WaitingForUserMessage};

/// State of agent which can be persisted to disk.
#[derive(Debug, Deserialize, Serialize)]
pub enum PersistedState {
    WaitingForUserMessage,
    WaitingForToolResponses,
    Interrupted { events: Vec<StreamEvent> },
    Failed { events: Vec<StreamEvent>, error: LlmError },
}

/// An agent restored from persistent storage.
pub enum RestoredAgent {
    WaitingForUserMessage(TypedAgent<WaitingForUserMessage>),
    WaitingForToolResponses(TypedAgent<WaitingForToolResponses>),
    Interrupted(TypedAgent<LlmStreamingInterrupted>),
    Failed(TypedAgent<LlmStreamingFailed>),
}

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("The persisted state doesn't align with the last message")]
    StateMismatch
}

impl RestoredAgent {
    pub fn restore(
        model: LlmModel,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        state: PersistedState,
    ) -> Result<Self, RestoreError> {
        macro_rules! agent { ($s:expr) => {
            TypedAgent { model, messages, tools, state: $s }
        }}
        // TODO: this needs to validate messages against the state...?
        match state {
            PersistedState::WaitingForUserMessage =>
                Ok(Self::WaitingForUserMessage(agent!(WaitingForUserMessage))),
            PersistedState::WaitingForToolResponses =>
                Ok(Self::WaitingForToolResponses(agent!(WaitingForToolResponses))),
            PersistedState::Interrupted { events } =>
                Ok(Self::Interrupted(agent!(LlmStreamingInterrupted::new(events)))),
            PersistedState::Failed { events, error } =>
                Ok(Self::Failed(agent!(LlmStreamingFailed::new(events, error)))),
        }
    }
}