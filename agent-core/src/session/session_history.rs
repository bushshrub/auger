use std::marker::PhantomData;
use provider::{LlmRequest, ToolDefinition};
use crate::{SessionId, SystemPrompt};

mod private {
    pub trait Sealed {}
}

pub(crate) trait TurnState: private::Sealed {}

pub(crate) struct NeedsInput;
pub(crate) struct ReadyToSend;

impl private::Sealed for NeedsInput {}
impl private::Sealed for ReadyToSend {}
impl TurnState for NeedsInput {}
impl TurnState for ReadyToSend {}

pub(crate) struct TurnBuilder<'h, S: TurnState> {
    history: &'h mut SessionHistory,
    model: String,
    tools: Vec<ToolDefinition>,
    _state: PhantomData<S>,
}

pub struct SessionHistory {
    id: SessionId,
    system_prompt: SystemPrompt,
    messages: Vec<provider::Message>,
}

impl SessionHistory {
    pub fn new(id: SessionId, system_prompt: SystemPrompt) -> Self {
        let mut hist = Self { id, system_prompt: system_prompt.clone(), messages: Vec::new() };
        hist.push_message(provider::Message::System(system_prompt.into()));
        hist
    }

    pub fn push_message(&mut self, message: provider::Message) {
        self.messages.push(message);
    }

    pub fn push_llm_response(&mut self, resp: provider::LlmResponse) {
        self.push_message(resp.into());
    }

    pub(crate) fn begin_user_turn(&mut self, model: String, tools: Vec<ToolDefinition>) -> TurnBuilder<'_, NeedsInput> {
        TurnBuilder { history: self, model, tools, _state: PhantomData }
    }

    pub(crate) fn begin_tool_turn(&mut self, model: String, tools: Vec<ToolDefinition>) -> TurnBuilder<'_, NeedsInput> {
        TurnBuilder { history: self, model, tools, _state: PhantomData }
    }

    pub fn messages(&self) -> &[provider::Message] {
        &self.messages
    }
}

impl<'h> TurnBuilder<'h, NeedsInput> {
    pub(crate) fn with_user_message(self, msg: String) -> TurnBuilder<'h, ReadyToSend> {
        self.history.push_message(provider::Message::User(msg));
        TurnBuilder { history: self.history, model: self.model, tools: self.tools, _state: PhantomData }
    }

    pub(crate) fn with_tool_results(self, steering: Option<String>, results: Vec<provider::ToolResult>) -> TurnBuilder<'h, ReadyToSend> {
        if let Some(prompt) = steering {
            self.history.push_message(provider::Message::User(prompt));
        }
        for result in results {
            self.history.push_message(result.into());
        }
        TurnBuilder { history: self.history, model: self.model, tools: self.tools, _state: PhantomData }
    }
}

impl<'h> TurnBuilder<'h, ReadyToSend> {
    pub(crate) fn build(self) -> LlmRequest {
        LlmRequest {
            model: self.model,
            messages: self.history.messages.clone(),
            tools: self.tools,
        }
    }
}
