use async_trait::async_trait;
use serde_json::json;

use crate::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct Shell;

const MAX_OUTPUT: usize = 20_000;

#[async_trait]
impl Tool for Shell {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "shell",
            description: "Run a shell command and return its stdout, stderr, and exit code. \
                Use for builds, tests, git, find, etc. Avoid interactive commands.",
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

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: command".into()))?
            .to_owned();

        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(&command)
                .output()
        })
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))??;

        let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        if stdout.len() > MAX_OUTPUT {
            stdout.truncate(MAX_OUTPUT);
            stdout.push_str("\n[truncated]");
        }
        if stderr.len() > MAX_OUTPUT {
            stderr.truncate(MAX_OUTPUT);
            stderr.push_str("\n[truncated]");
        }

        let exit_code = output.status.code().unwrap_or(-1);

        Ok(json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code
        })
        .to_string()
        .into())
    }
}
