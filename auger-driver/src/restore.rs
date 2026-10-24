//! Restore support for auger-driver sessions.

use crate::agent::{pending_tool_calls, HarnessEntry, InputEntry, Turn};
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
    turns: Vec<Turn>,
    /// Incomplete assistant turn. `None` both when the
    /// last turn settled and when the stream died before producing anything.
    partial: Option<AssistantResponse>,
}

impl RestoreState {
    pub fn new(turns: Vec<Turn>, partial: Option<AssistantResponse>) -> Self {
        Self { turns, partial }
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
/// The state is derived from the shape of `turns`, in order:
/// 1. the last output turn left tool calls unanswered ->
///    [`WaitingForToolResponses`]
/// 2. the last turn is an input turn the model never answered ->
///    [`LlmStreamingInterrupted`]
/// 3. otherwise -> [`WaitingForUserMessage`]
///
/// Rule 2 also covers a crash between recording a prompt and opening the
/// stream: the prompt is kept rather than dropped, so `retry` resends it.
/// Rule 1 outranks it deliberately, since sealing a partial while tool calls
/// are unanswered would send a `tool_use` block that never gets a result.
pub fn restore(model: LlmModel, system_prompt: String, tools: Vec<ToolDefinition>, state: RestoreState) -> RestoredAgent {
    let RestoreState { turns, partial } = state;

    let last_output = turns.iter().rposition(|turn| matches!(turn, Turn::Output(_)));
    for (index, turn) in turns.iter().enumerate() {
        let Turn::Output(response) = turn else { continue };
        if Some(index) == last_output {
            continue;
        }
        assert!(
            response.tool_calls().iter().all(|call| {
                matches!(turns.get(index + 1), Some(Turn::Input { entries }) if entries.iter().any(
                    |entry| matches!(entry, InputEntry::Harness(HarnessEntry::ToolResult(result))
                        if result.tool_call_id == call.id),
                ))
            }),
            "restored turn {index} left tool calls unanswered; only the most recent output turn \
             may have pending calls"
        );
    }

    if !pending_tool_calls(&turns).is_empty() {
        RestoredAgent::WaitingForToolResponses(TypedAgent {
            model,
            system_prompt,
            turns,
            tools,
            state: WaitingForToolResponses {},
        })
    } else if matches!(turns.last(), Some(Turn::Input { .. })) {
        RestoredAgent::Interrupted(TypedAgent {
            model,
            system_prompt,
            turns,
            tools,
            state: LlmStreamingInterrupted { partial },
        })
    } else {
        RestoredAgent::WaitingForUserMessage(TypedAgent {
            model,
            system_prompt,
            turns,
            tools,
            state: WaitingForUserMessage {},
        })
    }
}