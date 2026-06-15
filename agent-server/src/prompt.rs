use std::sync::Arc;

use agent_tools::Tool;
use provider::{Message, Role};

#[derive(Clone)]
pub struct SystemPrompt {
    base: String,
}

impl SystemPrompt {
    pub fn new(base: impl Into<String>) -> Self {
        Self { base: base.into() }
    }

    pub fn add_tools(mut self, tools: &[Arc<dyn Tool>]) -> Self {
        let tool_descriptions: String = tools
            .iter()
            .map(|t| format!("{}: {}", t.name(), t.description()))
            .collect::<Vec<_>>()
            .join("\n");
        self.base.push_str("\n\nAvailable tools:\n");
        self.base.push_str(&tool_descriptions);
        self
    }
}

impl From<SystemPrompt> for Message {
    fn from(prompt: SystemPrompt) -> Self {
        Message {
            role: Role::System,
            content: prompt.base,
            tool_calls: None,
            tool_call_id: None,
        }
    }
}
