use async_trait::async_trait;

use crate::{Tool, ToolError};

const DEFAULT_MAX_LENGTH: usize = 32_768;

pub struct WebFetch;

#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL and return the response body as text."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch."
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum number of characters to return. Defaults to 32768."
                }
            },
            "required": ["url"]
        })
    }

    async fn call(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing required argument: url".into()))?;

        let max_length = args
            .get("max_length")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_LENGTH);

        let resp = reqwest::get(url)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp
            .text()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        if status >= 400 {
            return Ok(serde_json::json!({
                "url": url,
                "status": status,
                "error": format!("HTTP {status}")
            }));
        }

        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let (content, truncated) = if body.len() > max_length {
            (body[..max_length].to_string(), true)
        } else {
            (body, false)
        };

        Ok(serde_json::json!({
            "url": url,
            "status": status,
            "content_type": content_type,
            "content": content,
            "truncated": truncated
        }))
    }
}
