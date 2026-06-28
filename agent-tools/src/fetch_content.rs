use async_trait::async_trait;
use scraper::{ElementRef, Html, Selector};
use serde_json::json;

use crate::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};
use crate::rate_limiter::RateLimiter;

/// Minimum milliseconds between content fetch requests.
const FETCH_RATE_LIMIT_MS: u64 = 1000;
const MAX_TEXT_CHARS: usize = 20_000;

/// Tags whose entire subtree should be skipped during text extraction.
const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "head", "meta", "link", "nav", "footer", "header",
];

/// Block-level tags that should be separated with newlines.
const BLOCK_TAGS: &[&str] = &[
    "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "br", "tr", "td", "th",
    "article", "section", "blockquote", "pre",
];

/// CSS selectors tried in order to find the main content region.
const CONTENT_SELECTORS: &[&str] = &[
    "article", "main", "[role=\"main\"]", "#content", ".content", ".post", ".article", "body",
];

pub struct FetchContent {
    client: reqwest::Client,
    rate_limiter: RateLimiter,
}

impl FetchContent {
    pub fn new() -> Self {
        FetchContent {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                .build()
                .expect("failed to build reqwest client"),
            rate_limiter: RateLimiter::new(FETCH_RATE_LIMIT_MS),
        }
    }
}

impl Default for FetchContent {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_text(el: ElementRef) -> String {
    let tag = el.value().name();

    if SKIP_TAGS.contains(&tag) {
        return String::new();
    }

    let is_block = BLOCK_TAGS.contains(&tag);
    let mut parts: Vec<String> = Vec::new();

    for child in el.children() {
        if let Some(text_node) = child.value().as_text() {
            let t = text_node.trim();
            if !t.is_empty() {
                parts.push(t.to_string());
            }
        } else if let Some(child_el) = ElementRef::wrap(child) {
            let child_text = extract_text(child_el);
            if !child_text.is_empty() {
                parts.push(child_text);
            }
        }
    }

    if is_block {
        let joined = parts.join(" ");
        format!("\n{}\n", joined.trim())
    } else {
        parts.join(" ")
    }
}

fn html_to_text(html: &str) -> String {
    let doc = Html::parse_document(html);

    for selector_str in CONTENT_SELECTORS {
        let Ok(selector) = Selector::parse(selector_str) else {
            continue;
        };
        if let Some(el) = doc.select(&selector).next() {
            let text = extract_text(el);
            let clean = collapse_whitespace(&text);
            if clean.len() > 200 {
                return clean;
            }
        }
    }

    String::new()
}

fn collapse_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_newline = false;
    let mut blank_lines = 0usize;

    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if prev_newline {
                blank_lines += 1;
                if blank_lines <= 1 {
                    result.push('\n');
                }
            }
            prev_newline = true;
        } else {
            blank_lines = 0;
            prev_newline = false;
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    result.trim().to_string()
}

#[async_trait]
impl Tool for FetchContent {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "fetch_content",
            description: "Fetch a URL and extract its readable text content. \
                HTML is parsed and converted to clean text suitable for LLM consumption. \
                Use this after web_search to read the full content of a result.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch and extract text from"
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

        self.rate_limiter.acquire().await;

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

        let (text, truncated) = if content_type.contains("text/html") {
            let mut extracted = html_to_text(&body);
            let trunc = extracted.len() > MAX_TEXT_CHARS;
            if trunc {
                extracted.truncate(MAX_TEXT_CHARS);
                extracted.push_str("\n[truncated]");
            }
            (extracted, trunc)
        } else {
            let trunc = body.len() > MAX_TEXT_CHARS;
            let mut text = body;
            if trunc {
                text.truncate(MAX_TEXT_CHARS);
                text.push_str("\n[truncated]");
            }
            (text, trunc)
        };

        Ok(ToolCallResult::success(json!({
            "url": url,
            "status": status,
            "content_type": content_type,
            "text": text,
            "truncated": truncated,
        })
        .to_string()))
    }
}
