//! States which occur if the driver's stream is interrupted.
//!
//! Interruption can either be caused by the user
//! or by the stream failing midway.

use crate::agent::{InputEntry, State};
use crate::agent::TypedAgent;
use crate::agent::{Entry, HarnessEntry, Prompt, ReadyToStream};
use getset::Getters;
use provider::AssistantResponse;

/// The LLM stream was interrupted midway.
/// This could be either caused by the user cancelling it,
/// or by some kind of provider failure.
#[derive(Getters)]
pub struct LlmStreamingInterrupted {
    pub(super) partial: Option<AssistantResponse>
}

impl State for LlmStreamingInterrupted {}

impl TypedAgent<LlmStreamingInterrupted> {

    /// Leaves the assistant message in and continues with the given prompt
    pub fn seal_and_continue(
        mut self,
        msg: Prompt,
    ) -> TypedAgent<ReadyToStream> {
        if let Some(partial) = self.state.partial {
            self.entries.push(Entry::Assistant(partial));
        }
        self.entries.push(Entry::Input(msg.into()));
        TypedAgent {
            model: self.model,
            tools: self.tools,
            entries: self.entries,
            system_prompt: self.system_prompt,
            state: ReadyToStream {},
        }
    }

    /// Amend the last prompt. Discards interrupted response.
    pub fn amend(mut self, msg: Prompt) -> TypedAgent<ReadyToStream> {
        while matches!(
            self.entries.last(),
            Some(Entry::Input(entry)) if matches!(entry, InputEntry::User(_) | InputEntry::Harness(_))
        ) {
            self.entries.pop();
        }
        self.entries.push(Entry::Input(msg.into()));
        TypedAgent {
            model: self.model,
            tools: self.tools,
            entries: self.entries,
            system_prompt: self.system_prompt,
            state: ReadyToStream {},
        }
    }

    /// Retry the response without the partial response
    pub fn retry(self) -> TypedAgent<ReadyToStream> {
        TypedAgent {
            model: self.model,
            tools: self.tools,
            entries: self.entries,
            system_prompt: self.system_prompt,
            state: ReadyToStream {},
        }
    }
}
