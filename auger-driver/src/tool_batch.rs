use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

use either::Either;
use provider::{ToolCallRequest, ToolResult};
use thiserror::Error;

pub type ToolCallId = String;

#[derive(Error, Debug)]
pub enum AddToolResponseIssue {
    #[error("The tool with id '{0}' was not requested by the LLM")]
    NotRequested(String),
    #[error("The tool with id '{0}' already has a result in this batch")]
    AlreadyProvided(String),
    #[error("Missing results for tool calls: {0:?}")]
    Incomplete(Vec<String>),
}

/// The state of a tool batch.
pub trait ToolBatchState {}

/// Some tools don't yet have a response
pub struct Resolving;
/// All tools have a response.
pub struct Resolved;

impl ToolBatchState for Resolving {}
impl ToolBatchState for Resolved {}

/// Tool calls requested by the model and their results.
pub struct ToolBatch<S: ToolBatchState> {
    pending_calls: HashMap<ToolCallId, ToolCallRequest>,
    results: HashMap<ToolCallId, ToolResult>,
    _state: PhantomData<S>,
}

impl ToolBatch<Resolving> {
    pub(crate) fn new(tool_calls: Vec<ToolCallRequest>) -> Self {
        let pending_calls = tool_calls
            .into_iter()
            .map(|call| (call.id.clone(), call))
            .collect();

        Self {
            pending_calls,
            results: HashMap::new(),
            _state: PhantomData,
        }
    }

    /// The calls still awaiting a result.
    pub fn requested(&self) -> impl Iterator<Item = &ToolCallRequest> {
        self.pending_calls.values()
    }

    pub fn add_result(&mut self, call_id: ToolCallId, result: ToolResult) -> Result<(), AddToolResponseIssue> {
        if self.results.contains_key(&call_id) {
            return Err(AddToolResponseIssue::AlreadyProvided(call_id));
        }

        self.pending_calls
            .remove(&call_id)
            .ok_or_else(|| AddToolResponseIssue::NotRequested(call_id.clone()))?;

        self.results.insert(call_id, result);
        Ok(())
    }

    /// Mark all unresolved calls as interrupted without executing them.
    pub fn interrupt_remaining(mut self) -> ToolBatch<Resolved> {
        for call_id in self.pending_calls.keys() {
            self.results.insert(
                call_id.clone(),
                ToolResult::new(
                    call_id.clone(),
                    "Tool call interrupted before execution".to_string(),
                ),
            );
        }

        ToolBatch {
            pending_calls: HashMap::new(),
            results: self.results,
            _state: PhantomData,
        }
    }

    /// Try to convert the resolving batch into a resolved batch.
    /// If there are still pending tool calls, this will just return itself.
    pub fn into_resolved(self) -> Either<Self, ToolBatch<Resolved>> {
        if self.pending_calls.is_empty() {
            Either::Right(ToolBatch {
                pending_calls: self.pending_calls,
                results: self.results,
                _state: PhantomData,
            })
        } else {
            Either::Left(self)
        }
    }
}

impl ToolBatch<Resolved> {
    /// Consume the completed batch and return provider tool results.
    pub fn drain(self) -> Vec<ToolResult> {
        self.results.into_values().collect()
    }
}
