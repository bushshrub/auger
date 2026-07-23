use agent_tools::JsonSchema;
use agent_tools::Tool;
use agent_tools::ToolCallResult;
use agent_tools::ToolDetails;
use agent_tools::ToolError;
use async_trait::async_trait;
use scraper::ElementRef;
use scraper::Html;
use scraper::Selector;
use serde_json::json;

pub struct WebFetchText {
    client: reqwest::Client,
}

// ~8000 chars ≈ 2000 tokens (English words ~1.3 tokens/word, ~5 chars/word →
// 3.85 chars/token)
const MAX_INLINE: usize = 8_000;

impl WebFetchText {
    pub fn new() -> Self {
        WebFetchText {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (compatible; auger-agent/0.1)")
                .build()
                .expect("failed to build reqwest client"),
        }
    }
}

impl Default for WebFetchText {
    fn default() -> Self {
        Self::new()
    }
}

fn is_noise(tag: &str) -> bool {
    matches!(
        tag,
        "script" | "style" | "noscript" | "head" | "meta" | "link" | "svg" | "canvas"
    )
}

fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
            | "br"
            | "tr"
            | "blockquote"
            | "pre"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "main"
            | "nav"
            | "ul"
            | "ol"
            | "table"
    )
}

fn walk(el: ElementRef<'_>, out: &mut String) {
    let tag = el.value().name();
    if is_noise(tag) {
        return;
    }
    if is_block(tag) && !out.ends_with('\n') {
        out.push('\n');
    }
    for child in el.children() {
        if let Some(child_el) = ElementRef::wrap(child) {
            walk(child_el, out);
        } else if let scraper::Node::Text(t) = child.value() {
            let s = t.trim();
            if !s.is_empty() {
                out.push_str(s);
                out.push(' ');
            }
        }
    }
}

fn html_to_text(html: &str) -> String {
    let doc = Html::parse_document(html);
    let body_sel = Selector::parse("body").unwrap();
    let mut raw = String::new();

    let root = doc
        .select(&body_sel)
        .next()
        .unwrap_or_else(|| doc.root_element());
    walk(root, &mut raw);

    // Collapse runs of blank lines and trim each line
    let mut out = String::with_capacity(raw.len());
    let mut prev_blank = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank {
                out.push('\n');
            }
            prev_blank = true;
        } else {
            out.push_str(trimmed);
            out.push('\n');
            prev_blank = false;
        }
    }
    out.trim().to_owned()
}

#[async_trait]
impl Tool for WebFetchText {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "web_fetch_text".to_string(),
            description: "Fetch a URL and return only the visible text content, stripping all \
                          HTML tags, scripts, and styles. Prefer this over web_fetch when you \
                          want readable prose rather than raw markup. If the extracted text \
                          exceeds the inline limit the full content is saved to a temporary file \
                          and the path is returned."
                .to_string(),
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

        let raw = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("failed to read body: {e}")))?;

        let text = if content_type.contains("text/html") {
            html_to_text(&raw)
        } else {
            raw
        };

        if text.len() <= MAX_INLINE {
            return Ok(ToolCallResult::success(
                json!({
                    "status": status,
                    "content_type": content_type,
                    "text": text,
                })
                .to_string(),
            ));
        }

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let path = std::env::temp_dir().join(format!("web_fetch_text_{nanos}.txt"));
        tokio::fs::write(&path, text.as_bytes())
            .await
            .map_err(|e| ToolError::Execution(format!("failed to write temp file: {e}")))?;

        Ok(ToolCallResult::success(
            json!({
                "status": status,
                "content_type": content_type,
                "text_size_bytes": text.len(),
                "full_response_path": path.to_string_lossy(),
                "note": format!(
                    "Extracted text ({} bytes) exceeds inline limit ({} bytes). \
                    Full content saved to: {}",
                    text.len(),
                    MAX_INLINE,
                    path.display()
                ),
            })
            .to_string(),
        ))
    }
}
