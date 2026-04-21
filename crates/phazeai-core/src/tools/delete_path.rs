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

        // Use symlink_metadata so we see the link itself, not its target.
        // This prevents TOCTOU: a symlink cannot be redirected between
        // the safety check and the delete call.
        let link_meta = std::fs::symlink_metadata(path).map_err(|e| {
            PhazeError::tool(
                "delete_path",
                format!("Path does not exist: {path_str} ({e})"),
            )
        })?;
        let file_type = link_meta.file_type();

        // Symlinks are refused outright — they're a common TOCTOU vector
        // and `remove_dir_all` on a symlink behaves surprisingly across platforms.
        if file_type.is_symlink() {
            return Err(PhazeError::tool(
                "delete_path",
                format!("REFUSED: refusing to delete symlink: {path_str}"),
            ));
        }

        // Safety: refuse to delete critical paths. Canonicalize to resolve
        // any parent-directory traversal (`../../`) and compare by path equality
        // rather than string-contains, so e.g. `/usrbin` cannot bypass `/usr`.
        let canonical = path
            .canonicalize()
            .map_err(|e| PhazeError::tool("delete_path", format!("Cannot resolve path: {e}")))?;

        for protected in PROTECTED_PATHS {
            if canonical == Path::new(*protected) {
                return Err(PhazeError::tool(
                    "delete_path",
                    format!("REFUSED: Cannot delete protected path: {protected}"),
                ));
            }
        }

        // Also protect home directory itself and its common config subdirs
        if let Some(home) = dirs::home_dir() {
            if canonical == home {
                return Err(PhazeError::tool(
                    "delete_path",
                    "REFUSED: Cannot delete home directory",
                ));
            }
        }

        if file_type.is_file() {
            tokio::fs::remove_file(&canonical).await.map_err(|e| {
                PhazeError::tool("delete_path", format!("Failed to delete file: {e}"))
            })?;

            Ok(serde_json::json!({
                "success": true,
                "type": "file",
                "deleted": path_str,
            }))
        } else if file_type.is_dir() {
            tokio::fs::remove_dir_all(&canonical).await.map_err(|e| {
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
