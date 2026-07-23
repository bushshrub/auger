use agent_tools::JsonSchema;
use agent_tools::Tool;
use agent_tools::ToolCallResult;
use agent_tools::ToolDetails;
use agent_tools::ToolError;
use async_trait::async_trait;
use serde_json::json;

pub struct Dummy;

#[async_trait]
impl Tool for Dummy {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "dummy".to_string(),
            description: "Test tool. Echoes back its arguments. Use to verify tool call routing \
                          works."
                .to_string(),
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Any string to echo back"
                }
            },
            "required": ["message"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let message = args["message"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: message".into()))?;

        Ok(ToolCallResult::success(
            json!({ "echo": message }).to_string(),
        ))
    }
}
