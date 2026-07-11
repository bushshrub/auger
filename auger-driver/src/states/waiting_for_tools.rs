use provider::LlmThread;
use provider::thread::ToolResultsPending;
use crate::driver::{Agent, ReadyToStream, State};
use crate::tool_batch::{Resolved, Resolving, ToolBatch};

/// The LLM has requested tool calls and the driver
/// is waiting for the tool call's results to be provided back.
pub struct WaitingForToolResponses {
    pub(crate) thread: LlmThread<ToolResultsPending>
}

impl State for WaitingForToolResponses {}

impl Agent<WaitingForToolResponses> {

    /// Get the batch of tool calls that were requested.
    pub fn get_batch(&self) -> ToolBatch<Resolving> {
        ToolBatch::new(self.state.thread.get_pending_tool_calls())
    }

    /// Submit a valid batch of tool responses.
    pub fn add_all_tool_responses(self, responses: ToolBatch<Resolved>) -> Agent<ReadyToStream> {
        let mut thread = self.state.thread;

        for response in responses.drain() {
            thread = match thread.add_tool_result(response) {
                Ok(either::Either::Left(thread)) => thread,
                Ok(either::Either::Right(thread)) => {
                    return Agent {
                        model: self.model,
                        tools: self.tools,
                        state: ReadyToStream::new(thread),
                    };
                }
                Err(error) => panic!("completed tool batch contained invalid result: {error}"),
            };
        }

        panic!("completed tool batch did not resolve all requested calls");
    }
}
