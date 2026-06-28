use async_trait::async_trait;
use serde_json::json;

use crate::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct Glob;

#[async_trait]
impl Tool for Glob {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "glob",
            description: "Find files and directories matching a glob pattern. Returns one absolute \
                path per line. Supports `*`, `**`, and `?` wildcards. Respects .gitignore.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match (e.g. 'src/**/*.rs'). \
                        Relative patterns are resolved against `base_dir`."
                },
                "base_dir": {
                    "type": "string",
                    "description": "Absolute directory to resolve relative patterns from. \
                        Defaults to the current working directory."
                }
            },
            "required": ["pattern"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: pattern".into()))?
            .to_string();
        let base_dir = args["base_dir"].as_str().map(str::to_string);

        let output = tokio::task::spawn_blocking(move || run_glob(&pattern, base_dir.as_deref()))
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))??;

        Ok(output.into())
    }
}

fn run_glob(pattern: &str, base_dir: Option<&str>) -> Result<String, ToolError> {
    let full_pattern = if std::path::Path::new(pattern).is_absolute() {
        pattern.to_string()
    } else {
        let base = match base_dir {
            Some(d) => d.to_string(),
            None => std::env::current_dir()
                .map_err(|e| ToolError::Io(e))?
                .to_string_lossy()
                .into_owned(),
        };
        format!("{}/{}", base.trim_end_matches('/'), pattern)
    };

    let git_root = find_git_root(std::path::Path::new(
        base_dir.unwrap_or("."),
    ));

    // Build a gitignore checker rooted at the git root (or cwd if not in a repo)
    let checker = git_root.as_deref().and_then(|root| {
        ignore::gitignore::GitignoreBuilder::new(root)
            .build()
            .ok()
    });

    let paths: Vec<String> = glob::glob(&full_pattern)
        .map_err(|e| ToolError::InvalidArgs(format!("invalid glob pattern: {e}")))?
        .filter_map(|entry| entry.ok())
        .filter(|path| {
            let Some(ref checker) = checker else {
                return true;
            };
            let is_dir = path.is_dir();
            !matches!(
                checker.matched(path, is_dir),
                ignore::Match::Ignore(_)
            )
        })
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    if paths.is_empty() {
        return Ok("No matches found.".to_string());
    }

    Ok(paths.join("\n"))
}

fn find_git_root(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut current = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}
