//! State when LLM is streaming the response back.
//!

use crate::agent::{TypedAgent, WaitingForUserMessage};
use crate::interrupt_states::{LlmStreamingFailed, LlmStreamingInterrupted};
use crate::waiting_for_tools::WaitingForToolResponses;
use provider::thread::ClankerTurn;
use provider::{LlmModel, LlmThread, ToolDefinition};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_util::sync::CancellationToken;
use tracing::error;

/// Future which when awaited, streams the LLM response.
/// Once done, returns a StreamResult which gives the result state after streaming.
pub struct LlmStreaming {
    cancellation: CancellationToken,
    inner: Pin<Box<dyn Future<Output = StreamResult> + Send>>,
}

impl LlmStreaming {
    pub(crate) fn new(
        model: LlmModel,
        tools: Vec<ToolDefinition>,
        thread: LlmThread<ClankerTurn>,
        event_callback: Box<dyn Fn(provider::StreamEvent) + Send + Sync>,
        cancellation: CancellationToken,
    ) -> Self {
        let inner = Box::pin(run_stream(
            model,
            tools,
            thread,
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
    thread: LlmThread<ClankerTurn>,
    event_callback: impl Fn(provider::StreamEvent) + Send + Sync + 'static,
    cancellation: CancellationToken,
) -> StreamResult {
    let mut events = Vec::new();
    let request = thread.create_request(tools.clone());
    let mut stream = tokio::select! {
        result = model.stream(request) => match result {
            Ok(stream) => stream,
            Err(error) => {
                error!(model = %model.name(), error = %error, "failed to start provider stream");
                return StreamResult::Failed(TypedAgent {
                    model,
                    tools,
                    state: LlmStreamingFailed::new(thread, events, error),
                });
            }
        },
        _ = cancellation.cancelled() => {
            return StreamResult::Interrupted(TypedAgent {
                model,
                tools,
                state: LlmStreamingInterrupted::new(thread, events),
            });
        },
    };

    loop {
        let event = tokio::select! {
            event = futures::StreamExt::next(&mut stream) => event,
            _ = cancellation.cancelled() => {
                stream.abort();

                return StreamResult::Interrupted(TypedAgent {
                    model,
                    tools,
                state: LlmStreamingInterrupted::new(thread, events),
                });
            }
        };

        match event {
            Some(Ok(event)) => {
                event_callback(event.clone());
                events.push(event);
            }
            Some(Err(error)) => {
                error!(model = %model.name(), error = %error, "provider stream failed");
                return StreamResult::Failed(TypedAgent {
                    model,
                    tools,
                    state: LlmStreamingFailed::new(thread, events, error),
                });
            }
            None => break,
        }
    }

    let response = match provider::LlmResponse::from_events(events.clone()) {
        provider::LlmResponse::Completed(response) => response,
        provider::LlmResponse::Partial(_) => {
            return StreamResult::Failed(TypedAgent {
                model,
                tools,
                state: LlmStreamingFailed::new(
                    thread,
                    events,
                    provider::LlmError {
                        message: "provider stream ended without a done event".to_string(),
                    },
                ),
            });
        }
    };
    let clanker_message = provider::ClankerMessage::from(response);

    match thread.add_clanker_reply(clanker_message) {
        either::Either::Left(thread) => StreamResult::WaitingForUserMessage(TypedAgent {
            model,
            tools,
            state: WaitingForUserMessage { thread },
        }),
        either::Either::Right(thread) => StreamResult::WaitingForToolResponses(TypedAgent {
            model,
            tools,
            state: WaitingForToolResponses { thread },
        }),
    }
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
