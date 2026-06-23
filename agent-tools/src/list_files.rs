use async_trait::async_trait;
use serde_json::json;

use crate::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct ListFiles;

#[async_trait]
impl Tool for ListFiles {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "list_files",
            description: "List files and directories at a given path. Non-recursive by default; set recursive=true to walk the full tree.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the directory to list"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "If true, list all files recursively",
                    "default": false
                }
            },
            "required": ["path"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: path".into()))?;
        let recursive = args["recursive"].as_bool().unwrap_or(false);

        let entries = collect_entries(path, recursive).await?;
        Ok(json!({ "entries": entries }).to_string().into())
    }
}

async fn collect_entries(path: &str, recursive: bool) -> Result<Vec<String>, ToolError> {
    let mut entries: Vec<String> = Vec::new();
    collect_dir(path, path, recursive, &mut entries).await?;
    entries.sort();
    Ok(entries)
}

fn collect_dir<'a>(
    root: &'a str,
    dir: &'a str,
    recursive: bool,
    out: &'a mut Vec<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ToolError>> + Send + 'a>> {
    Box::pin(async move {
        let mut read_dir = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let entry_path = entry.path();
            let relative = entry_path
                .strip_prefix(root)
                .unwrap_or(&entry_path)
                .to_string_lossy()
                .to_string();

            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                out.push(format!("{}/", relative));
                if recursive {
                    collect_dir(root, entry_path.to_str().unwrap_or(dir), recursive, out).await?;
                }
            } else {
                out.push(relative);
            }
        }
        Ok(())
    })
}
