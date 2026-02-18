use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::Path;

pub struct OpenTool;

#[async_trait::async_trait]
impl Tool for OpenTool {
    fn name(&self) -> &str {
        "open"
    }

    fn description(&self) -> &str {
        "Open a file or URL with the system's default application. For files, this opens them in the default editor or viewer. For URLs, opens in the default browser."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path or URL to open"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let path_str = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("open", "Missing required parameter: path"))?;

        // Check if it's a URL or file path
        let is_url = path_str.starts_with("http://") || path_str.starts_with("https://");

        if !is_url {
            let path = Path::new(path_str);
            if !path.exists() {
                return Err(PhazeError::tool("open", format!("Path does not exist: {path_str}")));
            }
        }

        // Use xdg-open on Linux
        let output = tokio::process::Command::new("xdg-open")
            .arg(path_str)
            .output()
            .await
            .map_err(|e| PhazeError::tool("open", format!("Failed to open: {e}")))?;

        if output.status.success() {
            Ok(serde_json::json!({
                "success": true,
                "path": path_str,
                "type": if is_url { "url" } else { "file" },
            }))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(PhazeError::tool("open", format!("Failed to open {path_str}: {stderr}")))
        }
    }
}
