//! Restore support for auger-driver sessions.

use crate::agent::{Entry, HarnessEntry, InputEntry};
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

impl RestoreState {
    pub fn new(entries: Vec<Entry>, tail: Tail) -> Self {
        Self { entries, tail }
    }
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
    let RestoreState { mut entries, tail } = state;

    if let Tail::Incomplete { partial } = tail {
        return RestoredAgent::Interrupted(TypedAgent {
            model,
            system_prompt,
            entries,
            tools,
            state: LlmStreamingInterrupted { partial },
        });
    }

    let assistant = entries
        .iter()
        .rposition(|entry| matches!(entry, Entry::Assistant(_)));

    // Prompts after the last assistant were never sent, so drop them.
    // Tool results did run, so they stay.
    let after = assistant.map_or(0, |index| index + 1);
    let trailing = entries.split_off(after);
    entries.extend(trailing.into_iter().filter(|entry| {
        matches!(
            entry,
            Entry::Input(InputEntry::Harness(HarnessEntry::ToolResult(_)))
        )
    }));

    let awaiting_tools = match assistant.map(|index| &entries[index]) {
        Some(Entry::Assistant(response)) => !response.tool_calls().is_empty(),
        _ => false,
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