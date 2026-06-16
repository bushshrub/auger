use std::sync::{mpsc, Arc};
use axum::response::sse::Event;
use tokio::sync::broadcast;
use uuid::Uuid;
use provider::{LlmProvider};
use crate::conversation::{Conversation, UserContent};
use crate::system_prompt::SystemPrompt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Running,
    AwaitingApproval,
}

pub struct Session {
    id: Uuid,
    conversation: Conversation,
    status: SessionStatus,
    /// The provider of the clanker
    provider: Arc<dyn LlmProvider>,
}

pub enum AgentEvent {
    UserMessage { content: Vec<UserContent> },
    Reasoning { delta: String },
    Content { delta: String }
}

pub struct SessionHandle {
    id: Uuid,
    cmds: mpsc::Sender<Cmd>,
    events: broadcast::Sender<AgentEvent>,
}

enum Cmd {
    SendMessage(Vec<UserContent>),
}

impl Session {

    /// Start a new session
    pub fn new(prompt: SystemPrompt, provider: &Arc<dyn LlmProvider>) -> Self {
        let id = Uuid::new_v4();
        let conversation = Conversation::new(prompt.into());
        let status = SessionStatus::Idle;
        Self {
            id, conversation, status, provider: Arc::clone(provider),
        }
    }

    pub fn user_send_message(&mut self, msg: Vec<UserContent>) -> Result<(), String> {
        todo!()
    }

    pub fn id(&self) -> Uuid {
        self.id.clone()
    }
}