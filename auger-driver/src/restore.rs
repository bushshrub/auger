//! Restore support for auger-driver sessions.

use crate::agent::Entry;
use crate::LlmStreamingInterrupted;
use crate::TypedAgent;
use crate::WaitingForToolResponses;
use crate::WaitingForUserMessage;
use provider::LlmModel;
use provider::ToolDefinition;
use provider::AssistantResponse;
use serde::Deserialize;
use serde::Serialize;

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
    let RestoreState { entries, tail } = state;

    if let Tail::Incomplete { partial } = tail {
        return RestoredAgent::Interrupted(TypedAgent {
            model,
            system_prompt,
            entries,
            tools,
            state: LlmStreamingInterrupted { partial },
        });
    }

    let awaiting_tools = match entries.last() {
        None => false,
        Some(Entry::Assistant(response)) => !response.tool_calls().is_empty(),
        Some(_) => panic!("settled session must end on an assistant message"),
    };

    if awaiting_tools {
        RestoredAgent::WaitingForToolResponses(TypedAgent {
            model,
            system_prompt,
            entries,
            tools,
            state: WaitingForToolResponses {},
        })
    } else {
        RestoredAgent::WaitingForUserMessage(TypedAgent {
            model,
            system_prompt,
            entries,
            tools,
            state: WaitingForUserMessage {},
        })
    }
}