use crate::agent::{ReadyToStream, State, TypedAgent};
use crate::tool_batch::{Resolved, Resolving, ToolBatch};
use provider::{ToolCallRequest};

/// The LLM has requested tool calls and the driver
/// is waiting for the tool call's results to be provided back.
pub struct WaitingForToolResponses;

impl State for WaitingForToolResponses {}

impl TypedAgent<WaitingForToolResponses> {

    /// Get all the tool names from the tool calls that were requested.
    pub fn tool_names_requested(&self) -> Vec<String> {
        let last_message = self.messages.last().expect("there should be at least one message in the thread");
        last_message
            .tool_calls()
            .iter()
            .map(|call| call.name.clone())
            .collect()
    }

    pub fn get_requested_tools(&self) -> Vec<ToolCallRequest> {
        self.state.thread.get_pending_tool_calls()
    }

    /// Get the batch of tool calls that were requested.
    pub fn get_batch(&self) -> ToolBatch<Resolving> {
        ToolBatch::new(self.state.thread.get_pending_tool_calls())
    }

    /// Submit a valid batch of tool responses.
    pub fn add_all_tool_responses(
        self,
        responses: ToolBatch<Resolved>,
    ) -> TypedAgent<ReadyToStream> {
        // prepare a provider::Message User variant with tool responess.
    }
}
