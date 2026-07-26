//! State when LLM is streaming the response back.

use crate::agent::{convert_entries_into_messages, Entry, TypedAgent};
use crate::agent::WaitingForUserMessage;
use crate::interrupt_states::LlmStreamingInterrupted;
use crate::waiting_for_tools::WaitingForToolResponses;
use provider::{fold_events, AssistantResponse, CompletedLlmResponse, LlmError, LlmModel, StreamEvent};
use provider::LlmRequest;
use provider::ToolDefinition;
use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio_util::sync::CancellationToken;
use tracing::error;
use crate::ReadyToStream;

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
        system_prompt: String,
        tools: Vec<ToolDefinition>,
        entries_so_far: Vec<Entry>,
        event_callback: Box<dyn Fn(provider::StreamEvent) + Send + Sync>,
        cancellation: CancellationToken,
    ) -> Self {
        let inner = Box::pin(run_stream(
            model,
            tools,
            system_prompt,
            entries_so_far,
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
    system_prompt: String,
    mut entries_so_far: Vec<Entry>,
    event_callback: impl Fn(provider::StreamEvent) + Send + Sync + 'static,
    cancellation: CancellationToken,
) -> StreamResult {
    let mut events = Vec::new();
    let mut sink = |event: StreamEvent| {
        event_callback(event.clone());
        events.push(event);
    };
    let messages_so_far = convert_entries_into_messages(entries_so_far.clone());
    let request = LlmRequest::new(messages_so_far.clone(), tools.clone());
    tokio::select! {
        result = model.stream(request, &mut sink) => match result {
            Ok(stream) => {
                let (resp, token_usage) = fold_events(&events);
                let clanker_msg = AssistantResponse::new(resp).expect("there to be blocks");
                let has_tool_calls = !clanker_msg.tool_calls().is_empty();
                entries_so_far.push(Entry::Assistant(clanker_msg));
                if !has_tool_calls {
                    StreamResult::WaitingForUserMessage(TypedAgent {
                        model,
                        tools,
                        system_prompt,
                        entries: entries_so_far,
                        state: WaitingForUserMessage {},
                    })
                } else {
                    StreamResult::WaitingForToolResponses(TypedAgent {
                        model,
                        tools,
                        system_prompt,
                        entries: entries_so_far,
                        state: WaitingForToolResponses {},
                    })
                }
            },
            Err(error) => {
                let (resp, token_usage) = fold_events(&events);
                error!(model = %model.name(), error = %error, "failed to start provider stream");
                StreamResult::Failed {
                    agent: TypedAgent {
                        model,
                        tools,
                        system_prompt,
                        entries: entries_so_far,
                        state: LlmStreamingInterrupted {
                            partial: AssistantResponse::from_interrupted(resp),
                        },
                    },
                    error
                }
            }
        },
        _ = cancellation.cancelled() => {
            let (resp, token_usage) = fold_events(&events);
            StreamResult::Interrupted(TypedAgent {
                model,
                tools,
                system_prompt,
                entries: entries_so_far,
                state: LlmStreamingInterrupted {
                    partial: AssistantResponse::from_interrupted(resp),
                }
            })
        },
    }
}

/// The result of running the stream.
pub enum StreamResult {
    /// The user interrupted the stream
    Interrupted(TypedAgent<LlmStreamingInterrupted>),
    /// An error occurred while trying to start the stream, or in the middle
    /// of streaming
    Failed {
        agent: TypedAgent<LlmStreamingInterrupted>,
        error: LlmError,
    },
    /// Stream completed successfully and the LLM has called tools
    WaitingForToolResponses(TypedAgent<WaitingForToolResponses>),
    /// Stream completed successfully and the LLM has not called any tools
    WaitingForUserMessage(TypedAgent<WaitingForUserMessage>),
}
