use async_trait::async_trait;
use serde_json::json;

use crate::{JsonSchema, Tool, ToolDetails, ToolError};

pub struct Dummy;

#[async_trait]
impl Tool for Dummy {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "dummy",
            description: "Test tool. Echoes back its arguments. Use to verify tool call routing works.",
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

        Ok(json!({ "echo": message }).to_string())
    }
}
