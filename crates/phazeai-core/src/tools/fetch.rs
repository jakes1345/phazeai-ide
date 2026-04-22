use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::collections::HashMap;

const MAX_RESPONSE_BYTES: usize = 50_000;

pub struct FetchTool;

#[async_trait::async_trait]
impl Tool for FetchTool {
    fn name(&self) -> &str {
        "fetch"
    }

    fn description(&self) -> &str {
        "Make an HTTP request to a URL. Returns the response status, headers, and body. Use this to fetch web pages, API responses, or download content. Body is truncated to 50KB."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD"],
                    "description": "HTTP method (default: GET)"
                },
                "headers": {
                    "type": "object",
                    "description": "Optional HTTP headers as key-value pairs",
                    "additionalProperties": { "type": "string" }
                },
                "body": {
                    "type": "string",
                    "description": "Optional request body (for POST/PUT/PATCH)"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Request timeout in seconds (default: 30)"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("fetch", "Missing required parameter: url"))?;

        let method = params
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET");

        let timeout_secs = params
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .user_agent("PhazeAI/1.0")
            .build()
            .map_err(|e| PhazeError::tool("fetch", format!("Failed to create HTTP client: {e}")))?;

        let mut request = match method.to_uppercase().as_str() {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "DELETE" => client.delete(url),
            "PATCH" => client.patch(url),
            "HEAD" => client.head(url),
            _ => {
                return Err(PhazeError::tool(
                    "fetch",
                    format!("Unsupported method: {method}"),
                ))
            }
        };

        // Add custom headers
        if let Some(headers) = params.get("headers").and_then(|v| v.as_object()) {
            for (key, value) in headers {
                if let Some(val) = value.as_str() {
                    request = request.header(key.as_str(), val);
                }
            }
        }

        // Add body
        if let Some(body) = params.get("body").and_then(|v| v.as_str()) {
            request = request.body(body.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| PhazeError::tool("fetch", format!("Request failed: {e}")))?;

        let status = response.status().as_u16();
        let status_text = response.status().canonical_reason().unwrap_or("Unknown");

        let response_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
            .collect();

        let content_type = response_headers
            .get("content-type")
            .cloned()
            .unwrap_or_default();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| PhazeError::tool("fetch", format!("Failed to read response: {e}")))?;

        let truncated = bytes.len() > MAX_RESPONSE_BYTES;
        let body_bytes = if truncated {
            &bytes[..MAX_RESPONSE_BYTES]
        } else {
            &bytes[..]
        };

        let body = String::from_utf8_lossy(body_bytes).to_string();

        Ok(serde_json::json!({
            "status": status,
            "status_text": status_text,
            "headers": response_headers,
            "content_type": content_type,
            "body": body,
            "body_bytes": bytes.len(),
            "truncated": truncated,
        }))
    }
}
