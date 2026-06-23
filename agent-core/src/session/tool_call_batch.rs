use std::collections::HashMap;
use thiserror::Error;
use tracing::{debug, error};
use agent_tools::ToolCallResult;
use provider::ToolCall;
use crate::session::tool_registry::{ToolInvokeIssue, ToolRegistry};

pub(crate) type ToolCallId = String;
pub(crate) struct ToolCallBatch {
    pending_calls: HashMap<ToolCallId, ToolCall>,
    results: HashMap<ToolCallId, ToolCallResult>,
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

impl ToolCallBatch {

    pub(crate) fn new() -> Self {
        Self {
            pending_calls: HashMap::new(),
            results: HashMap::new(),
        }
    }
    pub(crate) fn new_batch(tool_calls_requested: Vec<ToolCall>) -> Self {
        let pending_calls = tool_calls_requested
            .into_iter()
            .map(|tool_call| (tool_call.id.clone(), tool_call))
            .collect();
        Self {
            pending_calls,
            results: HashMap::new(),
        }
    }

    pub(crate) fn extend(&mut self, new_tool_calls: Vec<ToolCall>) {
        for tool_call in new_tool_calls {
            self.pending_calls.insert(tool_call.id.clone(), tool_call);
        }
    }

    pub(crate) async fn approve_and_run_tool_call(&mut self, id: &ToolCallId, registry: &ToolRegistry) -> Result<ToolCallResult, RunToolCallError> {
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

    pub(crate) fn deny_call(&mut self, id: &ToolCallId) -> Result<(), RunToolCallError> {
        if self.pending_calls.remove(id).is_some() {
            debug!(tool_call_id = %id, "Tool call denied and removed from pending calls");
            self.results.insert(id.clone(), ToolCallResult::from("Tool call denied".to_string()));
            Ok(())
        } else {
            Err(RunToolCallError::NotFound(id.clone()))
        }
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.pending_calls.is_empty()
    }

    pub(crate) fn drain(&mut self) -> Vec<provider::ToolResult> {
        let results = self.results.drain().map(|(id, result)| provider::ToolResult { tool_call_id: id, content: result.to_string() }).collect();
        results
    }
}