use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use uuid::Uuid;
use crate::session::{ReadToken, SessionError, SessionId, WriteToken};
use crate::session::events::{SessionEvent, UserCmd, UserMessage};

#[derive(Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub model: String,
    pub created_at: u64,
    pub read_token: ReadToken,
    pub write_token: WriteToken,
    /// Sender for commands to the session thread.
    cmds: mpsc::Sender<UserCmd>,
    /// Sender for events from the session thread.
    events: broadcast::Sender<SessionEvent>,
}

impl SessionHandle {

    pub(crate) fn new(id: SessionId, model: String, cmd_tx: mpsc::Sender<UserCmd>, event_tx: broadcast::Sender<SessionEvent>) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id,
            model,
            created_at,
            read_token: ReadToken(Uuid::new_v4()),
            write_token: WriteToken(Uuid::new_v4()),
            cmds: cmd_tx,
            events: event_tx,
        }
    }
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Enqueue a message for the clanker.
    pub fn enqueue(&self, msg: UserMessage) -> Result<(), SessionError> {
        self.cmds.send(UserCmd::SendMessage(msg.clone())).map_err(|_| SessionError::Closed)?;
        self.events.send(SessionEvent::UserMessage { content: msg }).ok();
        Ok(())
    }

    pub fn respond_to_tool_call(&self, tool_call_id: String, approved: bool) -> Result<(), SessionError> {
        let event = match approved {
            true => UserCmd::ApproveToolCall { tool_call_id: tool_call_id.clone() },
            false => UserCmd::DenyToolCall { tool_call_id: tool_call_id.clone() }
        };
        self.cmds.send(event).map_err(|_| SessionError::Closed)?;
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    pub fn snapshot(&self) -> Result<Vec<provider::Message>, SessionError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.cmds.send(UserCmd::Snapshot { reply: tx }).map_err(|_| SessionError::Closed)?;
        rx.recv().map_err(|_| SessionError::Closed)
    }
}