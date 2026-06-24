use async_trait::async_trait;
use serde_json::json;

use crate::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};

pub struct WebFetch {
    client: reqwest::Client,
}

const MAX_BODY: usize = 50_000;

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
                HTML is returned as-is; prefer URLs that serve plain text or JSON when possible.",
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

        let mut body = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("failed to read body: {e}")))?;

        let truncated = body.len() > MAX_BODY;
        if truncated {
            body.truncate(MAX_BODY);
            body.push_str("\n[truncated]");
        }

        Ok(json!({
            "status": status,
            "content_type": content_type,
            "body": body,
            "truncated": truncated
        })
        .to_string()
        .into())
    }
}
