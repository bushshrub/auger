//! State when LLM is streaming the response back.

use crate::agent::TypedAgent;
use crate::agent::WaitingForUserMessage;
use crate::interrupt_states::LlmStreamingFailed;
use crate::interrupt_states::LlmStreamingInterrupted;
use crate::waiting_for_tools::WaitingForToolResponses;
use provider::{LlmModel, LlmResponse, StreamEvent};
use provider::LlmRequest;
use provider::ToolDefinition;
use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio_util::sync::CancellationToken;
use tracing::error;

/// Future which when awaited, streams the LLM response.
/// Once done, returns a StreamResult which gives the result state after
/// streaming.
pub struct LlmStreaming {
    cancellation: CancellationToken,
    inner: Pin<Box<dyn Future<Output = StreamResult> + Send>>,
}

impl LlmStreaming {
    pub(crate) fn new(
        model: LlmModel,
        tools: Vec<ToolDefinition>,
        messages_so_far: Vec<provider::Message>,
        event_callback: Box<dyn Fn(provider::StreamEvent) + Send + Sync>,
        cancellation: CancellationToken,
    ) -> Self {
        let inner = Box::pin(run_stream(
            model,
            tools,
            messages_so_far,
            event_callback,
            cancellation.clone(),
        ));

        Self {
            cancellation,
            inner,
        }
    }

    pub fn interrupt_handle(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

impl Future for LlmStreaming {
    type Output = StreamResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

pub(crate) async fn run_stream(
    model: LlmModel,
    tools: Vec<ToolDefinition>,
    mut messages_so_far: Vec<provider::Message>,
    event_callback: impl Fn(provider::StreamEvent) + Send + Sync + 'static,
    cancellation: CancellationToken,
) -> StreamResult {
    let mut events = Vec::new();
    let mut sink = |event: StreamEvent| {
        event_callback(event.clone());
        events.push(event);
    };
    let request = LlmRequest::new(messages_so_far.clone(), tools.clone());
    tokio::select! {
        result = model.stream(request, &mut sink) => match result {
            Ok(stream) => {
                let resp = LlmResponse::from_events(events);
                match resp {
                    LlmResponse::Partial(_) => {panic!("seems like a bug")}
                    LlmResponse::Completed(complete_response) => {
                        let clanker_msg = complete_response.response;
                        let has_tool_calls = !clanker_msg.tool_calls().is_empty();
                        messages_so_far.push(clanker_msg.into());
                        if !has_tool_calls {
                            return StreamResult::WaitingForUserMessage(TypedAgent {
                                model,
                                tools,
                                messages: messages_so_far,
                                state: WaitingForUserMessage {},
                            });
                        } else {
                            return StreamResult::WaitingForToolResponses(TypedAgent {
                                model,
                                tools,
                                messages: messages_so_far,
                                state: WaitingForToolResponses {},
                            });
                        }
                    }
                }
                stream
            },
            Err(error) => {
                error!(model = %model.name(), error = %error, "failed to start provider stream");
                return StreamResult::Failed(TypedAgent {
                    model,
                    tools,
                    messages: messages_so_far,
                    state: LlmStreamingFailed::new(events, error),
                });
            }
        },
        _ = cancellation.cancelled() => {
            return StreamResult::Interrupted(TypedAgent {
                model,
                tools,
                messages: messages_so_far,
                state: LlmStreamingInterrupted::new(events),
            });
        },
    };
}

/// The result of running the stream.
pub enum StreamResult {
    /// The user interrupted the stream
    Interrupted(TypedAgent<LlmStreamingInterrupted>),
    /// An error occurred while trying to start the stream, or in the middle
    /// of streaming
    Failed(TypedAgent<LlmStreamingFailed>),
    /// Stream completed successfully and the LLM has called tools
    WaitingForToolResponses(TypedAgent<WaitingForToolResponses>),
    /// Stream completed successfully and the LLM has not called any tools
    WaitingForUserMessage(TypedAgent<WaitingForUserMessage>),
}
