use crate::{
    ClankerMessage, LlmRequest, Message, ToolCallRequest, ToolDefinition, ToolResult, UserPrompt,
};
use either::Either;
use std::marker::PhantomData;
use thiserror::Error;

/// The user's turn to the model, which may include a prompt and/or tool calls.
pub struct UserTurn;
/// After the model has responded, it may have made tool calls,
/// which MUST be all responded to.
pub struct ToolResultsPending {
    /// An optional steering prompt.
    steering_prompt: Option<UserPrompt>,
    /// Tool results that have been provided so far.
    tool_results: Vec<ToolResult>,
}

impl ToolResultsPending {
    fn new() -> Self {
        ToolResultsPending {
            steering_prompt: None,
            tool_results: Vec::new(),
        }
    }
}
/// State when it is time for the model to respond.
/// In this state the llm thread should be sent to the model.
pub struct ClankerTurn;
pub trait LlmThreadState: private::Sealed {}

impl LlmThreadState for UserTurn {}
impl LlmThreadState for ClankerTurn {}

impl LlmThreadState for ToolResultsPending {}

mod private {
    use super::*;

    pub trait Sealed {}

    impl Sealed for UserTurn {}
    impl Sealed for ClankerTurn {}

    impl Sealed for ToolResultsPending {}
}

/// A conversation thread with the LLM.
pub struct LlmThread<S: LlmThreadState> {
    messages: Vec<Message>,
    _state: S,
}

/// Any conversation thread with the LLM, regardless of state
pub enum AnyThread {
    /// User's turn to send a message to the model
    User(LlmThread<UserTurn>),
    /// User needs to respond to tool calls made by the model
    ToolsPending(LlmThread<ToolResultsPending>),
    /// Model's turn to respond to the user
    Clanker(LlmThread<ClankerTurn>),
}

impl From<LlmThread<UserTurn>> for AnyThread {
    fn from(thread: LlmThread<UserTurn>) -> Self {
        AnyThread::User(thread)
    }
}

impl From<LlmThread<ToolResultsPending>> for AnyThread {
    fn from(thread: LlmThread<ToolResultsPending>) -> Self {
        AnyThread::ToolsPending(thread)
    }
}

impl From<LlmThread<ClankerTurn>> for AnyThread {
    fn from(thread: LlmThread<ClankerTurn>) -> Self {
        AnyThread::Clanker(thread)
    }
}

impl<S: LlmThreadState> LlmThread<S> {
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Export the messages in this thread.
    pub fn export(self) -> Vec<Message> {
        self.messages.clone()
    }
}

impl LlmThread<UserTurn> {
    /// Start a new thread with the LLM, beginning with the system prompt
    pub fn new(system_prompt: String) -> Self {
        Self {
            messages: vec![Message::System(system_prompt)],
            _state: UserTurn,
        }
    }

    /// Add a user prompt to the thread.
    pub fn add_user_message(self, prompt: UserPrompt) -> LlmThread<ClankerTurn> {
        let mut messages = self.messages;
        messages.push(prompt.into());
        LlmThread {
            messages,
            _state: ClankerTurn,
        }
    }
}

impl LlmThread<ToolResultsPending> {
    /// Get the list of tool calls that are pending results.
    pub fn get_pending_tool_calls(&self) -> Vec<ToolCallRequest> {
        let mut tool_calls = Vec::new();
        // get last message
        if let Some(Message::Assistant {
            tool_calls: calls, ..
        }) = self.messages.last()
        {
            tool_calls.extend(calls.clone());
        } else {
            // shouldn't be possible I think...
            panic!("No assistant message found in thread");
        }
        tool_calls
    }

    pub fn add_steering_message(self, prompt: UserPrompt) -> LlmThread<ToolResultsPending> {
        LlmThread {
            messages: self.messages,
            _state: ToolResultsPending {
                steering_prompt: Some(prompt),
                tool_results: self._state.tool_results,
            },
        }
    }

    /// Add a tool result to the thread. An optional steering message can be added
    /// to the thread to guide the model's next response.
    pub fn add_tool_result(
        self,
        tool_result: ToolResult,
    ) -> Result<Either<Self, LlmThread<ClankerTurn>>, AddToolResultError> {
        let pending_tool_calls = self.get_pending_tool_calls();
        if !pending_tool_calls
            .iter()
            .any(|call| call.id == tool_result.id())
        {
            return Err(AddToolResultError::ToolNotRequested(
                tool_result.id().to_string(),
            ));
        }

        let mut tool_results = self._state.tool_results;
        tool_results.push(tool_result);

        let all_results_provided = pending_tool_calls
            .iter()
            .all(|call| tool_results.iter().any(|result| result.id() == call.id));

        if !all_results_provided {
            return Ok(Either::Left(LlmThread {
                messages: self.messages,
                _state: ToolResultsPending {
                    steering_prompt: self._state.steering_prompt,
                    tool_results,
                },
            }));
        }

        let mut messages = self.messages;
        let steering_message = if let Some(prompt) = self._state.steering_prompt {
            prompt
        } else {
            UserPrompt::new(String::new())
        };

        let msg = Message::User {
            message: steering_message,
            tool_call_results: tool_results,
        };
        messages.push(msg);
        Ok(Either::Right(LlmThread {
            messages,
            _state: ClankerTurn,
        }))
    }
}
#[derive(Debug, Error)]
pub enum AddToolResultError {
    /// Tool result passed in has an ID which was not requested.
    #[error("Tool result ID {0} was not requested")]
    ToolNotRequested(String),
}

impl LlmThread<ClankerTurn> {
    /// Abandon the model's turn without adding an assistant response.
    pub fn abandon_clanker_turn(self) -> LlmThread<UserTurn> {
        LlmThread {
            messages: self.messages,
            _state: UserTurn,
        }
    }

    /// Add a response from the LLM to the user. The harness
    /// will invoke this method when it receives a response from the LLM.
    ///
    /// If the response contains tool calls, the thread will transition to the ToolResultsPending state.
    /// Otherwise, it will transition to the UserTurn state.
    pub fn add_clanker_reply(
        self,
        clanker_response: ClankerMessage,
    ) -> Either<LlmThread<UserTurn>, LlmThread<ToolResultsPending>> {
        let mut messages = self.messages;
        let has_tool_calls = !&clanker_response.tool_calls.is_empty();
        messages.push(clanker_response.into());
        if has_tool_calls {
            Either::Right(LlmThread {
                messages,
                _state: ToolResultsPending::new(),
            })
        } else {
            Either::Left(LlmThread {
                messages,
                _state: UserTurn,
            })
        }
    }

    /// Create an LLM request from the thread. This is used to send the thread to the LLM for processing.
    pub fn create_request(&self, tools: Vec<ToolDefinition>) -> LlmRequest {
        LlmRequest::new(self.messages.clone(), tools)
    }
}
