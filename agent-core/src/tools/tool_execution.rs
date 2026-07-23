use super::tool_decisions::ToolAuthorization;
use super::tool_registry::ToolRegistry;
use crate::events::SessionEvent;
use crate::session::history::InputContent;
use auger_driver::ToolCallId;
use futures::future::join_all;
use getset::CloneGetters;
use provider::ToolCallRequest;
use serde::Deserialize;
use serde::Serialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc;
use std::task::Context;
use std::task::Poll;
use tokio_util::sync::CancellationToken;

pub(crate) struct ToolExecution {
    calls: Vec<ToolCallRequest>,
    authorization: ToolAuthorization,
    registry: Arc<ToolRegistry>,
    /// Sender to emit per-call result/error events as each call finishes.
    event_tx: mpsc::Sender<SessionEvent>,
    cancellation: CancellationToken,
}

pub(crate) enum ToolExecutionCompleted {
    Completed(Vec<ToolCallResult>),
    Interrupted(Vec<ToolCallResult>),
}

pub(crate) struct ToolExecutionFuture {
    cancellation: CancellationToken,
    inner: Pin<Box<dyn Future<Output = ToolExecutionCompleted> + Send>>,
}

impl ToolExecution {
    pub(crate) fn new(
        calls: Vec<ToolCallRequest>,
        authorization: ToolAuthorization,
        registry: Arc<ToolRegistry>,
        event_tx: mpsc::Sender<SessionEvent>,
    ) -> Self {
        Self {
            calls,
            authorization,
            registry,
            event_tx,
            cancellation: CancellationToken::new(),
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

impl ToolExecution {
    async fn run_inner(self) -> ToolExecutionCompleted {
        let ToolExecution {
            calls,
            authorization,
            registry,
            event_tx,
            cancellation,
            ..
        } = self;
        let call_ids = calls.iter().map(|req| req.id.clone().into());

        let execution = async {
            join_all(calls.iter().map(|call| async {
                let outcome = match authorization.denial_reason(&call.id) {
                    Some(reason) => ToolOutcome::Denied {
                        reason: Some(reason),
                    },
                    // TODO: Why on earth does None mean not denied??
                    None => match registry.invoke(call.clone()).await {
                        Ok(result) => {
                            let result = result.to_string();
                            ToolOutcome::Success {
                                content: vec![ToolData::Text { text: result }],
                            }
                        }
                        Err(error) => {
                            let error = error.to_string();
                            ToolOutcome::Error {
                                error: vec![ToolData::Text { text: error }],
                            }
                        }
                    },
                };
                let result = ToolCallResult {
                    tool_call_id: call.id.clone().into(),
                    outcome,
                };
                let _ = event_tx.send(SessionEvent::ToolCallResult(result.clone()));
                result
            }))
            .await
        };

        tokio::select! {
            _ = cancellation.cancelled() => ToolExecutionCompleted::Interrupted(call_ids.into_iter().map(|id| ToolCallResult {
                            tool_call_id: id,
                            outcome: ToolOutcome::Interrupted
                        }).collect()),
            results = execution => ToolExecutionCompleted::Completed(results),
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

#[derive(Serialize, Deserialize, Debug, Clone, CloneGetters)]
pub struct ToolCallResult {
    #[getset(get_clone = "pub")]
    tool_call_id: ToolCallId,
    outcome: ToolOutcome,
}

impl From<ToolCallResult> for provider::ToolResult {
    fn from(result: ToolCallResult) -> Self {
        let content = match result.outcome {
            ToolOutcome::Success { content } => content,
            ToolOutcome::Error { error } => error,
            ToolOutcome::Denied { reason } => match reason {
                Some(r) => vec![ToolData::Text {
                    text: format!("Tool call was denied: {}", r),
                }],
                None => vec![ToolData::Text {
                    text: "Tool call was denied. Do not attempt to make the same tool call.".into(),
                }],
            },
            ToolOutcome::Interrupted => {
                vec![ToolData::Text {
                    text: "Tool call was interrupted. Do not attempt to make the same tool call."
                        .into(),
                }]
            }
        };
        provider::ToolResult {
            tool_call_id: result.tool_call_id.into(),
            // TODO: The provider should be updated to take a vec
            content: content
                .iter()
                .map(|d| match d {
                    ToolData::Text { text } => text.as_str(),
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

impl From<ToolCallResult> for InputContent {
    fn from(result: ToolCallResult) -> Self {
        let content = match result.outcome {
            ToolOutcome::Success { content } => content,
            ToolOutcome::Error { error } => error,
            ToolOutcome::Denied { reason } => match reason {
                Some(r) => vec![ToolData::Text {
                    text: format!("Tool call was denied: {}", r),
                }],
                None => vec![ToolData::Text {
                    text: "Tool call was denied. Do not attempt to make the same tool call.".into(),
                }],
            },
            ToolOutcome::Interrupted => {
                vec![ToolData::Text {
                    text: "Tool call was interrupted. Do not attempt to make the same tool call."
                        .into(),
                }]
            }
        };
        InputContent::ToolResult {
            tool_call_id: result.tool_call_id,
            content,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcome {
    Success { content: Vec<ToolData> },
    Error { error: Vec<ToolData> },
    Denied { reason: Option<String> },
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolData {
    Text { text: String },
}
