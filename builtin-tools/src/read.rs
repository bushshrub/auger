use auger_toolengine::JsonSchema;
use auger_toolengine::PathAccess;
use auger_toolengine::Tool;
use auger_toolengine::ToolContext;
use auger_toolengine::ToolDetails;
use auger_toolengine::ToolError;
use async_trait::async_trait;
use serde_json::json;

const INLINE_LINE_LIMIT: usize = 500;

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "read_file".to_string(),
            description: "Read a file from the local filesystem. Returns file contents with line \
                          numbers. Use offset and limit to read a specific range of lines. If the \
                          output exceeds 500 lines and no limit is set, the result is written to \
                          a temp file instead."
                .to_string(),
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

    fn path(&self, args: &serde_json::Value) -> Option<PathAccess> {
        Some(PathAccess {
            path: args["path"].as_str()?.into(),
            write: false,
        })
    }

    async fn call(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<String, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: path".into()))?;
        let file = ctx.resolve(path);

        let offset = args["offset"].as_u64().unwrap_or(1) as usize;
        let limit = args["limit"].as_u64().map(|n| n as usize);

        let contents = tokio::fs::read_to_string(&file).await?;

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

        let line_count = end - start;
        let width = end.to_string().len();
        let mut out = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let lineno = start + i + 1;
            out.push_str(&format!("{lineno:>width$} {line}\n"));
        }

        if limit.is_none() && line_count > INLINE_LINE_LIMIT {
            let tmp_path = format!("/tmp/auger_read_{}.txt", sanitize_filename(path));
            tokio::fs::write(&tmp_path, &out).await?;
            return Ok(format!(
                "File has {total} lines (exceeds inline limit of {INLINE_LINE_LIMIT}). Full \
                 contents written to: {tmp_path}"
            ));
        }

        let mut result = format!("Total lines: {total}\n\n");
        result.push_str(&out);
        Ok(result)
    }
}

fn sanitize_filename(path: &str) -> String {
    path.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
