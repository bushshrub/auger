use std::fmt::Display;
use agent_tools::{Dummy, Tool};
use crate::conversation::{Conversation, UserContent};
use crate::system_prompt::SystemPrompt;
use provider::LlmProvider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, trace};
use uuid::Uuid;

mod events;
pub(crate) mod handle;

use events::{SessionEvent, Cmd};
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
    events: broadcast::Sender<SessionEvent>,
}

impl Session {
    pub fn spawn(prompt: SystemPrompt, provider: &Arc<impl LlmProvider + 'static>, model: String) -> SessionHandle {
        let (cmds_tx, mut cmds_rx) = mpsc::channel(32);
        let (events_tx, _) = broadcast::channel(32);

        let id = Uuid::new_v4();

        let session = Session {
            id,
            model: model.clone(),
            conversation: Conversation::new(prompt.into()),
            status: SessionStatus::Idle,
            provider: provider.clone(),
            events: events_tx.clone(),
        };

        tokio::spawn(session.run(cmds_rx));
        SessionHandle::new(id, model, cmds_tx, events_tx)
    }

    /// Runs the session. The user will send commands via `rx`.
    async fn run(self, mut rx: mpsc::Receiver<Cmd>) {
        use futures::stream::StreamExt;
        info!(session_id = %self.id, "Starting session");
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::SendMessage(content) => {
                    let _ = self.events.send(content.clone().into());

                    // TODO: these can just be sent off as a vec anyway...
                    let user_text = content.iter()
                        .filter_map(|c| match c {
                            UserContent::Text(t) => Some(t.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    let dummy = Dummy;
                    let dummy_details = dummy.details();
                    let dummy_params = dummy.parameters();
                    let request = provider::LlmRequest {
                        model: self.model.clone(),
                        messages: vec![provider::Message::User(user_text)],
                        tools: vec![provider::ToolDefinition {
                            name: dummy_details.name.to_string(),
                            description: Some(dummy_details.description.to_string()),
                            parameters: dummy_params.0,
                        }],
                    };
                    if let Ok(mut stream) = self.provider.stream(request).await {

                        while let Some(event_result) = stream.next().await {
                            match event_result {
                                Ok(provider::StreamEvent::TextDelta(text)) => {
                                    trace!("text delta: {}", text);
                                    let _ = self.events.send(SessionEvent::Content { delta: text });
                                }
                                Ok(provider::StreamEvent::ReasoningDelta(text)) => {
                                    trace!("reasoning delta: {}", text);
                                    let _ = self.events.send(SessionEvent::Reasoning { delta: text });
                                }
                                Ok(provider::StreamEvent::ToolCall { id, name, arguments }) => {
                                    trace!(tool_call_id = %id, tool = %name, "tool call delta: {}", arguments)
                                    // TODO: handle tool call deltas
                                }
                                // clanker has finished generating tool call request
                                Ok(provider::StreamEvent::ToolCallComplete {id, name, arguments}) => {
                                    debug!(tool_call_id = %id, tool = %name, "tool call complete: {}", arguments);
                                    let _ = self.events.send(SessionEvent::ToolCall { id, name, arguments });
                                }
                                Ok(provider::StreamEvent::Done { .. }) => {
                                    debug!("Response has finished generating");
                                    let _ = self.events.send(SessionEvent::Done);
                                },
                                // TODO: Handle errors while streaming e.g. rate limit, connection drops.
                                Err(e) => {
                                    error!("Error while streaming response from provider: {:?}", e);
                                },
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
        info!(session_id = %self.id, "Session has been closed");
    }
}


#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
pub(crate) struct ReadToken (Uuid);

impl Display for ReadToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<ReadToken> for String {
    fn from(token: ReadToken) -> Self {
        token.0.to_string()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
pub(crate) struct WriteToken (Uuid);

impl Display for WriteToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
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
