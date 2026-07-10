//! The various states that the session can be in

use crate::events::{PartialModelResponse, ToolCallRequest};
use crate::tool_call_batch::{Resolving, ToolCallBatch, ToolCallBatchError};
use either::Either;
use provider::thread::{ClankerTurn, ToolResultsPending, UserTurn};
use provider::{
    ClankerMessage, LlmRequest, LlmResponse, LlmThread, Message, StreamEvent, ToolDefinition,
    ToolResult, UserPrompt,
};
use std::fmt;
use uuid::Uuid;

pub trait State: private::Sealed {
    fn messages(&self) -> &[Message];
}

mod private {
    use super::*;

    pub trait Sealed {}

    impl Sealed for Idle {}
    impl Sealed for LlmTurnRunning {}

    impl Sealed for AwaitingHostFeedback {}
    impl Sealed for AwaitingInterruptedUserMessage {}
    impl Sealed for ResponseError {}
}

pub(crate) struct SessionState<S: State> {
    /// The session ID.
    id: Uuid,
    /// The actual state data
    state: S,
}

impl<S: State> SessionState<S> {
    pub(crate) fn messages(&self) -> Vec<Message> {
        self.state.messages().to_vec()
    }
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
            .field("tool_call_batch", &self.state.tool_call_batch)
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

impl fmt::Debug for SessionState<ResponseError> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionState<ResponseError>")
            .field("id", &self.id)
            .field("thread", &self.state.thread)
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct Idle {
    thread: LlmThread<UserTurn>,
}
impl State for Idle {
    fn messages(&self) -> &[Message] {
        self.thread.messages()
    }
}

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
impl State for LlmTurnRunning {
    fn messages(&self) -> &[Message] {
        self.thread.messages()
    }
}

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
                state: AwaitingHostFeedback {
                    tool_call_batch: ToolCallBatch::new(thread.get_pending_tool_calls()),
                    thread,
                },
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

    /// Discard a failed model turn without committing partial model output.
    pub fn fail_llm_turn(self) -> SessionState<ResponseError> {
        SessionState {
            id: self.id,
            state: ResponseError {
                thread: self.state.thread.abandon_clanker_turn(),
            },
        }
    }

    /// Commit useful partial assistant output from an interrupted model turn.
    pub fn interrupt_llm_turn(
        self,
        partial_response: Vec<StreamEvent>,
    ) -> (SessionState<ResponseError>, PartialModelResponse) {
        let response = LlmResponse::from(partial_response);
        let partial =
            PartialModelResponse::new(response.content.clone(), response.reasoning.clone());

        if partial.is_empty() {
            return (self.fail_llm_turn(), partial);
        }

        let response = LlmResponse {
            content: response.content,
            reasoning: response.reasoning,
            tool_calls: None,
            usage: None,
            stop_reason: None,
        };

        let state = match self.add_llm_response(ClankerMessage::from(response)) {
            Either::Left(state) => SessionState {
                id: state.id,
                state: ResponseError {
                    thread: state.state.thread,
                },
            },
            Either::Right(_) => {
                unreachable!("interrupted model turns do not commit tool calls")
            }
        };

        (state, partial)
    }

