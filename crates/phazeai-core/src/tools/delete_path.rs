use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::Path;

/// Critical paths that must never be deleted
const PROTECTED_PATHS: &[&str] = &[
    "/", "/home", "/usr", "/bin", "/sbin", "/etc", "/var", "/tmp", "/boot", "/dev", "/proc",
    "/sys", "/lib", "/lib64", "/opt",
];

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

        let path = Path::new(path_str);

        if !path.exists() {
            return Err(PhazeError::tool(
                "delete_path",
                format!("Path does not exist: {path_str}"),
            ));
        }

        // Safety: refuse to delete critical paths
        let canonical = path
            .canonicalize()
            .map_err(|e| PhazeError::tool("delete_path", format!("Cannot resolve path: {e}")))?;
        let canonical_str = canonical.to_string_lossy();

        for protected in PROTECTED_PATHS {
            if canonical_str.as_ref() == *protected {
                return Err(PhazeError::tool(
                    "delete_path",
                    format!("REFUSED: Cannot delete protected path: {protected}"),
                ));
            }
        }

        // Also protect home directory itself
        if let Some(home) = dirs::home_dir() {
            if canonical == home {
                return Err(PhazeError::tool(
                    "delete_path",
                    "REFUSED: Cannot delete home directory",
                ));
            }
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
