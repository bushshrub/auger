//! Basic design:
//! model responds back with some list of tools it wants to call
//! for each tool it wants to call:
//!     check if tool is auto approved
//!     if it is:
//!         spawn tool execution job.
//!         job should send message to main session thread via mpsc::Sender<SessionEvent>
//!         once it completes. (each tool has some timeout to prevent runaway)
//!     if not, leave it as undecided.
//!     if auto denied: immediately mark
//!
//! whenever user sends approval info -> if approved -> run it
//! if denied -> set DoneWithheld
//!
//! when tool finishes executing or it times out it will send message to session thread
//! session thread is responsible for handing off the information back into the ToolPhaseEngine

pub mod auto_approval;
pub(crate) mod registry;
pub(crate) mod result;

use crate::harness::tool_engine::ToolCallResultState::{DoneWithheld, Undecided};
use crate::harness::tool_engine::registry::ToolRegistry;
use provider::ToolCallRequest;
use std::collections::{HashMap, HashSet};
use tokio::runtime::Runtime;
use tracing::warn;

pub(crate) struct ToolPhaseEngine {
    calls: HashMap<String, ToolCall>,
    runtime: Runtime,
    registry: ToolRegistry,
}

impl ToolPhaseEngine {
    pub fn new(calls: Vec<ToolCallRequest>, runtime: Runtime, tools: ToolRegistry) -> Self {
        Self {
            calls: calls.into_iter().map(|call| (call.id.clone(), ToolCall::new(call))).collect(),
            runtime,
            registry: tools
        }
    }

    pub fn deny(&mut self, id: String) -> Option<()> {
        if let Some(c) = self.calls.get_mut(&id) {
            c.result = DoneWithheld("The tool call was denied".to_string());
            Some(())
        } else {
            warn!("Tool call id provided was not found: {}", id);
            None
        }
    }

    pub fn run(&mut self, ids: Vec<String>) {
        let calls = ids.iter().filter_map(|id| self.calls.get(id))
            .collect::<Vec<_>>();

    }
}

enum ToolCallResultState {
    Undecided,
    Running,
    DoneContinue(String),
    /// If any tool has DoneWithheld state, the results of this entire batch
    /// of tools will not be sent back to the model unless the user sends
    /// another message/clicks some button or something.
    /// If a tool call is denied by the user it should be sent into this state.
    DoneWithheld(String)
}

struct ToolCall {
    request: ToolCallRequest,
    result: ToolCallResultState,
}

impl ToolCall {
    fn new(req: ToolCallRequest) -> Self {
        Self {
            request: req,
            result: Undecided,
        }
    }

}