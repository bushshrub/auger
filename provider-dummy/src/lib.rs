use futures::stream;
use futures::StreamExt;
use provider::Block;
use provider::BlockKind;
use provider::CompletedLlmResponse;
use provider::LlmError;
use provider::LlmProvider;
use provider::LlmRequest;
use provider::StreamEnd;
use provider::StreamEvent;
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
            kind: provider::LlmErrorKind::Fatal,
            message: "dummy provider has no queued response".to_string(),
            status: None,
            request_id: None,
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
                kind: provider::LlmErrorKind::Fatal,
                message: "dummy provider queued a stream response for complete".to_string(),
                status: None,
                request_id: None,
            }),
        }
    }

    async fn stream(
        &self,
        model: &str,
        request: LlmRequest,
        sink: provider::EventSink<'_>,
    ) -> Result<StreamEnd, LlmError> {
        debug!(model, "dummy provider stream called");
        match self.next_response(request)? {
            DummyResponse::Response(response) => {
                for event in response_to_stream_events(&response) {
                    sink(event);
                }
                Ok(StreamEnd {
                    usage: response.usage,
                    stop_reason: response.stop_reason,
                })
            }
            DummyResponse::Error(error) => Err(error),
            DummyResponse::Stream(events) => {
                for event in events {
                    sink(event?);
                }
                Ok(StreamEnd { usage: None, stop_reason: None })
            }
            DummyResponse::PendingStream(events) => {
                let s = stream::iter(events);
                futures::pin_mut!(s);
                while let Some(event) = s.next().await {
                    sink(event?);
                }
                Ok(StreamEnd { usage: None, stop_reason: None })
            }
        }
    }
}

fn response_to_stream_events(response: &CompletedLlmResponse) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    for block in response.response.blocks() {
        match block {
            Block::Text(text) => {
                if !text.is_empty() {
                    events.push(StreamEvent::BlockStart { index: 0, kind: BlockKind::Text });
                    events.push(StreamEvent::BlockDelta { index: 0, delta: text.clone() });
                }
            }
            Block::Reasoning { text } => {
                if !text.is_empty() {
                    events.push(StreamEvent::BlockStart { index: 0, kind: BlockKind::Reasoning });
                    events.push(StreamEvent::BlockDelta { index: 0, delta: text.clone() });
                }
            }
            Block::ToolCall(tc) => {
                events.push(StreamEvent::BlockStart {
                    index: 0,
                    kind: BlockKind::ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                    },
                });
                events.push(StreamEvent::BlockDelta { index: 0, delta: tc.arguments.clone() });
                events.push(StreamEvent::BlockEnd { index: 0 });
            }
        }
    }

    events
}
