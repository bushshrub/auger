use async_trait::async_trait;
use serde_json::json;
use tokio::fs;

use crate::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "write_file",
            description: "Write content to a file, creating it (and any missing parent directories) \
                if it does not exist, or overwriting it if it does.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Full content to write to the file"
                }
            },
            "required": ["path", "content"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: path".into()))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: content".into()))?;

        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).await?;
            }
        }

        fs::write(path, content).await?;

        let lines = content.lines().count();
        Ok(json!({ "success": true, "path": path, "lines_written": lines })
            .to_string()
            .into())
    }
}
