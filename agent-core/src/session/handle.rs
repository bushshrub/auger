use crate::session::events::{
    SessionEvent, ToolCallResponse, UserAction, UserCommand, UserMessage,
};
use crate::session::{SessionError, SessionId};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub model: String,
    pub created_at: u64,
    /// Sender for commands to the session thread.
    cmds: mpsc::Sender<UserCommand>,
    /// Sender for events from the session thread.
    events: broadcast::Sender<SessionEvent>,
}

impl SessionHandle {
    pub(crate) fn new(
        id: SessionId,
        model: String,
        cmd_tx: mpsc::Sender<UserCommand>,
        event_tx: broadcast::Sender<SessionEvent>,
    ) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id,
            model,
            created_at,
            cmds: cmd_tx,
            events: event_tx,
        }
    }
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Enqueue a message for the clanker.
    pub fn enqueue(&self, msg: UserMessage) -> Result<(), SessionError> {
        self.cmds
            .send(UserAction::SendMessage(msg.clone()).into())
            .map_err(|_| SessionError::Closed)?;
        Ok(())
    }

    pub fn respond_to_tool_call(
        &self,
        tool_call_id: String,
        approved: bool,
        message: Option<String>,
    ) -> Result<(), SessionError> {
        let response = if approved {
            ToolCallResponse::Approve
        } else {
            ToolCallResponse::Deny
        };
        self.cmds
            .send(
                UserAction::RespondToToolCall {
                    response,
                    tool_call_id,
                    message,
                }
                .into(),
            )
            .map_err(|_| SessionError::Closed)?;
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    pub fn snapshot(&self) -> Result<Vec<provider::Message>, SessionError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.cmds
            .send(UserCommand::Snapshot { reply: tx })
            .map_err(|_| SessionError::Closed)?;
        rx.recv().map_err(|_| SessionError::Closed)
    }
}
