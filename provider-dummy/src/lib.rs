use futures::StreamExt;
use futures::stream;
use provider::CompletedLlmResponse;
use provider::LlmError;
use provider::LlmProvider;
use provider::LlmRequest;
use provider::LlmStream;
use provider::StreamEvent;
use provider::ToolCallRequest;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::debug;

#[derive(Clone, Debug, Default)]
pub struct DummyProvider {
    state: Arc<Mutex<DummyProviderState>>,
}

#[derive(Debug, Default)]
struct DummyProviderState {
    requests: Vec<LlmRequest>,
    responses: VecDeque<DummyResponse>,
}

#[derive(Debug, Clone)]
pub enum DummyResponse {
    Response(CompletedLlmResponse),
    Error(LlmError),
    Stream(Vec<Result<StreamEvent, LlmError>>),
    PendingStream(Vec<Result<StreamEvent, LlmError>>),
}

impl From<CompletedLlmResponse> for DummyResponse {
    fn from(response: CompletedLlmResponse) -> Self {
        Self::Response(response)
    }
}

impl DummyProvider {
    pub fn new(responses: impl IntoIterator<Item = CompletedLlmResponse>) -> Self {
        Self::new_responses(responses.into_iter().map(DummyResponse::from))
    }

    pub fn new_responses(responses: impl IntoIterator<Item = DummyResponse>) -> Self {
        Self {
            state: Arc::new(Mutex::new(DummyProviderState {
                requests: Vec::new(),
                responses: responses.into_iter().collect(),
            })),
        }
    }

    pub fn requests(&self) -> Vec<LlmRequest> {
        self.state
            .lock()
            .expect("dummy provider mutex poisoned")
            .requests
            .clone()
    }

    fn next_response(&self, request: LlmRequest) -> Result<DummyResponse, LlmError> {
        let mut state = self.state.lock().expect("dummy provider mutex poisoned");
        state.requests.push(request);
        state.responses.pop_front().ok_or_else(|| LlmError {
            message: "dummy provider has no queued response".to_string(),
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for DummyProvider {
    async fn complete(
        &self,
        model: &str,
        request: LlmRequest,
    ) -> Result<CompletedLlmResponse, LlmError> {
        debug!(model, "dummy provider complete called");
        match self.next_response(request)? {
            DummyResponse::Response(response) => Ok(response),
            DummyResponse::Error(error) => Err(error),
            DummyResponse::Stream(_) | DummyResponse::PendingStream(_) => Err(LlmError {
                message: "dummy provider queued a stream response for complete".to_string(),
            }),
        }
    }

    async fn stream(&self, model: &str, request: LlmRequest) -> Result<LlmStream, LlmError> {
        debug!(model, "dummy provider stream called");
        match self.next_response(request)? {
            DummyResponse::Response(response) => Ok(LlmStream::new(finite_stream(
                response_to_stream_events(response),
                model,
            ))),
            DummyResponse::Error(error) => Err(error),
            DummyResponse::Stream(events) => Ok(LlmStream::new(finite_stream(events, model))),
            DummyResponse::PendingStream(events) => Ok(LlmStream::new(
                stream::iter(events).chain(stream::pending()),
            )),
        }
    }
}

fn finite_stream(
    events: Vec<Result<StreamEvent, LlmError>>,
    model: &str,
) -> impl futures::Stream<Item = Result<StreamEvent, LlmError>> + Send + 'static {
    let model = model.to_string();
    stream::unfold(
        (VecDeque::from(events), model),
        |(mut events, model)| async move {
            match events.pop_front() {
                Some(event) => Some((event, (events, model))),
                None => {
                    debug!(model = %model, "dummy provider stream ended");
                    None
                }
            }
        },
    )
}

fn response_to_stream_events(response: CompletedLlmResponse) -> Vec<Result<StreamEvent, LlmError>> {
    let mut events = Vec::new();

    if !response.content.is_empty() {
        events.push(Ok(StreamEvent::TextDelta(response.content)));
    }

    if let Some(reasoning) = response.reasoning {
        events.push(Ok(StreamEvent::ReasoningDelta(reasoning)));
    }

    for ToolCallRequest {
        id,
        name,
        arguments,
    } in response.tool_calls.unwrap_or_default()
    {
        events.push(Ok(StreamEvent::ToolCallComplete {
            id,
            name,
            arguments,
        }));
    }

    events.push(Ok(StreamEvent::Done {
        usage: response.usage,
        stop_reason: response.stop_reason,
    }));

    events
}
