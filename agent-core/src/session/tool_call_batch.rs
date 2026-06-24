use std::collections::HashMap;
use std::marker::PhantomData;
use thiserror::Error;
use tracing::{debug, error};
use agent_tools::ToolCallResult;
use provider::ToolCall;
use crate::session::tool_registry::{ToolInvokeIssue, ToolRegistry};

pub(crate) type ToolCallId = String;

mod private {
    pub trait Sealed {}
}

pub(crate) trait BatchState: private::Sealed {}

pub(crate) struct Resolving;
pub(crate) struct Complete;

impl private::Sealed for Resolving {}
impl private::Sealed for Complete {}
impl BatchState for Resolving {}
impl BatchState for Complete {}

pub(crate) struct ToolCallBatch<S: BatchState> {
    pending_calls: HashMap<ToolCallId, ToolCall>,
    results: HashMap<ToolCallId, ToolCallResult>,
    _state: PhantomData<S>,
}

#[derive(Error, Debug)]
pub(crate) enum RunToolCallError {
    #[error("Tool call with id '{0}' not found in batch")]
    NotFound(String),
    #[error("Tool call with id '{0}' has already been processed")]
    AlreadyProcessed(String),
    #[error("Error invoking tool: {0}")]
    InvokeError(#[from] ToolInvokeIssue)
}

impl ToolCallBatch<Resolving> {
    pub(crate) fn new_batch(tool_calls_requested: Vec<ToolCall>) -> Self {
        let pending_calls = tool_calls_requested
            .into_iter()
            .map(|tool_call| (tool_call.id.clone(), tool_call))
            .collect();
        Self {
            pending_calls,
            results: HashMap::new(),
            _state: PhantomData,
        }
    }

    pub(crate) fn new_batch_with_results(
        tool_calls: Vec<ToolCall>,
        pre_resolved: Vec<provider::ToolResult>,
    ) -> Self {
        let pending_calls = tool_calls
            .into_iter()
            .map(|tc| (tc.id.clone(), tc))
            .collect();
        let results = pre_resolved
            .into_iter()
            .map(|r| (r.tool_call_id, ToolCallResult::from(r.content)))
            .collect();
        Self {
            pending_calls,
            results,
            _state: PhantomData,
        }
    }

    pub(crate) async fn approve_and_run(&mut self, id: &ToolCallId, registry: &ToolRegistry) -> Result<ToolCallResult, RunToolCallError> {
        let tool_call = self.pending_calls.remove(id).ok_or_else(|| RunToolCallError::NotFound(id.clone()))?;
        debug!(tool_call_id = %id, "Tool call approved and being executed");
        match registry.invoke(tool_call).await {
            Ok(result) => {
                debug!(tool_call_id = %id, "Tool call executed successfully");
                self.results.insert(id.clone(), result.clone());
                Ok(result)
            }
            Err(e) => {
                error!(tool_call_id = %id, "Error executing tool: {:?}", e);
                self.results.insert(id.clone(), ToolCallResult::error(e.to_string()));
                Err(RunToolCallError::InvokeError(e))
            }
        }
    }

    pub(crate) fn deny(&mut self, id: &ToolCallId) -> Result<(), RunToolCallError> {
        if self.pending_calls.remove(id).is_some() {
            debug!(tool_call_id = %id, "Tool call denied and removed from pending calls");
            self.results.insert(id.clone(), ToolCallResult::from("Tool call denied".to_string()));
            Ok(())
        } else {
            Err(RunToolCallError::NotFound(id.clone()))
        }
    }

    pub(crate) fn try_complete(self) -> Result<ToolCallBatch<Complete>, Self> {
        if self.pending_calls.is_empty() {
            Ok(ToolCallBatch {
                pending_calls: self.pending_calls,
                results: self.results,
                _state: PhantomData,
            })
        } else {
            Err(self)
        }
    }
}

impl ToolCallBatch<Complete> {
    pub(crate) fn drain(self) -> Vec<provider::ToolResult> {
        self.results
            .into_iter()
            .map(|(id, result)| provider::ToolResult { tool_call_id: id, content: result.to_string() })
            .collect()
    }
}
