//! The various states that the session can be in

use either::Either;
use uuid::Uuid;
use provider::{ClankerMessage, LlmRequest, LlmThread, ToolDefinition, ToolResult, UserPrompt};
use provider::thread::{AddToolResultError, ClankerTurn, ToolResultsPending, UserTurn};

pub trait State: private::Sealed {}

mod private {
    use super::*;

    pub trait Sealed {}

    impl Sealed for Idle {}
    impl Sealed for LlmTurnRunning {}

    impl Sealed for AwaitingHostFeedback {}
}

struct SessionState<S: State> {
    /// The session ID.
    id: Uuid,
    /// The actual state data
    state: S,
}


struct Idle {
    thread: LlmThread<UserTurn>
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

struct LlmTurnRunning {
    thread: LlmThread<ClankerTurn>
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

    /// Abandon this model turn without committing an assistant message.
    pub fn abandon_llm_turn(self) -> SessionState<Idle> {
        SessionState {
            id: self.id,
            state: Idle {
                thread: self.state.thread.abandon_clanker_turn(),
            },
        }
    }
}

/// The state after the LLM response has fully generated,
/// and there are tool calls that need results.
///
/// The host is expected to execute the tool calls,
/// and then provide the results for the tool calls.
struct AwaitingHostFeedback {
    thread: LlmThread<ToolResultsPending>,
}

impl State for AwaitingHostFeedback {}

impl SessionState<AwaitingHostFeedback> {
    pub fn add_steering_prompt(self, prompt: UserPrompt) -> SessionState<AwaitingHostFeedback> {
        SessionState {
            id: self.id,
            state: AwaitingHostFeedback {
                thread: self.state.thread.add_steering_message(prompt),
            },
        }
    }

    /// Add a tool result. Self-transitions if there are still
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
}
