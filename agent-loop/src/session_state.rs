//! The various states that the session can be in

use either::Either;
use provider::thread::{AddToolResultError, ClankerTurn, ToolResultsPending, UserTurn};
use provider::{
    ClankerMessage, LlmRequest, LlmResponse, LlmThread, StreamEvent, ToolDefinition, ToolResult,
    UserPrompt,
};
use std::fmt;
use uuid::Uuid;

pub trait State: private::Sealed {}

mod private {
    use super::*;

    pub trait Sealed {}

    impl Sealed for Idle {}
    impl Sealed for LlmTurnRunning {}

    impl Sealed for AwaitingHostFeedback {}
    impl Sealed for AwaitingInterruptedUserMessage {}
}

pub(crate) struct SessionState<S: State> {
    /// The session ID.
    id: Uuid,
    /// The actual state data
    state: S,
}

impl fmt::Debug for SessionState<Idle> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionState<Idle>")
            .field("id", &self.id)
            .field("thread", &self.state.thread)
            .finish()
    }
}

impl fmt::Debug for SessionState<LlmTurnRunning> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionState<LlmTurnRunning>")
            .field("id", &self.id)
            .field("thread", &self.state.thread)
            .finish()
    }
}

impl fmt::Debug for SessionState<AwaitingHostFeedback> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionState<AwaitingHostFeedback>")
            .field("id", &self.id)
            .field("thread", &self.state.thread)
            .finish()
    }
}

impl fmt::Debug for SessionState<AwaitingInterruptedUserMessage> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionState<AwaitingInterruptedUserMessage>")
            .field("id", &self.id)
            .field("thread", &self.state.thread)
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct Idle {
    thread: LlmThread<UserTurn>,
}
impl State for Idle {}

impl SessionState<Idle> {
    /// Begin a new session with the given system prompt.
    pub fn new(system_prompt: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            state: Idle {
                thread: LlmThread::new(system_prompt),
            },
        }
    }

    /// Add user message and transition to the LlmTurnRunning state.
    /// Runtime is responsible for submitting the request and streaming the response.
    pub fn add_user_message(self, prompt: UserPrompt) -> SessionState<LlmTurnRunning> {
        SessionState {
            id: self.id,
            state: LlmTurnRunning {
                thread: self.state.thread.add_user_message(prompt),
            },
        }
    }
}

pub(crate) struct LlmTurnRunning {
    thread: LlmThread<ClankerTurn>,
}
impl State for LlmTurnRunning {}

impl SessionState<LlmTurnRunning> {
    /// Create the provider request for this model turn.
    pub fn create_request(&self, tools: Vec<ToolDefinition>) -> LlmRequest {
        self.state.thread.create_request(tools)
    }

    /// Commit a complete model response and transition to the next session state.
    ///
    /// If the model responded with no tool calls, we move into an idle state.
    /// If the model responded with tool calls, we wait into a state
    /// that awaits for the session host to provide responses to the tool calls.
    ///
    /// # Note
    /// This agentic loop library doesn't prescribe how the session host
    /// should respond to tool calls, only that they do.
    pub fn add_llm_response(
        self,
        response: ClankerMessage,
    ) -> Either<SessionState<Idle>, SessionState<AwaitingHostFeedback>> {
        match self.state.thread.add_clanker_reply(response) {
            Either::Left(thread) => Either::Left(SessionState {
                id: self.id,
                state: Idle { thread },
            }),
            Either::Right(thread) => Either::Right(SessionState {
                id: self.id,
                state: AwaitingHostFeedback { thread },
            }),
        }
    }

    /// Abandon this model turn, committing a partial assistant message if one exists.
    pub fn abandon_llm_turn(
        self,
        partial_response: Vec<StreamEvent>,
    ) -> Either<SessionState<Idle>, SessionState<AwaitingHostFeedback>> {
        if partial_response.is_empty() {
            return Either::Left(SessionState {
                id: self.id,
                state: Idle {
                    thread: self.state.thread.abandon_clanker_turn(),
                },
            });
        }

        self.add_llm_response(ClankerMessage::from(LlmResponse::from(partial_response)))
    }

