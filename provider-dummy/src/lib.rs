use futures::stream;
use provider::{
    LlmError, LlmProvider, LlmRequest, LlmResponse, LlmStream, StreamEvent, ToolCallRequest,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

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
    Response(LlmResponse),
    Stream(Vec<Result<StreamEvent, LlmError>>),
}

impl From<LlmResponse> for DummyResponse {
    fn from(response: LlmResponse) -> Self {
        Self::Response(response)
    }
}

impl DummyProvider {
    pub fn new(responses: impl IntoIterator<Item = LlmResponse>) -> Self {
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
    async fn complete(&self, _model: &str, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        match self.next_response(request)? {
            DummyResponse::Response(response) => Ok(response),
            DummyResponse::Stream(_) => Err(LlmError {
                message: "dummy provider queued a stream response for complete".to_string(),
            }),
        }
    }

    async fn stream(&self, _model: &str, request: LlmRequest) -> Result<LlmStream, LlmError> {
        match self.next_response(request)? {
            DummyResponse::Response(response) => {
                Ok(Box::pin(stream::iter(response_to_stream_events(response))))
            }
            DummyResponse::Stream(events) => Ok(Box::pin(stream::iter(events))),
        }
    }
}

fn response_to_stream_events(response: LlmResponse) -> Vec<Result<StreamEvent, LlmError>> {
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
