/// Download tool — saves files from URLs to the local filesystem.
///
/// Supports downloading binaries, images, documents, archives, etc.
/// with progress tracking and size limits.
use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;

const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024; // 100MB limit

pub struct DownloadTool;

#[async_trait::async_trait]
impl Tool for DownloadTool {
    fn name(&self) -> &str {
        "download"
    }

    fn description(&self) -> &str {
        "Download a file from a URL and save it to the local filesystem. Supports any file type: images, archives, binaries, documents, etc. Maximum 100MB. Use this to download dependencies, assets, or any file the user needs."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL of the file to download"
                },
                "output_path": {
                    "type": "string",
                    "description": "Local path to save the downloaded file"
                },
                "overwrite": {
                    "type": "boolean",
                    "description": "Whether to overwrite if file already exists (default: false)"
                }
            },
            "required": ["url", "output_path"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("download", "Missing required parameter: url"))?;

        let output_path = params
            .get("output_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                PhazeError::tool("download", "Missing required parameter: output_path")
            })?;

        let overwrite = params
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let path = std::path::Path::new(output_path);

        if path.exists() && !overwrite {
            return Err(PhazeError::tool(
                "download",
                format!("File already exists: {output_path}. Set overwrite: true to replace."),
            ));
        }

        // Create parent directories
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                PhazeError::tool(
                    "download",
                    format!("Failed to create directory {}: {e}", parent.display()),
                )
            })?;
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .user_agent("PhazeAI/1.0")
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| PhazeError::tool("download", format!("HTTP client error: {e}")))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| PhazeError::tool("download", format!("Download failed: {e}")))?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(PhazeError::tool(
                "download",
                format!("HTTP {status}: Server returned an error"),
            ));
        }

        // Check content length before downloading
        if let Some(content_length) = response.content_length() {
            if content_length > MAX_DOWNLOAD_BYTES {
                return Err(PhazeError::tool(
                    "download",
                    format!(
                        "File too large: {} MB (limit: {} MB)",
                        content_length / (1024 * 1024),
                        MAX_DOWNLOAD_BYTES / (1024 * 1024)
                    ),
                ));
            }
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| PhazeError::tool("download", format!("Failed to read response: {e}")))?;

        if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
            return Err(PhazeError::tool(
                "download",
                format!("Downloaded data exceeds size limit"),
            ));
        }

        std::fs::write(path, &bytes).map_err(|e| {
            PhazeError::tool(
                "download",
                format!("Failed to write file {output_path}: {e}"),
            )
        })?;

        Ok(serde_json::json!({
            "path": output_path,
            "size_bytes": bytes.len(),
            "size_human": format_bytes(bytes.len() as u64),
            "content_type": content_type,
            "status": status,
            "message": format!("Downloaded {} to {output_path}", format_bytes(bytes.len() as u64)),
        }))
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
