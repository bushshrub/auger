use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;
use crate::conversation::UserContent;
use crate::session::{ReadToken, SessionError, SessionId, WriteToken};
use crate::session::events::{SessionEvent, Cmd};

#[derive(Clone)]
pub(crate) struct SessionHandle {
    pub(crate) id: SessionId,
    pub(crate) read_token: ReadToken,
    pub(crate) write_token: WriteToken,
    /// Sender for commands to the session task.
    pub(crate) cmds: mpsc::Sender<Cmd>,
    /// Sender for events from the session task.
    pub(crate) events: broadcast::Sender<SessionEvent>,
}

impl SessionHandle {

    pub fn new(id: SessionId, cmd_tx: mpsc::Sender<Cmd>, event_tx: broadcast::Sender<SessionEvent>) -> Self {
        Self {
            id,
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
    pub(crate) async fn enqueue(&self,msg: Vec<UserContent>) -> Result<(), SessionError> {
        self.cmds.send(Cmd::SendMessage(msg.clone())).await.map_err(|_| SessionError::Closed)?;
        self.events.send(SessionEvent::UserMessage { content: msg }).ok();
        Ok(())
    }

    pub(crate) async fn respond_to_tool_call(&self, tool_call_id: String, approved: bool) -> Result<(), SessionError> {
        let event = match approved {
            true => Cmd::ApproveToolCall { tool_call_id: tool_call_id.clone() },
            false => Cmd::DenyToolCall { tool_call_id: tool_call_id.clone() }
        };
        self.cmds.send(event).await.map_err(|_| SessionError::Closed)?;
        Ok(())
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    // async fn snapshot(&self, t: &ReadToken) -> Result<ConversationSnapshot, SessionError> {
    //     todo!()
    // }
}