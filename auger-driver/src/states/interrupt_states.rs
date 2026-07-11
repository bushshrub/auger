//! States which occur if the driver's stream is interrupted.
//!
//! Interruption can either be caused by the user
//! or by the stream failing midway.

use provider::{LlmThread, UserPrompt};
use getset::Getters;
use provider::thread::ClankerTurn;
use crate::driver::{Agent, ReadyToStream, State};

/// The LLM stream was interrupted midway.
#[derive(Getters)]
pub struct LlmStreamingInterrupted {
    thread: LlmThread<ClankerTurn>,
    #[getset(get = "pub")]
    events: Vec<provider::StreamEvent>,
}

impl State for LlmStreamingInterrupted {}

impl LlmStreamingInterrupted {
    pub(crate) fn new(thread: LlmThread<ClankerTurn>, events: Vec<provider::StreamEvent>) -> Self {
        Self { thread, events }
    }
}

impl Agent<LlmStreamingInterrupted> {
    /// Add a new user message.
    /// Choose whether the stream should be left with the partial response or not.
    pub fn add_message_to_continue(self, msg: UserPrompt, leave_partial_response: bool) -> Agent<ReadyToStream> {
        todo!()
    }
}

/// The LLM stream failed midway.
#[derive(Getters)]
pub struct LlmStreamingFailed {
    thread: LlmThread<ClankerTurn>,
    #[getset(get = "pub")]
    events: Vec<provider::StreamEvent>,
}

impl State for LlmStreamingFailed {}

impl LlmStreamingFailed {
    pub(crate) fn new(thread: LlmThread<ClankerTurn>, events: Vec<provider::StreamEvent>) -> Self {
        Self { thread, events }
    }
}

impl Agent<LlmStreamingFailed> {
    /// Retry the response without the partial response
    pub fn retry(self) -> Agent<ReadyToStream> {
        todo!()
    }
}
