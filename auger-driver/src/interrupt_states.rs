//! States which occur if the driver's stream is interrupted.
//!
//! Interruption can either be caused by the user
//! or by the stream failing midway.

use crate::agent::{ReadyToStream, State, TypedAgent};
use getset::Getters;
use provider::thread::ClankerTurn;
use provider::{LlmThread, UserPrompt};

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

impl TypedAgent<LlmStreamingInterrupted> {
    /// Add a new user message.
    /// Choose whether the stream should be left with the partial response or not.
    pub fn add_message_to_continue(
        self,
        msg: UserPrompt,
        leave_partial_response: bool,
    ) -> TypedAgent<ReadyToStream> {
        let thread = if leave_partial_response {
            let response = provider::LlmResponse::from(self.state.events);
            let reply = provider::ClankerMessage::from(response);

            match self.state.thread.add_clanker_reply(reply) {
                either::Either::Left(thread) => thread.add_user_message(msg),
                either::Either::Right(thread) => thread.abort_pending_tool_calls(msg),
            }
        } else {
            self.state
                .thread
                .abandon_clanker_turn()
                .add_user_message(msg)
        };

        TypedAgent {
            model: self.model,
            tools: self.tools,
            state: ReadyToStream::new(thread),
        }
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

impl TypedAgent<LlmStreamingFailed> {
    /// Add a new user message after abandoning the failed partial response.
    pub fn add_message_to_continue(self, msg: UserPrompt) -> TypedAgent<ReadyToStream> {
        let thread = self
            .state
            .thread
            .abandon_clanker_turn()
            .add_user_message(msg);
        TypedAgent {
            model: self.model,
            tools: self.tools,
            state: ReadyToStream::new(thread),
        }
    }

    /// Retry the response without the partial response
    pub fn retry(self) -> TypedAgent<ReadyToStream> {
        TypedAgent {
            model: self.model,
            tools: self.tools,
            state: ReadyToStream::new(self.state.thread),
        }
    }
}
