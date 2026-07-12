use super::tool_decisions::ToolAuthorization;
use super::tool_registry::ToolRegistry;
use auger_driver::{Resolved, Resolving, ToolBatch};
use futures::future::join_all;
use provider::{ToolCallRequest, ToolResult};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio_util::sync::CancellationToken;

pub(crate) trait ToolExecutionState {}

pub(crate) struct Ready;
pub(crate) struct Completed;
pub(crate) struct Interrupted;

impl ToolExecutionState for Ready {}
impl ToolExecutionState for Completed {}
impl ToolExecutionState for Interrupted {}

pub(crate) struct ToolExecution<S: ToolExecutionState> {
    batch: ToolBatch<Resolving>,
    authorization: ToolAuthorization,
    registry: Arc<ToolRegistry>,
    results: Vec<ToolResult>,
    cancellation: CancellationToken,
    _state: PhantomData<S>,
}

pub(crate) enum ToolExecutionCompleted {
    Completed(ToolExecution<Completed>),
    Interrupted(ToolExecution<Interrupted>),
}

pub(crate) struct ToolExecutionFuture {
    cancellation: CancellationToken,
    inner: Pin<Box<dyn Future<Output = ToolExecutionCompleted> + Send>>,
}

impl ToolExecution<Ready> {
    pub(crate) fn new(
        batch: ToolBatch<Resolving>,
        authorization: ToolAuthorization,
        registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            batch,
            authorization,
            registry,
            results: Vec::new(),
            cancellation: CancellationToken::new(),
            _state: PhantomData,
        }
    }

    pub(crate) fn run(self) -> ToolExecutionFuture {
        let cancellation = self.cancellation.clone();
        ToolExecutionFuture {
            cancellation,
            inner: Box::pin(self.run_inner()),
        }
    }
}

impl ToolExecution<Ready> {
    async fn run_inner(self) -> ToolExecutionCompleted {
        let ToolExecution {
            batch,
            authorization,
            registry,
            cancellation,
            ..
        } = self;
        let calls: Vec<ToolCallRequest> = batch.requested().cloned().collect();

        let execution = async {
            join_all(calls.iter().map(|call| async {
                let result = match authorization.denial_reason(&call.id) {
                    Some(reason) => ToolResult::new(call.id.clone(), reason),
                    None => registry
                        .invoke(call.clone())
                        .await
                        .map(|result| ToolResult::new(call.id.clone(), result.to_string()))
                        .unwrap_or_else(|error| {
                            ToolResult::new(call.id.clone(), error.to_string())
                        }),
                };
                result
            }))
            .await
        };

        tokio::select! {
            _ = cancellation.cancelled() => ToolExecutionCompleted::Interrupted(ToolExecution {
                batch,
                authorization,
                registry,
                results: Vec::new(),
                cancellation,
                _state: PhantomData,
            }),
            results = execution => ToolExecutionCompleted::Completed(ToolExecution {
                batch,
                authorization,
                registry,
                results,
                cancellation,
                _state: PhantomData,
            }),
        }
    }
}

impl ToolExecutionFuture {
    pub(crate) fn interrupt_handle(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

impl Future for ToolExecutionFuture {
    type Output = ToolExecutionCompleted;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

impl ToolExecution<Completed> {
    pub(crate) fn resolve(self) -> ToolBatch<Resolved> {
        self.batch
            .resolve_all(self.results)
            .expect("tool execution must produce one result per call")
    }
}

impl ToolExecution<Interrupted> {
    pub(crate) fn resolve(self) -> ToolBatch<Resolved> {
        self.batch.interrupt_remaining()
    }
}

impl ToolExecutionCompleted {
    pub(crate) fn resolve(self) -> ToolBatch<Resolved> {
        match self {
            Self::Completed(execution) => execution.resolve(),
            Self::Interrupted(execution) => execution.resolve(),
        }
    }
}
