use crate::events::{LlmDelta, SessionError, SessionEvent};
use futures::StreamExt;
use provider::{LlmModel, LlmRequest, StreamEvent};
use std::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(crate) struct ModelStreamOutcome {
    pub(crate) terminal: ModelStreamTerminal,
    pub(crate) partial_response: Vec<StreamEvent>,
}

/// The final state of a model stream, which can be either complete, failed, or cancelled.
pub(crate) enum ModelStreamTerminal {
    Complete,
    Failed(SessionError),
    Cancelled,
}

/// Stream the output of a model and push into events, with
/// cancellation mediated by the provider stream's abort operation.
pub(crate) async fn stream_model(
    model: LlmModel,
    request: LlmRequest,
    cancellation_token: CancellationToken,
    event_tx: mpsc::Sender<SessionEvent>,
) -> ModelStreamOutcome {
    let mut partial_response = Vec::new();
    let stream = tokio::select! {
        biased;
        _ = cancellation_token.cancelled() => {
            return cancelled(partial_response);
        }
        result = model.stream(request) => match result {
            Ok(stream) => stream,
            Err(err) => {
                return ModelStreamOutcome {
                    terminal: ModelStreamTerminal::Failed(SessionError::Model(err.to_string())),
                    partial_response,
                };
            }
        }
    };
    futures::pin_mut!(stream);

    loop {
        let event = tokio::select! {
            biased;
            _ = cancellation_token.cancelled() => {
                stream.as_mut().get_mut().abort();
                return cancelled(partial_response);
            }
            event = stream.next() => event,
        };

        match event {
            Some(Ok(event)) => {
                let done = matches!(event, StreamEvent::Done { .. });
                convert_and_emit(&event_tx, &event);
                partial_response.push(event);
                if done {
                    return ModelStreamOutcome {
                        terminal: ModelStreamTerminal::Complete,
                        partial_response,
                    };
                }
            }
            Some(Err(err)) => {
                return ModelStreamOutcome {
                    terminal: ModelStreamTerminal::Failed(SessionError::Model(err.to_string())),
                    partial_response,
                };
            }
            None => {
                return ModelStreamOutcome {
                    terminal: ModelStreamTerminal::Complete,
                    partial_response,
                };
            }
        }
    }
}

fn cancelled(partial_response: Vec<StreamEvent>) -> ModelStreamOutcome {
    ModelStreamOutcome {
        terminal: ModelStreamTerminal::Cancelled,
        partial_response,
    }
}

/// Convert provider StreamEvents into LlmDeltas
/// TODO: should be converted into impl From<...> instead of this nonsense
fn convert_and_emit(event_tx: &mpsc::Sender<SessionEvent>, event: &StreamEvent) {
    let delta = match event {
        StreamEvent::TextDelta(delta) => LlmDelta::AssistantContent(delta.clone()),
        StreamEvent::ReasoningDelta(delta) => LlmDelta::AssistantReasoning(delta.clone()),
        StreamEvent::ToolCall {
            id,
            name,
            arguments,
        }
        | StreamEvent::ToolCallComplete {
            id,
            name,
            arguments,
        } => LlmDelta::ToolCall {
            id: id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        },
        StreamEvent::Done { .. } => return,
    };
    let _ = event_tx.send(SessionEvent::LlmDelta(delta));
}
