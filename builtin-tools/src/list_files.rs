use async_trait::async_trait;
use serde_json::json;

use agent_tools::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct ListFiles;

#[async_trait]
impl Tool for ListFiles {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "list_files",
            description: "List files and directories at a given path. Non-recursive by default; set recursive=true to walk the full tree. Respects .gitignore.",
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

        let path = path.to_string();
        let entries = tokio::task::spawn_blocking(move || collect_entries(&path, recursive))
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))??;

        Ok(ToolCallResult::success(
            json!({ "entries": entries }).to_string(),
        ))
    }
}

fn collect_entries(root: &str, recursive: bool) -> Result<Vec<String>, ToolError> {
    let max_depth = if recursive { None } else { Some(1) };

    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true);

    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }

    let mut entries: Vec<String> = Vec::new();

    for result in builder.build() {
        let entry = result.map_err(|e| ToolError::Execution(e.to_string()))?;
        let path = entry.path();

        // Skip the root itself
        if path == std::path::Path::new(root) {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if path.is_dir() {
            entries.push(format!("{}/", relative));
        } else {
            entries.push(relative);
        }
    }

    entries.sort();
    Ok(entries)
}
