use crate::streaming::LlmStreaming as LlmStreamingFuture;
use getset::Getters;
use serde::{Deserialize, Serialize};
use provider::{AssistantResponse, ToolResult};
use provider::LlmModel;
use provider::Message;
use provider::ToolCallRequest;
use provider::ToolDefinition;
use provider::UserPrompt;
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

use derive_more::From;

/// A turn in the conversation. An input turn is everything the harness or the
/// user supplied before the model was asked to respond; an output turn is a
/// single response from the model.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Turn {
    Input { entries: Vec<InputEntry> },
    Output(AssistantResponse),
}

#[derive(Serialize, Deserialize, Debug, Clone, From)]
pub enum InputEntry {
    User(UserPrompt),
    Harness(HarnessEntry),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum HarnessEntry {
    ToolResult(ToolResult),
    /// A harness level message.
    Message(String)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Prompt {
    User(UserPrompt),
    Harness(String)
}

impl From<Prompt> for InputEntry {
    fn from(prompt: Prompt) -> Self {
        match prompt {
            Prompt::User(user) => InputEntry::User(user),
            Prompt::Harness(msg) => InputEntry::Harness(HarnessEntry::Message(msg))
        }
    }
}

/// Synchronous state machine for the auger driver.
/// This is the main state machine.
/// State enforced through typestates.
#[derive(Getters)]
pub struct TypedAgent<S: State> {
    pub(crate) model: LlmModel,
    pub(crate) system_prompt: String,
    /// The turns in this session, alternating input and output.
    #[get = "pub"]
    pub(crate) turns: Vec<Turn>,
    pub(crate) tools: Vec<ToolDefinition>,
    #[get = "pub"]
    pub(crate) state: S,
}

impl<S: State> TypedAgent<S> {
    /// The most recent output turn. May be None before the model has responded.
    pub(crate) fn last_output(&self) -> Option<&AssistantResponse> {
        self.turns.iter().rev().find_map(|turn| match turn {
            Turn::Output(response) => Some(response),
            _ => None,
        })
    }

    /// Add an input entry, extending the pending input turn or opening a new
    /// one if the last turn was the model's.
    pub(crate) fn push_input(&mut self, entry: InputEntry) {
        match self.turns.last_mut() {
            Some(Turn::Input { entries }) => entries.push(entry),
            _ => self.turns.push(Turn::Input {
                entries: vec![entry],
            }),
        }
    }
}

/// A state that the driver can be in.
pub trait State {}

/// The driver is waiting for a "user" message.
/// Note that "user" here can also be the harness.
/// Providing a message will begin the LLM stream and
/// transition it to the [`LlmStreaming`] state.
pub struct WaitingForUserMessage;
impl State for WaitingForUserMessage {}

impl TypedAgent<WaitingForUserMessage> {
    /// Create a new agent with the given system prompt and model.
    pub fn new(model: LlmModel, system_prompt: String, tools: Vec<ToolDefinition>) -> Self {
        let turns = Vec::new();
        let state = WaitingForUserMessage {};
        Self {
            system_prompt,
            turns,
            model,
            tools,
            state,
        }
    }

    /// Get the previous assistant message that occurred before this state.
    /// May be `None` if this is the first turn in the session.
    pub fn previous_message(&self) -> Option<&AssistantResponse> {
        let last_turn = self.turns.last()?;
        match last_turn {
            Turn::Output(response) => Some(response),
            _ => panic!(
                "auger driver state invariant violation: last turn should be an output turn when \
                 in WaitingForUserMessage state"
            ),
        }
    }

    /// Add a user message to the driver and transition it to the
    /// [`ReadyToStream`] state.
    pub fn add_message(mut self, msg: Prompt) -> TypedAgent<ReadyToStream> {
        self.push_input(msg.into());
        let state = ReadyToStream {};
        TypedAgent {
            model: self.model,
            system_prompt: self.system_prompt,
            turns: self.turns,
            tools: self.tools,
            state,
        }
    }
}

/// The driver is ready to begin streaming the LLM response.
pub struct ReadyToStream {}

impl State for ReadyToStream {}

impl TypedAgent<ReadyToStream> {
    /// Creates an interruptible LLM stream future.
    pub fn create_stream(
        self,
        cb: impl Fn(provider::StreamEvent) + Send + Sync + 'static,
    ) -> LlmStreamingFuture {
        let cancellation = CancellationToken::new();

        LlmStreamingFuture::new(
            self.model,
            self.system_prompt,
            self.tools,
            self.turns,
            Box::new(cb),
            cancellation,
        )
    }
}

pub(crate) fn convert_turns_into_messages(turns: Vec<Turn>) -> Vec<Message> {
    turns
        .into_iter()
        .map(|turn| match turn {
            Turn::Input { entries } => {
                let mut message = String::new();
                let mut tool_call_results = Vec::new();
                for entry in entries {
                    match entry {
                        InputEntry::User(prompt) => message.push_str(&prompt.message),
                        InputEntry::Harness(HarnessEntry::Message(text)) => {
                            message.push_str(&text)
                        }
                        InputEntry::Harness(HarnessEntry::ToolResult(result)) => {
                            tool_call_results.push(result)
                        }
                    }
                }
                Message::User {
                    message: UserPrompt::new(message),
                    tool_call_results,
                }
            }
            Turn::Output(response) => response.into(),
        })
        .collect()
}

/// Get the tool calls from the most recent output turn that are still waiting
/// for a result. Only the most recent output turn can have these.
pub(crate) fn pending_tool_calls(turns: &[Turn]) -> Vec<ToolCallRequest> {
    let mut iter = turns.iter().rev();
    let (answered, response) = match iter.next() {
        Some(Turn::Input { entries }) => {
            let answered: HashSet<&str> = entries
                .iter()
                .filter_map(|entry| match entry {
                    InputEntry::Harness(HarnessEntry::ToolResult(result)) => {
                        Some(result.tool_call_id.as_str())
                    }
                    _ => None,
                })
                .collect();
            match iter.next() {
                Some(Turn::Output(response)) => (answered, response),
                _ => return Vec::new(),
            }
        }
        Some(Turn::Output(response)) => (HashSet::new(), response),
        None => return Vec::new(),
    };

    response
        .tool_calls()
        .into_iter()
        .filter(|call| !answered.contains(call.id.as_str()))
        .collect()
}
