//! Minimal runtime for driving an Auger agent session.
//!
//! The session owns a small state machine around user input, model streaming,
//! tool feedback, interrupts, and shutdown.
//!
//! ```no_run
//! use agent_loop::{Session, SessionCommand};
//! use provider::{LlmModel, UserPrompt};
//! use tokio::runtime::Handle;
//!
//! fn choose_model() -> LlmModel {
//!     todo!("construct or resolve a provider model")
//! }
//!
//! let handle = Session::start(
//!     "You are a helpful coding agent.".to_string(),
//!     Vec::new(),
//!     choose_model(),
//!     Handle::current(),
//! );
//!
//! let _ = handle.command_channel().send(SessionCommand::AddUserMessage(
//!     UserPrompt::new("Inspect the repository.".to_string()),
//! ));
//!
//! let events = handle.event_channel().recv().expect("receive events");
//! ```

mod events;
mod runtime;
pub(crate) mod session_state;
pub(crate) mod tool_call_batch;

pub use events::{LlmDelta, ModelTurnOutcome, SessionError, SessionEvent, SessionStatus, Usage};
pub use runtime::{Session, SessionCommand, SessionHandle, SessionSnapshot};