    pub fn abandon_and_add_user_message(self, prompt: UserPrompt) -> SessionState<LlmTurnRunning> {
        SessionState {
            id: self.id,
            state: LlmTurnRunning {
                thread: self
                    .state
                    .thread
                    .abandon_clanker_turn()
                    .add_user_message(prompt),
            },
        }
    }
}

/// The state after the LLM response has fully generated,
/// and there are tool calls that need results.
///
/// The host is expected to execute the tool calls,
/// and then provide the results for the tool calls.
pub(crate) struct AwaitingHostFeedback {
    thread: LlmThread<ToolResultsPending>,
}

impl State for AwaitingHostFeedback {}

impl SessionState<AwaitingHostFeedback> {
    pub fn pending_tool_calls(&self) -> Vec<provider::ToolCallRequest> {
        self.state.thread.get_pending_tool_calls()
    }

    pub fn add_steering_prompt(self, prompt: UserPrompt) -> SessionState<AwaitingHostFeedback> {
        SessionState {
            id: self.id,
            state: AwaitingHostFeedback {
                thread: self.state.thread.add_steering_message(prompt),
            },
        }
    }

    /// Add a single tool result. Self-transitions if there are still
    /// more tool results to deal with, otherwise moves into a state which
    /// indicates it is time for the LLM to respond.
    /// Errors if the tool result passed in wasn't requested.
    pub fn add_tool_result(
        self,
        result: ToolResult,
    ) -> Result<Either<Self, SessionState<LlmTurnRunning>>, AddToolResultError> {
        match self.state.thread.add_tool_result(result)? {
            Either::Left(thread) => Ok(Either::Left(SessionState {
                id: self.id,
                state: AwaitingHostFeedback { thread },
            })),
            Either::Right(thread) => Ok(Either::Right(SessionState {
                id: self.id,
                state: LlmTurnRunning { thread },
            })),
        }
    }

    /// Validate that all tool results correspond to pending tool calls.
    pub fn validate_tool_results(&self, results: &[ToolResult]) -> Result<(), AddToolResultError> {
        for result in results {
            self.state.thread.validate_tool_result(result)?;
        }

        Ok(())
    }

    /// Adds multiple tool results.
    pub fn add_tool_results(
        self,
        results: Vec<ToolResult>,
    ) -> Result<Either<Self, SessionState<LlmTurnRunning>>, AddToolResultError> {
        let mut state = self;
        for result in results {
            state = match state.add_tool_result(result) {
                Ok(Either::Left(s)) => s,
                // todo: this disposes invalid tool results at the end, is that okay?
                Ok(Either::Right(s)) => return Ok(Either::Right(s)),
                Err(e) => return Err(e),
            };
        }
        Ok(Either::Left(state))
    }

    /// Abort every pending tool call and wait for the next user message.
    pub fn interrupt_pending_tool_calls(self) -> SessionState<AwaitingInterruptedUserMessage> {
        SessionState {
            id: self.id,
            state: AwaitingInterruptedUserMessage {
                thread: self.state.thread,
            },
        }
    }
}

/// The state after the session has interrupted tool call handling
/// and is waiting for the next user message to ride back with aborted tool results.
pub(crate) struct AwaitingInterruptedUserMessage {
    thread: LlmThread<ToolResultsPending>,
}

impl State for AwaitingInterruptedUserMessage {}

impl SessionState<AwaitingInterruptedUserMessage> {
    pub fn add_user_message(self, prompt: UserPrompt) -> SessionState<LlmTurnRunning> {
        SessionState {
            id: self.id,
            state: LlmTurnRunning {
                thread: self.state.thread.abort_pending_tool_calls(prompt),
            },
        }
    }
}
