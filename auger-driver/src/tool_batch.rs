use provider::ToolCallRequest;
use provider::ToolResult;
use serde::Deserialize;
use serde::Serialize;
use std::marker::PhantomData;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct ToolCallId(String);

impl From<String> for ToolCallId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<ToolCallId> for String {
    fn from(value: ToolCallId) -> Self {
        value.0
    }
}

/// The state of a tool batch.
pub trait ToolBatchState {}

/// Calls the model requested. No results yet.
#[derive(Debug)]
pub struct Unresolved;

/// Every call has exactly one result.
#[derive(Debug)]
pub struct Resolved;

impl ToolBatchState for Unresolved {}
impl ToolBatchState for Resolved {}

/// Tool calls requested by the model and their results.
#[derive(Debug)]
pub struct ToolBatch<S: ToolBatchState> {
    calls: Vec<ToolCallRequest>,
    results: Vec<ToolResult>,
    _state: PhantomData<S>,
}

impl ToolBatch<Unresolved> {
    pub(crate) fn new(calls: Vec<ToolCallRequest>) -> Self {
        Self {
            calls,
            results: Vec::new(),
            _state: PhantomData,
        }
    }

    pub fn calls(&self) -> &[ToolCallRequest] {
        &self.calls
    }

    /// Resolve the tool batch by providing a function that returns the result for each tool call.
    ///
    /// ```
    /// use auger_driver::{TypedAgent, WaitingForToolResponses};
    /// use std::collections::HashMap;
    ///
    /// fn submit(agent: TypedAgent<WaitingForToolResponses>, mut outputs: HashMap<String, String>) {
    ///     let batch = agent
    ///         .get_batch()
    ///         .resolve(|call| outputs.remove(&call.id).unwrap_or_default());
    ///     let _ready = agent.add_all_tool_responses(batch);
    /// }
    /// ```
    pub fn resolve(
        self,
        mut answer: impl FnMut(&ToolCallRequest) -> String,
    ) -> ToolBatch<Resolved> {
        let results = self
            .calls
            .iter()
            .map(|call| ToolResult {
                tool_call_id: call.id.clone(),
                content: answer(call),
            })
            .collect();
        ToolBatch {
            calls: self.calls,
            results,
            _state: PhantomData,
        }
    }
}

impl ToolBatch<Resolved> {
    /// Consume the completed batch and return provider tool results.
    pub(crate) fn into_results(self) -> Vec<ToolResult> {
        self.results
    }
}
