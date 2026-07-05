use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::runtime::Handle;
use provider::LlmModel;
use crate::tools::tool_registry::ToolRegistry;

struct Ctx {
    model: LlmModel,
    tools: ToolRegistry,
    /// Flag to interrupt the session loop
    cancel: Arc<AtomicBool>,
    /// Tokio runtime used for executing tools.
    rt: Handle,
}

struct Session<S> {
    ctx: Ctx,
    state: S,
}