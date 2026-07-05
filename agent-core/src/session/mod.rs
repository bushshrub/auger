use uuid::Uuid;

pub(crate) mod events;
pub(crate) mod handle;

pub(crate) mod session_loop;
mod sess_automaton;

pub type SessionId = Uuid;

#[derive(Debug)]
pub enum SessionError {
    Closed
}
