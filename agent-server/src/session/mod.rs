use crate::conversation::{Conversation, UserContent};
use crate::system_prompt::SystemPrompt;
use provider::LlmProvider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, trace};
use uuid::Uuid;

mod events;
pub(crate) mod handle;

use events::{AgentEvent, Cmd};
use handle::SessionHandle;

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
    model: String,
    conversation: Conversation,
    status: SessionStatus,
    provider: Arc<dyn LlmProvider>,
    events: broadcast::Sender<AgentEvent>,
}

impl Session {
    pub fn spawn(prompt: SystemPrompt, provider: &Arc<impl LlmProvider + 'static>, model: String) -> SessionHandle {
        let (cmds_tx, mut cmds_rx) = mpsc::channel(32);
        let (events_tx, _) = broadcast::channel(32);

        let id = Uuid::new_v4();

        let session = Session {
            id,
            model,
            conversation: Conversation::new(prompt.into()),
            status: SessionStatus::Idle,
            provider: provider.clone(),
            events: events_tx.clone(),
        };

        tokio::spawn(session.run(cmds_rx));
        SessionHandle::new(id, cmds_tx, events_tx)
    }

    /// Runs the session. The user will send commands via `rx`.
    async fn run(self, mut rx: mpsc::Receiver<Cmd>) {
        use futures::stream::StreamExt;
        info!("Starting session: {}", self.id);
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::SendMessage(content) => {
                    debug!(session_id = %self.id, "Received user message: {:#?}", content);
                    let _ = self.events.send(content.clone().into());

                    let user_text = content.iter()
                        .filter_map(|c| match c {
                            UserContent::Text(t) => Some(t.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    let request = provider::LlmRequest {
                        model: self.model.clone(),
                        messages: vec![provider::Message::User(user_text)],
                        tools: vec![],
                    };
                    debug!("Sending request to provider: {:#?}", request);
                    if let Ok(mut stream) = self.provider.stream(request).await {
                        while let Some(event_result) = stream.next().await {
                            match event_result {
                                Ok(provider::StreamEvent::Text(text)) => {
                                    trace!("Received text from provider: {}", text);
                                    let _ = self.events.send(AgentEvent::Content { delta: text });
                                }
                                Ok(provider::StreamEvent::Reasoning(text)) => {
                                    trace!("Received reasoning from provider: {}", text);
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


#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
pub(crate) struct ReadToken (Uuid);

impl ReadToken {
    pub fn to_string(&self) -> String {
        self.0.to_string()
    }
}

impl From<ReadToken> for String {
    fn from(token: ReadToken) -> Self {
        token.0.to_string()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
pub(crate) struct WriteToken (Uuid);

impl WriteToken {
    pub fn to_string(&self) -> String {
        self.0.to_string()
    }
}

impl From<WriteToken> for String {
    fn from(token: WriteToken) -> Self {
        token.0.to_string()
    }
}

#[derive(Debug)]
pub enum SessionError {
    BadToken,
    Closed
}
