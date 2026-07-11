//! Driver API example.
//!
//! The host owns execution of the returned stream future. The driver owns the
//! conversation and returns the next valid state after each LLM turn.
//!
//! ```no_run
//! use auger_driver::{Agent, StreamResult};
//! use provider::{LlmModel, ToolDefinition, UserPrompt};
//!
//! async fn run_agent(model: LlmModel) {
//!     let tools: Vec<ToolDefinition> = load_tool_definitions();
//!     let agent = Agent::new(
//!         model,
//!         "You are a helpful coding agent.".to_string(),
//!         tools,
//!     );
//!
//!     let mut result = agent
//!         .add_message(UserPrompt::new(
//!             "Inspect the repository.".to_string(),
//!         ))
//!         .add_event_callback(|event| {
//!             println!("stream event: {event:?}");
//!         })
//!         .create_stream()
//!         .await;
//!
//!     loop {
//!         result = match result {
//!             StreamResult::WaitingForUserMessage(driver) => {
//!                 let message = UserPrompt::new("Continue.".to_string());
//!
//!                 driver
//!                     .add_message(message)
//!                     .create_stream()
//!                     .await
//!             }
//!
//!             StreamResult::WaitingForToolResponses(driver) => {
//!                 let pending_tools = driver.get_batch();
//!                 let tool_results = execute_tools(pending_tools).await;
//!
//!                 driver
//!                     .add_all_tool_responses(tool_results)
//!                     .create_stream()
//!                     .await
//!             }
//!
//!             StreamResult::Interrupted(driver) => {
//!                 for event in driver.events() {
//!                     println!("interrupted event: {event:?}");
//!                 }
//!
//!                 driver
//!                     .add_message_to_continue(
//!                         UserPrompt::new("Please continue.".to_string()),
//!                         true,
//!                     )
//!                     .create_stream()
//!                     .await
//!             }
//!
//!             StreamResult::Failed(driver) => {
//!                 for event in driver.events() {
//!                     println!("failed event: {event:?}");
//!                 }
//!
//!                 driver.retry().create_stream().await
//!             }
//!         };
//!     }
//! }
//!
//! fn load_tool_definitions() -> Vec<ToolDefinition> {
//!     todo!("construct the available tool definitions")
//! }
//!
//! async fn execute_tools(
//!     pending_tools: impl Sized,
//! ) -> impl Sized {
//!     todo!("apply policy, execute tools, and return a resolved batch")
//! }
//! ```

mod agent;
pub mod states;
pub mod tool_batch;
