use async_trait::async_trait;
use serde_json::json;
use tokio::fs;

use crate::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct EditFile;

#[async_trait]
impl Tool for EditFile {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "edit_file",
            description: "Perform an exact string replacement in a file. \
                Fails if old_string is not found or appears more than once.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact string to replace; must appear exactly once in the file"
                },
                "new_string": {
                    "type": "string",
                    "description": "The string to substitute in place of old_string"
                }
            },
            "required": ["path", "old_string", "new_string"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: path".into()))?;
        let old_string = args["old_string"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: old_string".into()))?;
        let new_string = args["new_string"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: new_string".into()))?;

        let content = fs::read_to_string(path).await?;

        let count = content.matches(old_string).count();
        match count {
            0 => {
                return Err(ToolError::Execution(format!(
                    "old_string not found in {path}"
                )))
            }
            n if n > 1 => {
                return Err(ToolError::Execution(format!(
                    "old_string appears {n} times in {path}; it must be unique"
                )))
            }
            _ => {}
        }

        let new_content = content.replacen(old_string, new_string, 1);
        fs::write(path, &new_content).await?;

        Ok(ToolCallResult::success(json!({ "success": true, "path": path }).to_string()))
    }
}
