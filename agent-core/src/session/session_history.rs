use provider::{LlmRequest, ToolDefinition};
use crate::{SessionId, SystemPrompt};

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

    /// Create a new request and push the user message into history.
    pub(crate) fn create_request(&mut self, model: String, user_msg: String, tools: Vec<ToolDefinition>) -> LlmRequest {
        self.push_message(provider::Message::User(user_msg.clone()));
        let messages = self.messages.clone();
        LlmRequest {
            model,
            messages,
            tools,
        }
    }

    pub(crate) fn create_tool_call_response_msg(&mut self, model: String, steering_prompt: Option<String>, results: Vec<provider::ToolResult>) -> LlmRequest {
        let mut messages = self.messages.clone();
        if let Some(prompt) = steering_prompt {
            messages.push(provider::Message::User(prompt));
        }
        for result in results {
            messages.push(result.into());
        }
        LlmRequest {
            model,
            messages,
            tools: Vec::new(),
        }
    }

    pub fn messages(&self) -> &[provider::Message] {
        &self.messages
    }

}
