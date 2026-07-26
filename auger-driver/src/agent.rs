use crate::streaming::LlmStreaming as LlmStreamingFuture;
use getset::Getters;
use serde::{Deserialize, Serialize};
use provider::{AssistantResponse, ToolResult};
use provider::LlmModel;
use provider::Message;
use provider::ToolDefinition;
use provider::UserPrompt;
use tokio_util::sync::CancellationToken;

use derive_more::From;

/// An item in the conversation.
#[derive(Serialize, Deserialize, Debug, Clone, From)]
pub enum Entry {
    Input(InputEntry),
    Assistant(AssistantResponse),
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
    /// The entries in this session. The order is as follows:
    /// entries := (run Entry::Assistant)* run?
    /// run := (Entry::User | Entry::Harness)+
    #[get = "pub"]
    pub(crate) entries: Vec<Entry>,
    pub(crate) tools: Vec<ToolDefinition>,
    #[get = "pub"]
    pub(crate) state: S,
}

impl<S: State> TypedAgent<S> {
    /// Retrieve the last assistant response that we've seen, and its index.
    /// May be None.
    pub(crate) fn previous_assistant_at(&self) -> Option<(usize, &AssistantResponse)> {
        let index = self
            .entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::Assistant(_)))?;
        match &self.entries[index] {
            Entry::Assistant(assistant) => Some((index, assistant)),
            _ => None,
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
        let entries = Vec::new();
        let state = WaitingForUserMessage {};
        Self {
            system_prompt,
            entries,
            model,
            tools,
            state,
        }
    }

    /// Get the previous assistant message that occurred before this state.
    /// May be `None` if this is the first turn in the session.
    pub fn previous_message(&self) -> Option<&AssistantResponse> {
        let last_entry = self.entries.last()?;
        match last_entry {
            Entry::Assistant(assistant) => Some(assistant),
            _ => panic!(
                "auger driver state invariant violation: last message should be an assistant \
                 message when in WaitingForUserMessage state"
            ),
        }
    }

    /// Add a user message to the driver and transition it to the
    /// [`ReadyToStream`] state.
    pub fn add_message(mut self, msg: Prompt) -> TypedAgent<ReadyToStream> {
        self.entries.push(Entry::Input(msg.into()));
        let state = ReadyToStream {};
        TypedAgent {
            model: self.model,
            system_prompt: self.system_prompt,
            entries: self.entries,
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
            self.entries,
            Box::new(cb),
            cancellation,
        )
    }
}

pub(crate) fn convert_entries_into_messages(entries: Vec<Entry>) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut user_message = String::new();
    let mut tool_call_results = Vec::new();
    let mut has_user_entry = false;

    for entry in entries {
        match entry {
            Entry::Input(input) => {
                has_user_entry = true;
                match input {
                    InputEntry::User(prompt) => {
                        user_message.push_str(&prompt.message);
                    }
                    InputEntry::Harness(harness_entry) => {
                        match harness_entry {
                            HarnessEntry::ToolResult(result) => {
                                tool_call_results.push(result);
                            }
                            HarnessEntry::Message(message) => {
                                user_message.push_str(&message)
                            }
                        }
                    }
                }
            }
            Entry::Assistant(response) => {
                if has_user_entry {
                    messages.push(Message::User {
                        message: UserPrompt::new(std::mem::take(&mut user_message)),
                        tool_call_results: std::mem::take(&mut tool_call_results),
                    });
                    has_user_entry = false;
                }
                messages.push(response.into());
            }
        }
    }

    if has_user_entry {
        messages.push(Message::User {
            message: UserPrompt::new(user_message),
            tool_call_results,
        });
    }

    messages
}
