//! Driver API example.
//!
//! The host owns execution of the returned stream future. The driver owns the
//! conversation and returns the next valid state after each LLM turn.
//!
//! ```no_run
//! use auger_driver::{StreamResult, TypedAgent, WaitingForUserMessage};
//! use provider::{LlmModel, ToolDefinition, UserPrompt};
//!
//! async fn run_agent(model: LlmModel) {
//!     let tools: Vec<ToolDefinition> = load_tool_definitions();
//!     let agent = TypedAgent::<WaitingForUserMessage>::new(
//!         model,
//!         "You are a helpful coding agent.".to_string(),
//!         tools,
//!     )
//!     .add_message(UserPrompt::new("Inspect the repository.".to_string()));
//!
//!     match agent.create_stream().await {
//!         StreamResult::WaitingForUserMessage(_) => {}
//!         StreamResult::WaitingForToolResponses(agent) => {
//!             let pending_tools = agent.get_batch();
//!             let _ = (pending_tools, execute_tools);
//!         }
//!         StreamResult::Interrupted(_) | StreamResult::Failed(_) => {}
//!     }
//! }
//!
//! fn load_tool_definitions() -> Vec<ToolDefinition> {
//!     todo!("construct the available tool definitions")
//! }
//!
//! async fn execute_tools(
//!     pending_tools: auger_driver::ToolBatch<auger_driver::Resolving>,
//! ) -> auger_driver::ToolBatch<auger_driver::Resolved> {
//!     todo!("apply policy, execute tools, and return a resolved batch")
//! }
//! ```

pub(crate) mod agent;
pub(crate) mod interrupt_states;
pub(crate) mod restore;
pub(crate) mod streaming;
pub(crate) mod tool_batch;
pub(crate) mod waiting_for_tools;

pub use agent::ReadyToStream;
pub use agent::State;
pub use agent::TypedAgent;
pub use agent::WaitingForUserMessage;
pub use interrupt_states::LlmStreamingFailed;
pub use interrupt_states::LlmStreamingInterrupted;
pub use restore::RestoreState;
pub use restore::RestoredAgent;
pub use restore::restore;
pub use streaming::LlmStreaming;
pub use streaming::StreamResult;
pub use tool_batch::AddToolResponseIssue;
pub use tool_batch::Resolved;
pub use tool_batch::Resolving;
pub use tool_batch::ToolBatch;
pub use tool_batch::ToolBatchState;
pub use tool_batch::ToolCallId;
pub use waiting_for_tools::WaitingForToolResponses;
