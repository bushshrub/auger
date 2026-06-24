use async_trait::async_trait;
use grep::regex::RegexMatcherBuilder;
use grep::searcher::{SearcherBuilder, Sink, SinkContext, SinkMatch, Searcher};
use serde_json::json;
use std::path::Path;
use walkdir::WalkDir;

use crate::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct Grep;

#[async_trait]
impl Tool for Grep {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "grep",
            description: "Search for a regex pattern in a file or directory. Returns matches in \
                path:line:content format. Context lines use path-line-content format.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Absolute path to a file or directory to search"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "When path is a directory, search recursively. Default: true"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive matching. Default: false"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Lines of context before and after each match. Default: 0",
                    "minimum": 0
                },
                "max_matches": {
                    "type": "integer",
                    "description": "Maximum number of matches before truncating. Default: 500",
                    "minimum": 1
                }
            },
            "required": ["pattern", "path"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: pattern".into()))?
            .to_string();
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: path".into()))?
            .to_string();
        let recursive = args["recursive"].as_bool().unwrap_or(true);
        let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
        let context_lines = args["context_lines"].as_u64().unwrap_or(0) as usize;
        let max_matches = args["max_matches"].as_u64().unwrap_or(500) as usize;

        let output = tokio::task::spawn_blocking(move || {
            run_grep(
                &pattern,
                &path,
                recursive,
                case_insensitive,
                context_lines,
                max_matches,
            )
        })
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))??;

        Ok(output.into())
    }
}

fn run_grep(
    pattern: &str,
    path: &str,
    recursive: bool,
    case_insensitive: bool,
    context_lines: usize,
    max_matches: usize,
) -> Result<String, ToolError> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .build(pattern)
        .map_err(|e| ToolError::InvalidArgs(format!("invalid regex: {e}")))?;

    let target = Path::new(path);
    let files: Vec<std::path::PathBuf> = if target.is_file() {
        vec![target.to_path_buf()]
    } else if target.is_dir() {
        let max_depth = if recursive { usize::MAX } else { 1 };
        WalkDir::new(target)
            .max_depth(max_depth)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .collect()
    } else {
        return Err(ToolError::InvalidArgs(format!("path not found: {path}")));
    };

    let mut searcher = SearcherBuilder::new()
        .before_context(context_lines)
        .after_context(context_lines)
        .line_number(true)
        .build();

    let mut all_lines: Vec<String> = Vec::new();
    let mut match_count = 0usize;
    let mut truncated = false;

    for file_path in &files {
        let display = file_path.to_string_lossy().into_owned();
        let mut sink = CollectSink::new(display, match_count, max_matches);
        // Silently skip unreadable or binary files.
        let _ = searcher.search_path(&matcher, file_path, &mut sink);
        match_count = sink.match_count;
        truncated = sink.truncated;
        all_lines.extend(sink.lines);
        if truncated {
            break;
        }
    }

    if all_lines.is_empty() {
        return Ok("No matches found.".to_string());
    }

    let mut output = all_lines.join("\n");
    if truncated {
        output.push_str(&format!("\n[Truncated at {max_matches} matches]"));
    }
    Ok(output)
}

struct CollectSink {
    path: String,
    lines: Vec<String>,
    match_count: usize,
    max_matches: usize,
    truncated: bool,
}

impl CollectSink {
    fn new(path: String, match_count: usize, max_matches: usize) -> Self {
        Self {
            path,
            lines: Vec::new(),
            match_count,
            max_matches,
            truncated: false,
        }
    }
}

impl Sink for CollectSink {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if self.match_count >= self.max_matches {
            self.truncated = true;
            return Ok(false);
        }
        let line = String::from_utf8_lossy(mat.bytes())
            .trim_end_matches(|c| c == '\n' || c == '\r')
            .to_string();
        let lineno = mat.line_number().unwrap_or(0);
        self.lines.push(format!("{}:{}:{}", self.path, lineno, line));
        self.match_count += 1;
        Ok(true)
    }

    fn context(&mut self, _: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, Self::Error> {
        let line = String::from_utf8_lossy(ctx.bytes())
            .trim_end_matches(|c| c == '\n' || c == '\r')
            .to_string();
        let lineno = ctx.line_number().unwrap_or(0);
        self.lines.push(format!("{}-{}-{}", self.path, lineno, line));
        Ok(true)
    }
}
