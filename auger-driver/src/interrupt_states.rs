//! States which occur if the driver's stream is interrupted.
//!
//! Interruption can either be caused by the user
//! or by the stream failing midway.

use crate::agent::State;
use crate::agent::TypedAgent;
use crate::agent::{Prompt, ReadyToStream, Turn};
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
        if let Some(partial) = self.state.partial.take() {
            self.turns.push(Turn::Output(partial));
        }
        self.push_input(msg.into());
        TypedAgent {
            model: self.model,
            tools: self.tools,
            turns: self.turns,
            system_prompt: self.system_prompt,
            state: ReadyToStream {},
        }
    }

    /// Amend the last prompt. Discards interrupted response.
    pub fn amend(mut self, msg: Prompt) -> TypedAgent<ReadyToStream> {
        if matches!(self.turns.last(), Some(Turn::Input { .. })) {
            self.turns.pop();
        }
        self.turns.push(Turn::Input {
            entries: vec![msg.into()],
        });
        TypedAgent {
            model: self.model,
            tools: self.tools,
            turns: self.turns,
            system_prompt: self.system_prompt,
            state: ReadyToStream {},
        }
    }

    /// Retry the response without the partial response
    pub fn retry(self) -> TypedAgent<ReadyToStream> {
        TypedAgent {
            model: self.model,
            tools: self.tools,
            turns: self.turns,
            system_prompt: self.system_prompt,
            state: ReadyToStream {},
        }
    }
}
