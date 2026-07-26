//! States which occur if the driver's stream is interrupted.
//!
//! Interruption can either be caused by the user
//! or by the stream failing midway.

use crate::ToolBatch;
use crate::agent::{Prompt, ReadyToStream};
use crate::agent::State;
use crate::agent::TypedAgent;
use getset::Getters;
use provider::{AssistantResponse};
use provider::Message;
use provider::UserPrompt;

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
        todo!()
    }

    /// Amend the last prompt. Discards interrupted response.
    pub fn amend(mut self, msg: Prompt) -> TypedAgent<ReadyToStream> {
        todo!()
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
