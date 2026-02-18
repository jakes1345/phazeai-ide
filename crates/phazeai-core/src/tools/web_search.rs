use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;

pub struct WebSearchTool;

#[async_trait::async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the internet using DuckDuckGo. Returns a list of results with titles, URLs, and snippets. Use this to find documentation, solutions, or information online."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("web_search", "Missing required parameter: query"))?;

        let max_results = params
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("PhazeAI/1.0")
            .build()
            .map_err(|e| PhazeError::tool("web_search", format!("HTTP client error: {e}")))?;

        // Use DuckDuckGo HTML lite (no API key required)
        let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding::encode(query));

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| PhazeError::tool("web_search", format!("Search request failed: {e}")))?;

        let html = response
            .text()
            .await
            .map_err(|e| PhazeError::tool("web_search", format!("Failed to read response: {e}")))?;

        // Parse results from DuckDuckGo HTML
        let results = parse_ddg_results(&html, max_results);

        Ok(serde_json::json!({
            "query": query,
            "results": results,
            "count": results.len(),
        }))
    }
}

fn parse_ddg_results(html: &str, max_results: usize) -> Vec<Value> {
    let mut results = Vec::new();

    // DuckDuckGo HTML lite uses class="result__a" for links and class="result__snippet" for snippets
    for segment in html.split("class=\"result__a\"").skip(1) {
        if results.len() >= max_results {
            break;
        }

        let url = extract_between(segment, "href=\"", "\"").unwrap_or_default();
        let title = extract_between(segment, ">", "</a>").unwrap_or_default();
        let snippet = if let Some(snip_start) = segment.find("class=\"result__snippet\"") {
            let snip_segment = &segment[snip_start..];
            extract_between(snip_segment, ">", "</")
                .unwrap_or_default()
                .trim()
                .to_string()
        } else {
            String::new()
        };

        // Skip internal DDG links
        if url.is_empty() || url.starts_with('/') {
            continue;
        }

        // Clean up the URL (DDG wraps them in a redirect)
        let clean_url = if url.contains("uddg=") {
            urlencoding::decode(
                url.split("uddg=").nth(1).unwrap_or(&url).split('&').next().unwrap_or(&url)
            )
            .unwrap_or_else(|_| url.clone().into())
            .to_string()
        } else {
            url.clone()
        };

        // Strip HTML tags from title and snippet
        let clean_title = strip_html_tags(&title);
        let clean_snippet = strip_html_tags(&snippet);

        results.push(serde_json::json!({
            "title": clean_title,
            "url": clean_url,
            "snippet": clean_snippet,
        }));
    }

    results
}

fn extract_between<'a>(text: &'a str, start: &str, end: &str) -> Option<String> {
    let start_idx = text.find(start)? + start.len();
    let remaining = &text[start_idx..];
    let end_idx = remaining.find(end)?;
    Some(remaining[..end_idx].to_string())
}

fn strip_html_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result.trim().to_string()
}
