use std::sync::Arc;

use auger_driver::{StreamResult, TypedAgent, WaitingForUserMessage};
use either::Either;
use provider::{LlmModel, LlmResponse, ToolCallRequest, ToolDefinition, ToolResult, UserPrompt};
use provider_dummy::DummyProvider;

#[tokio::test]
async fn completes_one_tool_call_iteration() {
    let provider = DummyProvider::new([
        LlmResponse {
            content: String::new(),
            reasoning: None,
            tool_calls: Some(vec![ToolCallRequest {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                arguments: "{\"path\":\"README.md\"}".to_string(),
            }]),
            usage: None,
            stop_reason: Some("tool_calls".to_string()),
        },
        LlmResponse {
            content: "The file has been read.".to_string(),
            reasoning: None,
            tool_calls: None,
            usage: None,
            stop_reason: Some("stop".to_string()),
        },
    ]);
    let model = LlmModel::new(Arc::new(provider.clone()), "dummy");
    let tools = vec![ToolDefinition {
        name: "read_file".to_string(),
        description: Some("Read a file from the workspace.".to_string()),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    }];

    let agent = TypedAgent::<WaitingForUserMessage>::new(model, "system".to_string(), tools)
        .add_message(UserPrompt::new("Read README.md.".to_string()));
    let result = agent.create_stream().await;
    let agent = match result {
        StreamResult::WaitingForToolResponses(agent) => agent,
        _ => panic!("expected tool calls"),
    };

    let batch = agent.get_batch();
    let batch = match batch
        .add_result(
            "call-1",
            ToolResult::new("call-1".to_string(), "README contents".to_string()),
        )
        .expect("tool call ID should be valid")
    {
        Either::Right(batch) => batch,
        Either::Left(_) => panic!("expected the single-call batch to be complete"),
    };

    let result = agent.add_all_tool_responses(batch).create_stream().await;
    assert!(matches!(result, StreamResult::WaitingForUserMessage(_)));

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].tools().len(), 1);
    assert_eq!(requests[1].tools().len(), 1);
}
