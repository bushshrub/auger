//! States which occur if the driver's stream is interrupted.
//!
//! Interruption can either be caused by the user
//! or by the stream failing midway.

use crate::ToolBatch;
use crate::agent::{Prompt, ReadyToStream};
use crate::agent::State;
use crate::agent::TypedAgent;
use getset::Getters;
use provider::{LlmResponse, PartialLlmResponse};
use provider::Message;
use provider::UserPrompt;

/// The LLM stream was interrupted midway.
#[derive(Getters)]
pub struct LlmStreamingInterrupted {
    pub(super) partial: PartialLlmResponse
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
}

/// The LLM stream failed midway.
pub struct LlmStreamingFailed {
    /// A possible partial response
    pub(super) partial: Option<PartialLlmResponse>,
    pub(super) error: provider::LlmError,
}

impl State for LlmStreamingFailed {}



impl TypedAgent<LlmStreamingFailed> {
    /// The provider error that caused the stream to fail.
    pub fn error(&self) -> &provider::LlmError {
        &self.state.error
    }

    /// Amends the previous "user" message before continuing
    pub fn amend(mut self, msg: Prompt) -> TypedAgent<ReadyToStream> {
        todo!("get rid of last entry")
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
