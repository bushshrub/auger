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
    /// Incomplete assistant turn. `None` both when the
    /// last turn settled and when the stream died before producing anything.
    partial: Option<AssistantResponse>,
}

impl RestoreState {
    pub fn new(entries: Vec<Entry>, partial: Option<AssistantResponse>) -> Self {
        Self { entries, partial }
    }
}


/// An agent restored from persistent state.
pub enum RestoredAgent {
    WaitingForUserMessage(TypedAgent<WaitingForUserMessage>),
    WaitingForToolResponses(TypedAgent<WaitingForToolResponses>),
    Interrupted(TypedAgent<LlmStreamingInterrupted>),
}

/// Restore an agent from persisted conversation entries.
///
/// The state is derived from the shape of `entries`, in order:
/// 1. the last assistant requested tool calls -> [`WaitingForToolResponses`]
/// 2. a partial is present, or the entries trail off in an input run that no
///    assistant answered -> [`LlmStreamingInterrupted`]
/// 3. otherwise -> [`WaitingForUserMessage`]
///
/// Rule 2 also covers a crash between recording a prompt and opening the
/// stream: the prompt is kept rather than dropped, so `retry` resends it.
/// Rule 1 outranks it deliberately, since sealing a partial while tool calls
/// are unanswered would send a `tool_use` block that never gets a result.
pub fn restore(model: LlmModel, system_prompt: String, tools: Vec<ToolDefinition>, state: RestoreState) -> RestoredAgent {
    let RestoreState { entries, partial } = state;

    let assistant = entries
        .iter()
        .rposition(|entry| matches!(entry, Entry::Assistant(_)));
    let after = assistant.map_or(0, |index| index + 1);

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
    } else if partial.is_some() || entries.len() > after {
        RestoredAgent::Interrupted(TypedAgent {
            model,
            system_prompt,
            entries,
            tools,
            state: LlmStreamingInterrupted { partial },
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