use crate::conversation::{Conversation, UserContent};
use crate::system_prompt::SystemPrompt;
use provider::LlmProvider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

mod events;
use events::{AgentEvent, Cmd};


/// The status of a session
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Running,
    AwaitingApproval,
}

pub(crate) type SessionId = Uuid;

/// Represents a conversation between the user and a clanker.
pub(crate) struct Session {
    id: SessionId,
    conversation: Conversation,
    status: SessionStatus,
    provider: Arc<dyn LlmProvider>,
    events: broadcast::Sender<AgentEvent>,
}

impl Session {
    pub fn spawn(prompt: SystemPrompt, provider: &Arc<dyn LlmProvider>) -> SessionHandle {
        todo!()
    }

    /// Runs the session. The user will send commands via `rx`.
    async fn run(self, mut rx: mpsc::Receiver<Cmd>) {
        use futures::stream::StreamExt;

        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::SendMessage(content) => {
                    let _ = self.events.send(content.clone().into());

                    let user_text = content.iter()
                        .filter_map(|c| match c {
                            UserContent::Text(t) => Some(t.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    let request = provider::LlmRequest {
                        model: "qwen3.6-27b".to_string(),
                        messages: vec![provider::Message::User(user_text)],
                        tools: vec![],
                    };

                    if let Ok(mut stream) = self.provider.stream(request).await {
                        while let Some(event_result) = stream.next().await {
                            match event_result {
                                Ok(provider::StreamEvent::Text(text)) => {
                                    let _ = self.events.send(AgentEvent::Content { delta: text });
                                }
                                Ok(provider::StreamEvent::Reasoning(text)) => {
                                    let _ = self.events.send(AgentEvent::Reasoning { delta: text });
                                }
                                Ok(provider::StreamEvent::ToolCall { .. }) => {
                                    // TODO: handle tool calls
                                }
                                Ok(provider::StreamEvent::Done { .. }) => break,
                                // TODO: Handle errors while streaming e.g. rate limit, connection drops.
                                Err(_) => break,
                            }
                        }
                    }
                }
                Cmd::ApproveToolCall { tool_call_id } => {
                    // TODO: handle approval
                }
                Cmd::DenyToolCall { tool_call_id } => {
                    // TODO: handle denial
                }
            }
        }
    }
}


#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ReadToken (Uuid);

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct WriteToken (Uuid);

#[derive(Clone)]
pub(crate) struct SessionHandle {
    id: SessionId,
    read_token: ReadToken,
    write_token: WriteToken,
    cmds: tokio::sync::mpsc::Sender<Cmd>,      // <-- was std::sync::mpsc
    events: tokio::sync::broadcast::Sender<AgentEvent>,
}

impl SessionHandle {
    fn id(&self) -> SessionId {
        self.id
    }

    fn tokens(&self) -> (ReadToken, WriteToken) {
        (self.read_token.clone(), self.write_token.clone())
    }

    async fn enqueue(&self, t: &WriteToken, msg: Vec<UserContent>) -> Result<(), SessionError> {
        todo!()
    }

    async fn approve(&self, t: &WriteToken, tool_call_id: String, approved: bool) -> Result<(), SessionError> {
        todo!()
    }

    fn subscribe(&self, t: &ReadToken) -> Result<broadcast::Receiver<AgentEvent>, SessionError> {
        todo!()
    }

    // async fn snapshot(&self, t: &ReadToken) -> Result<ConversationSnapshot, SessionError> {
    //     todo!()
    // }
}

#[derive(Debug)]
pub enum SessionError {
    BadToken,
    Closed
}
