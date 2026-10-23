use auger_toolengine::JsonSchema;
use auger_toolengine::PathAccess;
use auger_toolengine::Tool;
use auger_toolengine::ToolContext;
use auger_toolengine::ToolDetails;
use auger_toolengine::ToolError;
use async_trait::async_trait;
use serde_json::json;
use tokio::fs;

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "write_file".to_string(),
            description: "Write content to a file, creating it (and any missing parent \
                          directories) if it does not exist, or overwriting it if it does."
                .to_string(),
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

    fn path(&self, args: &serde_json::Value) -> Option<PathAccess> {
        Some(PathAccess {
            path: args["path"].as_str()?.into(),
            write: true,
        })
    }

    async fn call(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<String, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: path".into()))?;
        let file = ctx.resolve(path);
        let content = args["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: content".into()))?;

        if let Some(parent) = file.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).await?;
            }
        }

        fs::write(&file, content).await?;

        let lines = content.lines().count();
        Ok(
            json!({ "success": true, "path": path, "lines_written": lines }).to_string(),
        )
    }
}
