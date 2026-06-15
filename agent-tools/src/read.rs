use async_trait::async_trait;

use crate::{Tool, ToolError};

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file from the local filesystem. Supports optional line range via offset and limit."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to read."
                },
                "offset": {
                    "type": "integer",
                    "description": "0-based line number to start reading from. Requires limit to be set."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read. Use with offset to paginate through large files."
                }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing required argument: path".into()))?;

        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let limit = args.get("limit").and_then(|v| v.as_u64());

        let content = tokio::fs::read_to_string(path).await?;

        let lines: Vec<&str> = content.lines().collect();
        let start = offset as usize;
        let total = lines.len();

        let (selected_lines, truncated) = if let Some(lim) = limit {
            let end = (start + lim as usize).min(total);
            let has_more = end < total;
            (lines[start..end].to_vec(), has_more)
        } else {
            (lines[start..].to_vec(), false)
        };

        let result_content = selected_lines.join("\n");

        Ok(serde_json::json!({
            "path": path,
            "content": result_content,
            "lines": {
                "from": start + 1,
                "to": start + selected_lines.len(),
                "total": total
            },
            "truncated": truncated
        }))
    }
}
