use provider::ToolCallRequest;
use provider::ToolResult;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
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

/// All tools have a response.
#[derive(Debug)]
pub struct Resolved;

impl ToolBatchState for Resolved {}

/// Tool calls requested by the model and their results.
#[derive(Debug)]
pub struct ToolBatch<S: ToolBatchState> {
    pending_calls: HashMap<ToolCallId, ToolCallRequest>,
    results: HashMap<ToolCallId, ToolResult>,
    _state: PhantomData<S>,
}

impl ToolBatch<Resolved> {
    /// Consume the completed batch and return provider tool results.
    pub fn drain(self) -> Vec<ToolResult> {
        self.results.into_values().collect()
    }

    pub fn results(&self) -> Vec<&ToolResult> {
        self.results.values().collect()
    }
}
