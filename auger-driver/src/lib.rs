//! Driver API example.
//!
//! The host owns execution of the returned stream future. The driver owns the
//! conversation and returns the next valid state after each LLM turn.
//!
//! ```no_run
//! use auger_driver::{Prompt, StreamResult, TypedAgent, WaitingForUserMessage};
//! use provider::{LlmModel, ToolDefinition, UserPrompt};
//!
//! async fn run_agent(model: LlmModel) {
//!     let tools: Vec<ToolDefinition> = load_tool_definitions();
//!     let agent = TypedAgent::<WaitingForUserMessage>::new(
//!         model,
//!         "You are a helpful coding agent.".to_string(),
//!         tools,
//!     )
//!     .add_message(Prompt::User(UserPrompt::new(
//!         "Inspect the repository.".to_string(),
//!     )));
//!
//!     match agent.create_stream(|_event| {}).await {
//!         StreamResult::WaitingForUserMessage { .. } => {}
//!         StreamResult::WaitingForToolResponses { agent, .. } => {
//!             let _ = agent.get_requested_tools();
//!         }
//!         StreamResult::Interrupted(_) | StreamResult::Failed { .. } => {}
//!     }
//! }
//!
//! fn load_tool_definitions() -> Vec<ToolDefinition> {
//!     todo!("construct the available tool definitions")
//! }
//! ```

pub(crate) mod agent;
pub(crate) mod interrupt_states;
pub(crate) mod restore;
pub(crate) mod streaming;
pub(crate) mod tool_batch;
pub(crate) mod waiting_for_tools;

pub use agent::Turn;
pub use agent::InputEntry;
pub use agent::HarnessEntry;
pub use agent::Prompt;
pub use agent::ReadyToStream;
pub use agent::State;
pub use agent::TypedAgent;
pub use agent::WaitingForUserMessage;
pub use interrupt_states::LlmStreamingInterrupted;
pub use restore::RestoreError;
pub use restore::RestoreState;
pub use restore::RestoredAgent;
pub use restore::restore;
pub use streaming::LlmStreaming;
pub use streaming::StreamResult;
pub use tool_batch::Resolved;
pub use tool_batch::ToolBatch;
pub use tool_batch::ToolBatchState;
pub use tool_batch::ToolCallId;
pub use tool_batch::Unresolved;
pub use waiting_for_tools::WaitingForToolResponses;
