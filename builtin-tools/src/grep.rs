use async_trait::async_trait;
use grep::regex::RegexMatcherBuilder;
use grep::searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkBuilder;
use serde_json::json;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use agent_tools::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct Grep;

impl Grep {
    pub(crate) fn run_shell_command(words: &[String]) -> Option<Result<String, ToolError>> {
        let name = words.first()?.as_str();
        if name != "grep" && name != "rg" {
            return None;
        }

        let mut recursive = name == "rg";
        let mut case_insensitive = false;
        let mut fixed_string = false;
        let mut index = 1;
        while let Some(argument) = words.get(index) {
            if argument == "--" {
                index += 1;
                break;
            }
            if !argument.starts_with('-') || argument == "-" {
                break;
            }
            for option in argument[1..].chars() {
                match option {
                    'i' => case_insensitive = true,
                    'r' | 'R' => recursive = true,
                    'E' | 'G' | 'H' | 'n' | 's' => {}
                    'F' => fixed_string = true,
                    _ => return None,
                }
            }
            index += 1;
        }

        let pattern = words.get(index).map(|pattern| {
            if fixed_string {
                escape_regex(pattern)
            } else {
                pattern.clone()
            }
        })?;
        let path = words.get(index + 1).cloned().unwrap_or_else(|| ".".into());
        if words.get(index + 2).is_some() {
            return None;
        }

        Some(run_grep(
            &pattern,
            &path,
            recursive,
            case_insensitive,
            0,
            500,
        ))
    }
}

fn escape_regex(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '.' | '^' | '$' | '|' | '(' | ')' | '[' | ']' | '{' | '}' | '*' | '+' | '?') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

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

        Ok(ToolCallResult::success(output))
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
    if !target.is_file() && !target.is_dir() {
        return Err(ToolError::InvalidArgs(format!("path not found: {path}")));
    }

    let files = if target.is_file() {
        vec![target.to_path_buf()]
    } else {
        let max_depth = if recursive { None } else { Some(1) };
        let mut walker = WalkBuilder::new(target);
        walker.standard_filters(true);
        if let Some(max_depth) = max_depth {
            walker.max_depth(Some(max_depth));
        }

        walker
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|file_type| file_type.is_file()))
            .map(|entry| entry.into_path())
            .collect()
    };

    let match_count = Arc::new(AtomicUsize::new(0));
    let truncated = Arc::new(AtomicBool::new(false));
    let next_file = Arc::new(AtomicUsize::new(0));
    let results = Arc::new(Mutex::new(vec![None; files.len()]));
    let thread_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(files.len().max(1));

    std::thread::scope(|scope| {
        for _ in 0..thread_count {
            let next_file = Arc::clone(&next_file);
            let match_count = Arc::clone(&match_count);
            let truncated = Arc::clone(&truncated);
            let results = Arc::clone(&results);
            let matcher = &matcher;
            let files = &files;

            scope.spawn(move || {
                let mut searcher = SearcherBuilder::new()
                    .before_context(context_lines)
                    .after_context(context_lines)
                    .line_number(true)
                    .build();

                loop {
                    let index = next_file.fetch_add(1, Ordering::Relaxed);
                    if index >= files.len() || match_count.load(Ordering::Relaxed) >= max_matches {
                        break;
                    }
                    let lines = search_file(
                        &files[index],
                        matcher,
                        &mut searcher,
                        max_matches,
                        &match_count,
                        &truncated,
                    );
                    results.lock().unwrap()[index] = Some(lines);
                }
            });
        }
    });

    let all_lines: Vec<String> = results
        .lock()
        .unwrap()
        .iter_mut()
        .filter_map(Option::take)
        .flatten()
        .collect();

    if all_lines.is_empty() {
        return Ok("No matches found.".to_string());
    }

    let mut output = all_lines.join("\n");
    if truncated.load(Ordering::Relaxed) {
        output.push_str(&format!("\n[Truncated at {max_matches} matches]"));
    }
    Ok(output)
}

fn search_file(
    file_path: &Path,
    matcher: &grep::regex::RegexMatcher,
    searcher: &mut Searcher,
    max_matches: usize,
    match_count: &AtomicUsize,
    truncated: &AtomicBool,
) -> Vec<String> {
    let display = file_path.to_string_lossy().into_owned();
    let mut sink = CollectSink::new(display, match_count, max_matches, truncated);
    // Silently skip unreadable or binary files.
    let _ = searcher.search_path(matcher, file_path, &mut sink);
    sink.lines
}

struct CollectSink<'a> {
    path: String,
    lines: Vec<String>,
    match_count: &'a AtomicUsize,
    max_matches: usize,
    truncated: &'a AtomicBool,
}

impl<'a> CollectSink<'a> {
    fn new(
        path: String,
        match_count: &'a AtomicUsize,
        max_matches: usize,
        truncated: &'a AtomicBool,
    ) -> Self {
        Self {
            path,
            lines: Vec::new(),
            match_count,
            max_matches,
            truncated,
        }
    }
}

impl Sink for CollectSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let mut current = self.match_count.load(Ordering::Relaxed);
        loop {
            if current >= self.max_matches {
                self.truncated.store(true, Ordering::Relaxed);
                return Ok(false);
            }
            match self.match_count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(updated) => current = updated,
            }
        }
        let line = String::from_utf8_lossy(mat.bytes())
            .trim_end_matches(|c| c == '\n' || c == '\r')
            .to_string();
        let lineno = mat.line_number().unwrap_or(0);
        self.lines
            .push(format!("{}:{}:{}", self.path, lineno, line));
        Ok(true)
    }

    fn context(&mut self, _: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, Self::Error> {
        let line = String::from_utf8_lossy(ctx.bytes())
            .trim_end_matches(|c| c == '\n' || c == '\r')
            .to_string();
        let lineno = ctx.line_number().unwrap_or(0);
        self.lines
            .push(format!("{}-{}-{}", self.path, lineno, line));
        Ok(true)
    }
}
