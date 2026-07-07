use provider::{TokenUsage, UserPrompt};
use serde::{Deserialize, Serialize};
use std::sync::mpsc::SyncSender;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Image {
    Url(String),
    Base64(String),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserMessage {
    pub(crate) msg: String,
    pub(crate) images: Vec<Image>,
}

impl UserMessage {
    pub fn new(msg: String) -> Self {
        Self {
            msg,
            images: vec![],
        }
    }
}

impl From<UserMessage> for UserPrompt {
    fn from(msg: UserMessage) -> Self {
        // todo: ignores images
        UserPrompt::new(msg.msg)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolCallResponse {
    Approve,
    Deny,
}

/// Actions that the user can take directly in a session
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UserAction {
    /// Send a message to the clanker
    SendMessage(UserMessage),
    /// Respond to a tool call that was requested by the clanker
    RespondToToolCall {
        response: ToolCallResponse,
        tool_call_id: String,
        message: Option<String>,
    },
    /// Stop the current turn. Pending tool calls are resolved as
    /// interrupted; the session parks until the user sends a message,
    /// which travels back together with the interrupted results.
    Interrupt,
}

#[derive(Clone, Debug)]
pub(crate) enum UserCommand {
    Action(UserAction),
    Snapshot {
        reply: SyncSender<Vec<provider::Message>>,
    },
}

impl From<UserAction> for UserCommand {
    fn from(action: UserAction) -> Self {
        UserCommand::Action(action)
    }
}

/// Event caused by a clanker response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClankerEvent {
    ReasoningDelta {
        delta: String,
    },
    ContentDelta {
        delta: String,
    },
    ToolCallRequest {
        id: String,
        name: String,
        arguments: String,
    },
    Done {
        usage: Option<TokenUsage>,
        stop_reason: Option<String>,
    },
}

/// Events in the lifecycle of a requested tool call: how it was decided
/// (auto-approved / denied / interrupted; user approval is echoed via
/// `SessionEvent::User`) and how it completed (result / error).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolCallEvent {
    // -- decision events -------------------------------------------------
    AutoApproved {
        id: String,
        name: String,
        arguments: String,
    },
    Denied {
        id: String,
        reason: Option<String>,
    },
    /// The call was pending when the user interrupted the turn and was
    /// resolved as "interrupted".
    Interrupted {
        id: String,
    },
    // -- completion events -------------------------------------------------
    Result {
        id: String,
        result: String,
    },
    Error {
        id: String,
        // TODO: bad type
        error: String,
    },
}

/// The resting states of the session automaton, as reported to clients.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum StateKind {
    /// Waiting for the user to send a message.
    Ready,
    /// The LLM turn / agent turn cycle is running.
    Generating,
    /// Gated tool calls await user approval.
    AwaitingApproval,
    /// The turn was interrupted with tool calls pending; waiting for a
    /// user message to submit alongside the interrupted results.
    Interrupted,
}

/// Session-level events: state transitions, rejections, and errors.
/// Together with `ClankerEvent` and `ToolCallEvent`, these make the event
/// stream a complete projection source for clients.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LifecycleEvent {
    StateChanged {
        state: StateKind,
    },
    /// A command was not legal in the current state and was ignored.
    CommandRejected {
        reason: String,
    },
    ProviderError {
        message: String,
    },
    Closed,
}

/// Events
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SessionEvent {
    User(UserAction),
    Clanker(ClankerEvent),
    /// Event in the lifecycle of a requested tool call.
    ToolCall(ToolCallEvent),
    /// Session-level event: state transition, rejection, or error.
    Lifecycle(LifecycleEvent),
}

impl From<UserAction> for SessionEvent {
    fn from(action: UserAction) -> Self {
        SessionEvent::User(action)
    }
}

impl From<ClankerEvent> for SessionEvent {
    fn from(event: ClankerEvent) -> Self {
        SessionEvent::Clanker(event)
    }
}

impl From<ToolCallEvent> for SessionEvent {
    fn from(event: ToolCallEvent) -> Self {
        SessionEvent::ToolCall(event)
    }
}

impl From<LifecycleEvent> for SessionEvent {
    fn from(event: LifecycleEvent) -> Self {
        SessionEvent::Lifecycle(event)
    }
}
