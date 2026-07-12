//! Driver API example.
//!
//! The host owns execution of the returned stream future. The driver owns the
//! conversation and returns the next valid state after each LLM turn.
//!
//! ```no_run
//! use auger_driver::{Agent, AgentStatus};
//! use provider::{LlmModel, ToolDefinition, UserPrompt};
//!
//! async fn run_agent(model: LlmModel) {
//!     let tools: Vec<ToolDefinition> = load_tool_definitions();
//!     let mut agent = Agent::new(
//!         model,
//!         "You are a helpful coding agent.".to_string(),
//!         tools,
//!     );
//!
//!     let completion = agent
//!         .send_message(UserPrompt::new(
//!             "Inspect the repository.".to_string(),
//!         ), |event| {
//!             println!("stream event: {event:?}");
//!         })
//!         .expect("agent should accept a user message")
//!         .await;
//!     agent.complete(completion);
//!
//!     if agent.status() == AgentStatus::WaitingForToolResponses {
//!         let pending_tools = agent.pending_tools().expect("tools should be pending");
//!         let tool_results = execute_tools(pending_tools).await;
//!         let completion = agent
//!             .submit_tool_results(tool_results, |_| {})
//!             .expect("agent should accept tool results")
//!             .await;
//!         agent.complete(completion);
//!         assert_eq!(agent.status(), AgentStatus::WaitingForUserMessage);
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
pub(crate) mod streaming;
pub(crate) mod tool_batch;
pub(crate) mod waiting_for_tools;

pub use agent::{ReadyToStream, State, TypedAgent, WaitingForUserMessage};
pub use interrupt_states::{LlmStreamingFailed, LlmStreamingInterrupted};
pub use streaming::{LlmStreaming, StreamResult};
pub use tool_batch::{AddToolResponseIssue, Resolved, Resolving, ToolBatch, ToolBatchState};
pub use waiting_for_tools::WaitingForToolResponses;
