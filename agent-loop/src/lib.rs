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
//! let _ = handle.commands().send(SessionCommand::SubmitInput(
//!     UserPrompt::new("Inspect the repository.".to_string()),
//! ));
//! ```

mod runtime;
pub(crate) mod session_state;

pub use runtime::{Session, SessionCommand, SessionEvent, SessionHandle};
