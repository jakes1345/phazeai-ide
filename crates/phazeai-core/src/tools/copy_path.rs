use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::Path;

pub struct CopyPathTool;

#[async_trait::async_trait]
impl Tool for CopyPathTool {
    fn name(&self) -> &str {
        "copy_path"
    }

    fn description(&self) -> &str {
        "Copy a file or directory to a new location. For directories, copies recursively."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Source path to copy from"
                },
                "destination": {
                    "type": "string",
                    "description": "Destination path to copy to"
                }
            },
            "required": ["source", "destination"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let source = params
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("copy_path", "Missing required parameter: source"))?;

        let destination = params
            .get("destination")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("copy_path", "Missing required parameter: destination"))?;

        let source_path = Path::new(source);
        let dest_path = Path::new(destination);

        if !source_path.exists() {
            return Err(PhazeError::tool("copy_path", format!("Source path does not exist: {source}")));
        }

        if source_path.is_file() {
            // Ensure parent directory exists
            if let Some(parent) = dest_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| PhazeError::tool("copy_path", format!("Failed to create parent dirs: {e}")))?;
            }

            let bytes = tokio::fs::copy(source_path, dest_path)
                .await
                .map_err(|e| PhazeError::tool("copy_path", format!("Failed to copy file: {e}")))?;

            Ok(serde_json::json!({
                "success": true,
                "type": "file",
                "source": source,
                "destination": destination,
                "bytes_copied": bytes,
            }))
        } else if source_path.is_dir() {
            let count = copy_dir_recursive(source_path, dest_path).await
                .map_err(|e| PhazeError::tool("copy_path", format!("Failed to copy directory: {e}")))?;

            Ok(serde_json::json!({
                "success": true,
                "type": "directory",
                "source": source,
                "destination": destination,
                "files_copied": count,
            }))
        } else {
            Err(PhazeError::tool("copy_path", format!("Unsupported file type: {source}")))
        }
    }
}

pub async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<usize, std::io::Error> {
    tokio::fs::create_dir_all(dst).await?;
    let mut count = 0;

    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            count += Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path).await?;
            count += 1;
        }
    }

    Ok(count)
}
