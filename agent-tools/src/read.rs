use async_trait::async_trait;
use serde_json::json;

use crate::{JsonSchema, Tool, ToolDetails, ToolError};

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "read_file",
            description: "Read a file from the local filesystem. Returns file contents with line numbers. Use offset and limit to read a specific range of lines.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "1-indexed line number to start reading from",
                    "minimum": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read",
                    "minimum": 1
                }
            },
            "required": ["path"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: path".into()))?;

        let offset = args["offset"].as_u64().unwrap_or(1) as usize;
        let limit = args["limit"].as_u64().map(|n| n as usize);

        let contents = tokio::fs::read_to_string(path).await?;

        let lines: Vec<&str> = contents.lines().collect();
        let total = lines.len();

        if offset > total + 1 {
            return Err(ToolError::InvalidArgs(format!(
                "offset {offset} exceeds file length ({total} lines)"
            )));
        }

        let start = offset.saturating_sub(1);
        let end = match limit {
            Some(n) => (start + n).min(total),
            None => total,
        };

        let mut out = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let lineno = start + i + 1;
            out.push_str(&format!("{lineno}\t{line}\n"));
        }

        Ok(json!({ "content": out, "total_lines": total }))
    }
}
