use crate::agent::{Entry, HarnessEntry, Prompt, ReadyToStream};
use std::collections::HashSet;
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
        let (_, response) = self.previous_assistant_at().expect(
            "auger driver state invariant violation: an assistant message must precede the \
             WaitingForToolResponses state",
        );
        response
    }

    fn get_tool_calls(&self) -> Vec<ToolCallRequest> {
        let (index, response) = self.previous_assistant_at().expect(
            "auger driver state invariant violation: an assistant message must precede the \
             WaitingForToolResponses state",
        );

        let answered: HashSet<&str> = self.entries[index + 1..]
            .iter()
            .filter_map(|entry| match entry {
                Entry::Input(InputEntry::Harness(HarnessEntry::ToolResult(result))) => {
                    Some(result.tool_call_id.as_str())
                }
                _ => None,
            })
            .collect();

        response
            .tool_calls()
            .into_iter()
            .filter(|call| !answered.contains(call.id.as_str()))
            .collect()
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
        for result in responses.drain() {
            self.entries
                .push(InputEntry::from(HarnessEntry::ToolResult(result)).into());
        }
        TypedAgent {
            model: self.model,
            entries: self.entries,
            system_prompt: self.system_prompt,
            tools: self.tools,
            state: ReadyToStream {},
        }
    }
}
