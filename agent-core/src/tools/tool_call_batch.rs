use std::collections::HashMap;
use std::marker::PhantomData;
use either::Either;
use thiserror::Error;
use tracing::{debug, error};
use agent_tools::ToolCallResult;
use provider::ToolCallRequest;
use crate::tools::tool_registry::{ToolInvokeIssue, ToolRegistry};

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
    pending_calls: HashMap<ToolCallId, ToolCallRequest>,
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
    pub(crate) fn new_batch(tool_calls_requested: Vec<ToolCallRequest>) -> Self {
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

    pub(crate) async fn approve_and_run(mut self, id: &ToolCallId, registry: &ToolRegistry) -> Result<Either<Self, ToolCallBatch<Complete>>, RunToolCallError> {
        let tool_call = self.pending_calls.remove(id).ok_or_else(|| RunToolCallError::NotFound(id.clone()))?;
        debug!(tool_call_id = %id, "Tool call approved and being executed");
        let result = match registry.invoke(tool_call).await {
            Ok(result) => {
                debug!(tool_call_id = %id, "Tool call executed successfully");
                result
            }
            Err(e) => {
                error!(tool_call_id = %id, "Error executing tool: {:?}", e);
                return Err(RunToolCallError::InvokeError(e));
            }
        };
        self.results.insert(id.clone(), result);
        Ok(Self::transition(self))
    }

    /// Deny a tool call
    pub(crate) fn deny(mut self, id: &ToolCallId) -> Result<Either<Self, ToolCallBatch<Complete>>, RunToolCallError> {
        self.pending_calls.remove(id).ok_or_else(|| RunToolCallError::NotFound(id.clone()))?;
        debug!(tool_call_id = %id, "Tool call denied and removed from pending calls");
        self.results.insert(id.clone(), ToolCallResult::denied_by_user("Tool call denied by user".to_string()));
        Ok(Self::transition(self))
    }

    fn transition(self) -> Either<Self, ToolCallBatch<Complete>> {
        if self.pending_calls.is_empty() {
            Either::Right(ToolCallBatch { pending_calls: self.pending_calls, results: self.results, _state: PhantomData })
        } else {
            Either::Left(self)
        }
    }
}

impl ToolCallBatch<Complete> {
    pub(crate) fn drain(self) -> Vec<provider::ToolResult> {
        self.results
            .into_iter()
            .map(|(id, result)| provider::ToolResult::new(id, result.to_string()))
            .collect()
    }
}
