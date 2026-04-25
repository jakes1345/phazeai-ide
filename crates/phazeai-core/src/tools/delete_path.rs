use crate::error::PhazeError;
use crate::tools::sandbox;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;

pub struct DeletePathTool;

#[async_trait::async_trait]
impl Tool for DeletePathTool {
    fn name(&self) -> &str {
        "delete_path"
    }

    fn description(&self) -> &str {
        "Delete a file or directory. Refuses to delete critical system paths. For directories, deletes recursively."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to delete"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let path_str = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("delete_path", "Missing required parameter: path"))?;

        // sandbox::resolve_within_workspace handles: empty input, system-protected
        // paths (extended list incl. /srv /mnt /media /root), home directory,
        // workspace boundary, and ../ symlink escapes.
        let resolved = sandbox::resolve_within_workspace("delete_path", path_str)?;
        let path = resolved.as_path();

        if !path.exists() {
            return Err(PhazeError::tool(
                "delete_path",
                format!("Path does not exist: {path_str}"),
            ));
        }

        if path.is_file() || path.is_symlink() {
            tokio::fs::remove_file(path).await.map_err(|e| {
                PhazeError::tool("delete_path", format!("Failed to delete file: {e}"))
            })?;

            Ok(serde_json::json!({
                "success": true,
                "type": "file",
                "deleted": path_str,
            }))
        } else if path.is_dir() {
            tokio::fs::remove_dir_all(path).await.map_err(|e| {
                PhazeError::tool("delete_path", format!("Failed to delete directory: {e}"))
            })?;

            Ok(serde_json::json!({
                "success": true,
                "type": "directory",
                "deleted": path_str,
            }))
        } else {
            Err(PhazeError::tool(
                "delete_path",
                format!("Unsupported file type: {path_str}"),
            ))
        }
    }
}
