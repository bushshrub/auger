use async_trait::async_trait;
use serde_json::json;

use agent_tools::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct WebFetch {
    client: reqwest::Client,
}

// ~8000 chars ≈ 2000 tokens (English words ~1.3 tokens/word, ~5 chars/word → 3.85 chars/token)
const MAX_INLINE: usize = 8_000;

impl WebFetch {
    pub fn new() -> Self {
        WebFetch {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (compatible; auger-agent/0.1)")
                .build()
                .expect("failed to build reqwest client"),
        }
    }
}

impl Default for WebFetch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetch {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "web_fetch",
            description: "Fetch a URL and return its content as text. \
                Useful for reading documentation, APIs, or web pages. \
                HTML is returned as-is; prefer URLs that serve plain text or JSON when possible. \
                If the response body exceeds the inline limit the full content is saved to a \
                temporary file and the path is returned instead.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                }
            },
            "required": ["url"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: url".into()))?
            .to_owned();

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("request failed: {e}")))?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_owned();

        let body = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("failed to read body: {e}")))?;

        if body.len() <= MAX_INLINE {
            return Ok(ToolCallResult::success(
                json!({
                    "status": status,
                    "content_type": content_type,
                    "body": body,
                })
                .to_string(),
            ));
        }

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let path = std::env::temp_dir().join(format!("web_fetch_{nanos}.txt"));
        tokio::fs::write(&path, body.as_bytes())
            .await
            .map_err(|e| ToolError::Execution(format!("failed to write temp file: {e}")))?;

        Ok(ToolCallResult::success(
            json!({
                "status": status,
                "content_type": content_type,
                "body_size_bytes": body.len(),
                "full_response_path": path.to_string_lossy(),
                "note": format!(
                    "Response body ({} bytes) exceeds inline limit ({} bytes). \
                    Full content saved to: {}",
                    body.len(),
                    MAX_INLINE,
                    path.display()
                ),
            })
            .to_string(),
        ))
    }
}
