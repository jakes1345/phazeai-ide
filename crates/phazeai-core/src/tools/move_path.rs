use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::Path;

pub struct MovePathTool;

#[async_trait::async_trait]
impl Tool for MovePathTool {
    fn name(&self) -> &str {
        "move_path"
    }

    fn description(&self) -> &str {
        "Move or rename a file or directory. Works across filesystems by falling back to copy+delete."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Source path to move"
                },
                "destination": {
                    "type": "string",
                    "description": "Destination path"
                }
            },
            "required": ["source", "destination"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let source = params
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("move_path", "Missing required parameter: source"))?;

        let destination = params
            .get("destination")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                PhazeError::tool("move_path", "Missing required parameter: destination")
            })?;

        let source_path = Path::new(source);
        let dest_path = Path::new(destination);

        if !source_path.exists() {
            return Err(PhazeError::tool(
                "move_path",
                format!("Source path does not exist: {source}"),
            ));
        }

        // Ensure parent directory exists
        if let Some(parent) = dest_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                PhazeError::tool("move_path", format!("Failed to create parent dirs: {e}"))
            })?;
        }

        // Try rename first (fast, same filesystem)
        match tokio::fs::rename(source_path, dest_path).await {
            Ok(()) => Ok(serde_json::json!({
                "success": true,
                "source": source,
                "destination": destination,
                "method": "rename",
            })),
            Err(_) => {
                // Fallback: copy then delete (cross-filesystem)
                if source_path.is_file() {
                    tokio::fs::copy(source_path, dest_path).await.map_err(|e| {
                        PhazeError::tool("move_path", format!("Failed to copy: {e}"))
                    })?;
                    tokio::fs::remove_file(source_path).await.map_err(|e| {
                        PhazeError::tool("move_path", format!("Failed to remove source: {e}"))
                    })?;
                } else {
                    // For directories, use recursive copy then remove
                    super::copy_path::copy_dir_recursive(source_path, dest_path)
                        .await
                        .map_err(|e| {
                            PhazeError::tool("move_path", format!("Failed to copy dir: {e}"))
                        })?;
                    tokio::fs::remove_dir_all(source_path).await.map_err(|e| {
                        PhazeError::tool("move_path", format!("Failed to remove source dir: {e}"))
                    })?;
                }

                Ok(serde_json::json!({
                    "success": true,
                    "source": source,
                    "destination": destination,
                    "method": "copy_and_delete",
                }))
            }
        }
    }
}
