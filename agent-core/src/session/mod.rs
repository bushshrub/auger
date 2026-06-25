use uuid::Uuid;

pub(crate) mod events;
pub(crate) mod handle;

pub(crate) mod session_loop;

mod session_history;

pub type SessionId = Uuid;

#[derive(Debug)]
pub enum SessionError {
    Closed
}
