//! Events and command types for a session

use std::collections::HashSet;
use tokio_util::sync::CancellationToken;
use auger_driver::{ReadyToStream, Resolved, TypedAgent, WaitingForToolResponses, WaitingForUserMessage};
use provider::{LlmThread, UserPrompt};
use provider::thread::UserTurn;
use crate::tools::tool_decisions::UserToolDecisions;

/// User sent commands to the session
#[derive(Clone, Debug)]
pub enum SessionCommand {
    /// Send a message
    SendMessage(UserPrompt),
    /// Interrupt the current activity on the stream
    Interrupt,
    /// Make a decision on a tool.
    ToolDecision {
        id: String,
        approved: bool,
        message: Option<String>,
    },
}

/// Events that occur during the session
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// A provider event emitted while the LLM is streaming.
    StreamEvent(provider::StreamEvent),
    /// The session has stopped and will not emit further events.
    Closed,
}

pub(crate) enum LoopEvent {
    /// User commands
    Cmd(SessionCommand),
    /// Internal state transition
    StateTransition(HarnessState)
}

/// The current state that the harness is in, with additional data as needed
pub(crate) enum HarnessState {
    /// The session is waiting for a user message
    WaitingForUserMessage { agent: TypedAgent<WaitingForUserMessage> },
    /// The session is ready to stream
    ReadyToStream { agent: TypedAgent<ReadyToStream> },
    /// LLM streaming is in progress
    Streaming { cancel: CancellationToken },
    /// LLM streaming came back and there are tool calls
    HasToolCalls { agent: TypedAgent<WaitingForToolResponses>},
    /// Tool call execution is in progress
    WaitingForToolResults { cancel: CancellationToken },
    /// Session is waiting for consent for tool calls
    NeedsUserConsent { agent: TypedAgent<WaitingForToolResponses>, user_tool_decisions: UserToolDecisions },
}