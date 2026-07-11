use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use provider::{LlmModel, StreamEvent, ToolDefinition, UserPrompt};
use tokio_util::sync::CancellationToken;

use crate::agent::{TypedAgent, WaitingForUserMessage};
use crate::interrupt_states::{LlmStreamingFailed, LlmStreamingInterrupted};
use crate::streaming::{LlmStreaming, StreamResult};
use crate::tool_batch::{Resolved, Resolving, ToolBatch};
use crate::waiting_for_tools::WaitingForToolResponses;

/// The current externally visible state of an [`Agent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    WaitingForUserMessage,
    WaitingForToolResponses,
    Interrupted,
    Failed,
}

enum AgentInner {
    WaitingForUserMessage(TypedAgent<WaitingForUserMessage>),
    WaitingForToolResponses(TypedAgent<WaitingForToolResponses>),
    Interrupted(TypedAgent<LlmStreamingInterrupted>),
    Failed(TypedAgent<LlmStreamingFailed>),
}

/// Non-generic facade over the driver's typestate transitions.
pub struct Agent {
    inner: AgentInner,
}

impl Agent {
    pub fn new(
        model: LlmModel,
        system_prompt: impl Into<String>,
        tools: Vec<ToolDefinition>,
    ) -> Self {
        Self {
            inner: AgentInner::WaitingForUserMessage(TypedAgent::new(
                model,
                system_prompt.into(),
                tools,
            )),
        }
    }

    pub fn status(&self) -> AgentStatus {
        match self.inner {
            AgentInner::WaitingForUserMessage(_) => AgentStatus::WaitingForUserMessage,
            AgentInner::WaitingForToolResponses(_) => AgentStatus::WaitingForToolResponses,
            AgentInner::Interrupted(_) => AgentStatus::Interrupted,
            AgentInner::Failed(_) => AgentStatus::Failed,
        }
    }

    pub fn pending_tools(&self) -> Option<ToolBatch<Resolving>> {
        match &self.inner {
            AgentInner::WaitingForToolResponses(agent) => Some(agent.get_batch()),
            _ => None,
        }
    }

    /// Events received before an interruption or stream failure.
    pub fn partial_events(&self) -> Option<&[StreamEvent]> {
        match &self.inner {
            AgentInner::Interrupted(agent) => Some(agent.state.events()),
            AgentInner::Failed(agent) => Some(agent.state.events()),
            _ => None,
        }
    }

    pub fn send_message(
        self,
        message: UserPrompt,
        on_event: impl Fn(StreamEvent) + Send + Sync + 'static,
    ) -> Result<AgentStream, InvalidTransition> {
        match self.inner {
            AgentInner::WaitingForUserMessage(agent) => Ok(AgentStream::new(
                agent
                    .add_message(message)
                    .add_event_callback(on_event)
                    .create_stream(),
            )),
            other => Err(InvalidTransition::new(
                Self { inner: other },
                AgentStatus::WaitingForUserMessage,
            )),
        }
    }

    pub fn submit_tool_results(
        self,
        results: ToolBatch<Resolved>,
        on_event: impl Fn(StreamEvent) + Send + Sync + 'static,
    ) -> Result<AgentStream, InvalidTransition> {
        match self.inner {
            AgentInner::WaitingForToolResponses(agent) => Ok(AgentStream::new(
                agent
                    .add_all_tool_responses(results)
                    .add_event_callback(on_event)
                    .create_stream(),
            )),
            other => Err(InvalidTransition::new(
                Self { inner: other },
                AgentStatus::WaitingForToolResponses,
            )),
        }
    }

    /// Continue after an interrupted response with a new user message.
    pub fn continue_after_interruption(
        self,
        message: UserPrompt,
        leave_partial_response: bool,
        on_event: impl Fn(StreamEvent) + Send + Sync + 'static,
    ) -> Result<AgentStream, InvalidTransition> {
        match self.inner {
            AgentInner::Interrupted(agent) => Ok(AgentStream::new(
                agent
                    .add_message_to_continue(message, leave_partial_response)
                    .add_event_callback(on_event)
                    .create_stream(),
            )),
            other => Err(InvalidTransition::new(
                Self { inner: other },
                AgentStatus::Interrupted,
            )),
        }
    }

    /// Retry a failed response from the beginning of the model turn.
    pub fn retry_after_failure(
        self,
        on_event: impl Fn(StreamEvent) + Send + Sync + 'static,
    ) -> Result<AgentStream, InvalidTransition> {
        match self.inner {
            AgentInner::Failed(agent) => Ok(AgentStream::new(
                agent.retry().add_event_callback(on_event).create_stream(),
            )),
            other => Err(InvalidTransition::new(
                Self { inner: other },
                AgentStatus::Failed,
            )),
        }
    }

    /// Continue after a failed response with a new user message.
    pub fn continue_after_failure(
        self,
        message: UserPrompt,
        on_event: impl Fn(StreamEvent) + Send + Sync + 'static,
    ) -> Result<AgentStream, InvalidTransition> {
        match self.inner {
            AgentInner::Failed(agent) => Ok(AgentStream::new(
                agent
                    .add_message_to_continue(message)
                    .add_event_callback(on_event)
                    .create_stream(),
            )),
            other => Err(InvalidTransition::new(
                Self { inner: other },
                AgentStatus::Failed,
            )),
        }
    }
}

impl From<StreamResult> for Agent {
    fn from(result: StreamResult) -> Self {
        let inner = match result {
            StreamResult::WaitingForUserMessage(agent) => AgentInner::WaitingForUserMessage(agent),
            StreamResult::WaitingForToolResponses(agent) => {
                AgentInner::WaitingForToolResponses(agent)
            }
            StreamResult::Interrupted(agent) => AgentInner::Interrupted(agent),
            StreamResult::Failed(agent) => AgentInner::Failed(agent),
        };
        Self { inner }
    }
}

/// A rejected operation together with the unchanged agent.
pub struct InvalidTransition {
    agent: Agent,
    expected: AgentStatus,
}

impl std::fmt::Debug for InvalidTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InvalidTransition")
            .field("actual", &self.actual())
            .field("expected", &self.expected)
            .finish()
    }
}

impl InvalidTransition {
    fn new(agent: Agent, expected: AgentStatus) -> Self {
        Self { agent, expected }
    }

    pub fn agent(self) -> Agent {
        self.agent
    }

    pub fn actual(&self) -> AgentStatus {
        self.agent.status()
    }

    pub fn expected(&self) -> AgentStatus {
        self.expected
    }
}

/// An interruptible stream that returns the ergonomic agent facade.
pub struct AgentStream {
    cancellation: CancellationToken,
    inner: Pin<Box<dyn Future<Output = Agent> + Send>>,
}

impl AgentStream {
    fn new(stream: LlmStreaming) -> Self {
        let cancellation = stream.interrupt_handle();
        Self {
            cancellation,
            inner: Box::pin(async move { Agent::from(stream.await) }),
        }
    }

    pub fn interrupt_handle(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

impl Future for AgentStream {
    type Output = Agent;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}
