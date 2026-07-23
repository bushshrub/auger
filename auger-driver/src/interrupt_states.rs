//! States which occur if the driver's stream is interrupted.
//!
//! Interruption can either be caused by the user
//! or by the stream failing midway.

use crate::agent::{ReadyToStream, State, TypedAgent};
use crate::ToolBatch;
use getset::Getters;
use provider::{ClankerMessage, LlmResponse, Message, UserPrompt};

/// The LLM stream was interrupted midway.
#[derive(Getters)]
pub struct LlmStreamingInterrupted {
    #[getset(get = "pub")]
    events: Vec<provider::StreamEvent>,
}

impl State for LlmStreamingInterrupted {}

impl LlmStreamingInterrupted {
    pub(crate) fn new(events: Vec<provider::StreamEvent>) -> Self {
        Self {  events }
    }
}

impl TypedAgent<LlmStreamingInterrupted> {

    /// Add a new user message.
    /// Choose whether the stream should be left with the partial response or not.
    pub fn add_message_to_continue(
        mut self,
        msg: UserPrompt,
        leave_partial_response: bool,
    ) -> TypedAgent<ReadyToStream> {
        let user_message = if leave_partial_response {
            let response = LlmResponse::from_events(self.state.events);
            let reply = ClankerMessage::from(response);
            // TODO: Should marking the remaining tool calls be the responsibility of the driver?
            let tool_call_results = if !reply.tool_calls().is_empty() {
                ToolBatch::new(reply.tool_calls().to_vec()).interrupt_remaining().drain()
            } else {
                Vec::new()
            };
            Message::User { message: msg, tool_call_results }
        } else {
            msg.into()
        };

        self.messages.push(user_message);

        TypedAgent {
            model: self.model,
            tools: self.tools,
            messages: self.messages,
            state: ReadyToStream {},
        }
    }
}

/// The LLM stream failed midway.
#[derive(Getters)]
pub struct LlmStreamingFailed {
    #[getset(get = "pub")]
    events: Vec<provider::StreamEvent>,
    #[getset(get = "pub")]
    error: provider::LlmError,
}

impl State for LlmStreamingFailed {}

impl LlmStreamingFailed {
    pub(crate) fn new(
        events: Vec<provider::StreamEvent>,
        error: provider::LlmError,
    ) -> Self {
        Self {events, error }
    }
}

impl TypedAgent<LlmStreamingFailed> {
    /// The provider error that caused the stream to fail.
    pub fn error(&self) -> &provider::LlmError {
        self.state.error()
    }

    /// Add a new user message after abandoning the failed partial response.
    pub fn add_message_to_continue(mut self, msg: UserPrompt) -> TypedAgent<ReadyToStream> {
        self.messages.push(msg.into());
        TypedAgent {
            model: self.model,
            tools: self.tools,
            messages: self.messages,
            state: ReadyToStream {},
        }
    }

    /// Retry the response without the partial response
    pub fn retry(self) -> TypedAgent<ReadyToStream> {
        TypedAgent {
            model: self.model,
            tools: self.tools,
            messages: self.messages,
            state: ReadyToStream {},
        }
    }
}
