use provider::{LlmError, LlmProvider, LlmRequest, LlmResponse, LlmStream};

pub struct OpenAiChatCompletionsProvider {

}

impl OpenAiChatCompletionsProvider {
    pub fn new() -> Self {
        todo!()
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiChatCompletionsProvider {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        todo!()
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        todo!()
    }
}