    /// Interrupt a user-cancelled model turn, preserving partial output.
    pub fn user_interrupt_llm_turn(
        self,
        partial_response: Vec<StreamEvent>,
    ) -> Either<SessionState<Idle>, SessionState<AwaitingInterruptedUserMessage>> {
        if partial_response.is_empty() {
            return Either::Left(SessionState {
                id: self.id,
                state: Idle {
                    thread: self.state.thread.abandon_clanker_turn(),
                },
            });
        }

        match self.add_llm_response(ClankerMessage::from(LlmResponse::from(partial_response))) {
            Either::Left(state) => Either::Left(state),
            Either::Right(state) => Either::Right(state.interrupt_pending_tool_calls()),
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
    tool_call_batch: ToolCallBatch<Resolving>,
}

impl State for AwaitingHostFeedback {
    fn messages(&self) -> &[Message] {
        self.thread.messages()
    }
}

impl SessionState<AwaitingHostFeedback> {
    pub fn add_steering_prompt(self, prompt: UserPrompt) -> SessionState<AwaitingHostFeedback> {
        SessionState {
            id: self.id,
            state: AwaitingHostFeedback {
                thread: self.state.thread.add_steering_message(prompt),
                tool_call_batch: self.state.tool_call_batch,
            },
        }
    }

    pub fn requested_tool_calls(&self) -> Vec<ToolCallRequest> {
        self.state
            .tool_call_batch
            .requested()
            .into_iter()
            .map(ToolCallRequest::from_provider)
            .collect()
    }

    /// Adds multiple tool results.
    pub fn add_tool_results(
        self,
        results: Vec<ToolResult>,
    ) -> Result<Either<Self, SessionState<LlmTurnRunning>>, (Self, ToolCallBatchError)> {
        let SessionState {
            id,
            state:
                AwaitingHostFeedback {
                    thread,
                    tool_call_batch,
                },
        } = self;

        match tool_call_batch.resolve_many(results) {
            Ok(Either::Left(tool_call_batch)) => Ok(Either::Left(SessionState {
                id,
                state: AwaitingHostFeedback {
                    thread,
                    tool_call_batch,
                },
            })),
            Ok(Either::Right(tool_call_batch)) => {
                let mut thread = thread;
                for result in tool_call_batch.into_results() {
                    thread = match thread.add_tool_result(result) {
                        Ok(Either::Left(next)) => next,
                        Ok(Either::Right(thread)) => {
                            return Ok(Either::Right(SessionState {
                                id,
                                state: LlmTurnRunning { thread },
                            }));
                        }
                        Err(err) => {
                            unreachable!("tool batch accepted an invalid result: {err}");
                        }
                    };
                }

                unreachable!("complete tool batch did not complete the provider thread")
            }
            Err((tool_call_batch, err)) => Err((
                SessionState {
                    id,
                    state: AwaitingHostFeedback {
                        thread,
                        tool_call_batch,
                    },
                },
                err,
            )),
        }
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

impl State for AwaitingInterruptedUserMessage {
    fn messages(&self) -> &[Message] {
        self.thread.messages()
    }
}

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

/// The state after a model response failed.
///
/// The loop has abandoned the active model turn, optionally preserving partial
/// assistant output, and waits for the next user message to continue.
pub(crate) struct ResponseError {
    thread: LlmThread<UserTurn>,
}

impl State for ResponseError {
    fn messages(&self) -> &[Message] {
        self.thread.messages()
    }
}

impl SessionState<ResponseError> {
    pub fn add_user_message(self, prompt: UserPrompt) -> SessionState<LlmTurnRunning> {
        SessionState {
            id: self.id,
            state: LlmTurnRunning {
                thread: self.state.thread.add_user_message(prompt),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_interrupt_marks_partial_tool_calls_interrupted_on_next_user_message() {
        let state = SessionState::<Idle>::new("system".to_string())
            .add_user_message(UserPrompt::new("use a tool".to_string()));

        let interrupted = state.user_interrupt_llm_turn(vec![StreamEvent::ToolCallComplete {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            arguments: "{}".to_string(),
        }]);

        let Either::Right(state) = interrupted else {
            panic!("partial tool calls should wait for interrupted user message");
        };

        let state = state.add_user_message(UserPrompt::new("continue".to_string()));
        let request = state.create_request(Vec::new());

        assert!(matches!(
            &request.messages()[2],
            Message::Assistant { tool_calls, .. }
                if tool_calls.len() == 1
                    && tool_calls[0].id == "call_1"
                    && tool_calls[0].name == "read_file"
                    && tool_calls[0].arguments == "{}"
        ));
        assert!(matches!(
            &request.messages()[3],
            Message::User {
                message,
                tool_call_results,
            } if message.message() == "continue"
                && tool_call_results.len() == 1
                && tool_call_results[0].id() == "call_1"
                && tool_call_results[0].content() == "aborted"
        ));
    }
}
