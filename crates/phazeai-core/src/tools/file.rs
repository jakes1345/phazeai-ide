use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::Path;

pub struct ReadFileTool;

#[async_trait::async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Supports optional line range with offset and limit parameters."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("read_file", "Missing required parameter: path"))?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| PhazeError::tool("read_file", format!("Failed to read '{}': {}", path, e)))?;

        let offset = params
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|v| v.saturating_sub(1) as usize)
            .unwrap_or(0);

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let selected: Vec<String> = lines
            .into_iter()
            .skip(offset)
            .take(limit.unwrap_or(usize::MAX))
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", offset + i + 1, line))
            .collect();

        Ok(serde_json::json!({
            "content": selected.join("\n"),
            "path": path,
            "total_lines": total_lines,
            "lines_shown": selected.len(),
        }))
    }
}

pub struct WriteFileTool;

#[async_trait::async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("write_file", "Missing required parameter: path"))?;

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                PhazeError::tool("write_file", "Missing required parameter: content")
            })?;

        if let Some(parent) = Path::new(path).parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    PhazeError::tool(
                        "write_file",
                        format!("Failed to create directory '{}': {}", parent.display(), e),
                    )
                })?;
            }
        }

        tokio::fs::write(path, content).await.map_err(|e| {
            PhazeError::tool("write_file", format!("Failed to write '{}': {}", path, e))
        })?;

        Ok(serde_json::json!({
            "path": path,
            "bytes_written": content.len(),
            "success": true,
        }))
    }
}
