use async_trait::async_trait;
use scraper::{Html, Selector};
use serde_json::json;

use crate::{JsonSchema, Tool, ToolCallResult, ToolDetails, ToolError};
use crate::rate_limiter::RateLimiter;

/// Minimum milliseconds between DuckDuckGo search requests.
const SEARCH_RATE_LIMIT_MS: u64 = 2000;
const MAX_RESULTS: usize = 10;
const DDG_URL: &str = "https://html.duckduckgo.com/html/";

pub struct WebSearch {
    client: reqwest::Client,
    rate_limiter: RateLimiter,
}

impl WebSearch {
    pub fn new() -> Self {
        WebSearch {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                .build()
                .expect("failed to build reqwest client"),
            rate_limiter: RateLimiter::new(SEARCH_RATE_LIMIT_MS),
        }
    }
}

impl Default for WebSearch {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_url(href: &str) -> String {
    // DDG redirect links: /l/?kh=-1&uddg=ENCODED_URL
    if href.starts_with("http") {
        return href.to_string();
    }
    let fake_base = format!("https://duckduckgo.com{href}");
    if let Ok(parsed) = reqwest::Url::parse(&fake_base) {
        if let Some((_, v)) = parsed.query_pairs().find(|(k, _)| k == "uddg") {
            return v.into_owned();
        }
    }
    href.to_string()
}

fn parse_results(html: &str, max: usize) -> Vec<serde_json::Value> {
    let doc = Html::parse_document(html);

    let result_sel = Selector::parse("div.result:not(.result--more):not(.result--ad)").unwrap();
    let title_sel = Selector::parse("a.result__a").unwrap();
    let snippet_sel = Selector::parse("a.result__snippet").unwrap();

    let mut results = Vec::new();

    for result in doc.select(&result_sel).take(max) {
        let Some(title_el) = result.select(&title_sel).next() else {
            continue;
        };
        let title = title_el.text().collect::<String>().trim().to_string();
        let href = title_el.value().attr("href").unwrap_or("").to_string();
        let url = extract_url(&href);

        if url.is_empty() || title.is_empty() {
            continue;
        }

        let snippet = result
            .select(&snippet_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        results.push(json!({
            "title": title,
            "url": url,
            "snippet": snippet,
        }));
    }

    results
}

#[async_trait]
impl Tool for WebSearch {
    fn details(&self) -> ToolDetails {
        ToolDetails {
            name: "web_search",
            description: "Search the web using DuckDuckGo. Returns titles, URLs, and snippets. \
                Use fetch_content to retrieve the full text of any result URL.",
        }
    }

    fn parameters(&self) -> JsonSchema {
        JsonSchema(json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (1-10, default 10)",
                    "minimum": 1,
                    "maximum": 10
                }
            },
            "required": ["query"]
        }))
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolCallResult, ToolError> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: query".into()))?
            .to_owned();

        let max_results = args["max_results"]
            .as_u64()
            .unwrap_or(MAX_RESULTS as u64)
            .min(MAX_RESULTS as u64) as usize;

        self.rate_limiter.acquire().await;

        let response = self
            .client
            .post(DDG_URL)
            .form(&[("q", &query)])
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("search request failed: {e}")))?;

        let status = response.status().as_u16();
        if status != 200 {
            return Err(ToolError::Execution(format!(
                "DuckDuckGo returned status {status}"
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("failed to read response body: {e}")))?;

        let results = parse_results(&html, max_results);

        if results.is_empty() {
            return Ok(json!({
                "query": query,
                "results": [],
                "note": "No results found. DuckDuckGo may be rate-limiting — try again shortly."
            })
            .to_string()
            .into());
        }

        Ok(json!({
            "query": query,
            "results": results,
        })
        .to_string()
        .into())
    }
}
