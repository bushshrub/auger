use auger_toolengine::JsonSchema;
use auger_toolengine::Tool;
use auger_toolengine::ToolContext;
use auger_toolengine::ToolDetails;
use auger_toolengine::ToolError;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

pub struct Shell;

const MAX_OUTPUT: usize = 20_000;

static RUN_ID: AtomicU64 = AtomicU64::new(0);

#[async_trait]
impl Tool for Shell {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "shell".to_string(),
            description: "Run a bash command and return its stdout, stderr, and exit code. Use \
                          for builds, tests, git, find, etc. Avoid interactive commands. The \
                          working directory persists between calls (starts at the workspace \
                          root). Prefer absolute paths; don't prefix commands with cd."
                .to_string(),
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                }
            },
            "required": ["command"]
        }))
    }

    fn command(&self, args: &serde_json::Value) -> Option<String> {
        args["command"].as_str().map(str::to_owned)
    }

    async fn call(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<String, ToolError> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: command".into()))?;
        // The final directory is written to `cwd_file`, so a `cd` carries over
        // to the next call. An `exit` skips it and leaves the directory as is.
        let cwd_file = std::env::temp_dir().join(format!(
            "auger-cwd-{}-{}",
            std::process::id(),
            RUN_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let script = format!(
            "{command}\n__auger_status=$?\npwd -P >| \"$AUGER_CWD_FILE\"\nexit $__auger_status"
        );
        let cwd = ctx.cwd().to_path_buf();
        let marker = cwd_file.clone();

        let output = tokio::task::spawn_blocking(move || {
            let output = std::process::Command::new("bash")
                .arg("-c")
                .arg(&script)
                .current_dir(&cwd)
                .env("AUGER_CWD_FILE", &marker)
                .output()
                .map_err(|error| ToolError::Execution(error.to_string()))?;
            Ok::<_, ToolError>((
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
                output.status.code().unwrap_or(-1),
            ))
        })
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))??;

        if let Ok(dir) = std::fs::read_to_string(&cwd_file) {
            let dir = PathBuf::from(dir.trim_end_matches('\n'));
            if dir.is_dir() {
                ctx.set_cwd(dir);
            }
        }
        let _ = std::fs::remove_file(&cwd_file);

        let (mut stdout, mut stderr, exit_code) = output;

        if stdout.len() > MAX_OUTPUT {
            stdout.truncate(MAX_OUTPUT);
            stdout.push_str("\n[truncated]");
        }
        if stderr.len() > MAX_OUTPUT {
            stderr.truncate(MAX_OUTPUT);
            stderr.push_str("\n[truncated]");
        }

        Ok(
            json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code
            })
            .to_string(),
        )
    }
}
