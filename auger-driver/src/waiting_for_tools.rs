use crate::agent::{pending_tool_calls, HarnessEntry, Prompt, ReadyToStream};
use crate::agent::State;
use crate::agent::TypedAgent;
use crate::tool_batch::Resolved;
use crate::tool_batch::Resolving;
use crate::tool_batch::ToolBatch;
use provider::AssistantResponse;
use provider::ToolCallRequest;

/// The LLM has requested tool calls and the driver
/// is waiting for the tool call's results to be provided back.
pub struct WaitingForToolResponses;

impl State for WaitingForToolResponses {}

impl TypedAgent<WaitingForToolResponses> {
    pub fn previous_message(&self) -> &AssistantResponse {
        self.last_output().expect(
            "auger driver state invariant violation: an output turn must precede the \
             WaitingForToolResponses state",
        )
    }

    fn get_tool_calls(&self) -> Vec<ToolCallRequest> {
        pending_tool_calls(&self.turns)
    }

    /// Get all the tool names from the tool calls that were requested.
    pub fn tool_names_requested(&self) -> Vec<String> {
        self.get_tool_calls()
            .into_iter()
            .map(|call| call.name)
            .collect()
    }

    pub fn get_requested_tools(&self) -> Vec<ToolCallRequest> {
        self.get_tool_calls()
    }

    /// Get the batch of tool calls that were requested.
    pub fn get_batch(&self) -> ToolBatch<Resolving> {
        ToolBatch::new(self.get_tool_calls())
    }

    /// Inject a prompt. This is useful for things like steering.
    pub fn inject(mut self, prompt: Prompt) -> Self {
        self.push_input(prompt.into());
        self
    }

    /// Submit a valid batch of tool responses.
    pub fn add_all_tool_responses(
        mut self,
        responses: ToolBatch<Resolved>,
    ) -> TypedAgent<ReadyToStream> {
        for result in responses.drain() {
            self.push_input(HarnessEntry::ToolResult(result).into());
        }
        TypedAgent {
            model: self.model,
            turns: self.turns,
            system_prompt: self.system_prompt,
            tools: self.tools,
            state: ReadyToStream {},
        }
    }
}
