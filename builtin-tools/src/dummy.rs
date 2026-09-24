use auger_core::JsonSchema;
use auger_core::Tool;
use auger_core::ToolDetails;
use auger_core::ToolError;
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

    async fn call(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let message = args["message"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: message".into()))?;

        Ok(
            json!({ "echo": message }).to_string(),
        )
    }
}
