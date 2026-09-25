use auger_core::JsonSchema;
use auger_core::Tool;
use auger_core::ToolDetails;
use auger_core::ToolError;
use async_trait::async_trait;
use serde_json::json;

pub struct Shell;

const MAX_OUTPUT: usize = 20_000;

#[async_trait]
impl Tool for Shell {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "shell".to_string(),
            description: "Run a shell command and return its stdout, stderr, and exit code. Use \
                          for builds, tests, git, find, etc. Avoid interactive commands."
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

    async fn call(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: command".into()))?
            .to_owned();

        let output = tokio::task::spawn_blocking(move || {
            let output = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(&command)
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
