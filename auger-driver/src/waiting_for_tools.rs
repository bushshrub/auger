use crate::agent::{Entry, HarnessEntry, Prompt, ReadyToStream};
use crate::agent::State;
use crate::agent::TypedAgent;
use crate::tool_batch::Resolved;
use crate::tool_batch::Resolving;
use crate::tool_batch::ToolBatch;
use provider::AssistantResponse;
use provider::Message;
use provider::ToolCallRequest;
use provider::UserPrompt;
use crate::InputEntry;

/// The LLM has requested tool calls and the driver
/// is waiting for the tool call's results to be provided back.
pub struct WaitingForToolResponses;

impl State for WaitingForToolResponses {}

impl TypedAgent<WaitingForToolResponses> {
    pub fn previous_message(&self) -> &AssistantResponse {
        let assistant_message = self.entries.last().expect("there to contain some message before this state");
        match assistant_message {
            Entry::Assistant(response) => response,
            _ => panic!(
                "auger driver state invariant violation: last message should be an assistant \
                 message when in WaitingForToolResponses state"
            ),
        }
    }

    fn get_tool_calls(&self) -> Vec<ToolCallRequest> {
        let last_message = self
            .entries()
            .last()
            .expect("there should be at least one message in the thread")
            .clone();
        match last_message {
            Entry::Assistant(response) => response.tool_calls(),
            _ => panic!(
                "auger driver state invariant violation: last message should be an assistant \
                 message when in WaitingForToolResponses state"
            ),
        }
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
        self.entries.push(Entry::Input(prompt.into()));
        TypedAgent {
            model: self.model,
            entries: self.entries,
            tools: self.tools,
            system_prompt: self.system_prompt,
            state: self.state,
        }
    }

    /// Submit a valid batch of tool responses.
    pub fn add_all_tool_responses(
        mut self,
        responses: ToolBatch<Resolved>,
    ) -> TypedAgent<ReadyToStream> {
        self.entries.push(InputEntry::from(HarnessEntry::ToolResults(responses.drain())).into());
        TypedAgent {
            model: self.model,
            entries: self.entries,
            system_prompt: self.system_prompt,
            tools: self.tools,
            state: ReadyToStream {},
        }
    }
}
