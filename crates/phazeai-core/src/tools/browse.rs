/// Web browsing tool — fetches a URL and converts HTML to clean readable text
/// ([`crate::tools::readable`]), optionally only the part a selector picks.
use crate::error::PhazeError;
use crate::tools::readable;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;

const MAX_CONTENT_CHARS: usize = 30_000;

pub struct BrowseTool;

#[async_trait::async_trait]
impl Tool for BrowseTool {
    fn name(&self) -> &str {
        "browse"
    }

    fn summary(&self) -> Option<&str> {
        Some("A web page, or one part of it, as text")
    }

    fn description(&self) -> &str {
        "Browse a web page and extract its readable content as clean text. Strips HTML, ads, navigation, and scripts. Returns the main text content of the page. Use this to read documentation, articles, README files, or any web page."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL of the web page to browse"
                },
                "selector": {
                    "type": "string",
                    "description": "Optional element to keep, as tag, .class, #id, tag.class or tag#id (e.g. 'main', 'article', '.content'); the whole page if nothing matches"
                },
                "include_links": {
                    "type": "boolean",
                    "description": "Whether to include hyperlinks in [text](url) format (default: false)"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("browse", "Missing required parameter: url"))?;

        let include_links = params
            .get("include_links")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| PhazeError::tool("browse", format!("HTTP client error: {e}")))?;

        let response = client
            .get(url)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| PhazeError::tool("browse", format!("Failed to fetch page: {e}")))?;

        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let html = response
            .text()
            .await
            .map_err(|e| PhazeError::tool("browse", format!("Failed to read page: {e}")))?;

        let selector = params
            .get("selector")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());
        let base = reqwest::Url::parse(&final_url).ok();
        let page = if readable::is_html(&content_type) || content_type.is_empty() {
            readable::to_text(
                &html,
                &readable::Options {
                    links: include_links,
                    base: base.as_ref(),
                    selector,
                },
            )
        } else {
            readable::Page {
                title: String::new(),
                text: html,
                selected: false,
            }
        };
        let note = selector
            .filter(|_| !page.selected)
            .map(|s| format!("nothing matched {s:?}; showing the whole page"));
        let title = page.title;
        let text = page.text;

        // Truncate if too long
        let truncated = text.len() > MAX_CONTENT_CHARS;
        let content = if truncated {
            format!(
                "{}...\n\n[Content truncated at {} chars]",
                crate::text::truncate_bytes(&text, MAX_CONTENT_CHARS),
                MAX_CONTENT_CHARS
            )
        } else {
            text
        };

        Ok(serde_json::json!({
            "url": final_url,
            "title": title,
            "status": status,
            "content_type": content_type,
            "content": content,
            "content_length": content.len(),
            "truncated": truncated,
            "note": note,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_its_selector_as_what_it_supports() {
        let schema = BrowseTool.parameters_schema();
        let desc = schema["properties"]["selector"]["description"]
            .as_str()
            .unwrap();
        assert!(desc.contains(".class"), "{desc}");
    }
}
