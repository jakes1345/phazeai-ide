use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::Path;

pub struct CreateDirectoryTool;

#[async_trait::async_trait]
impl Tool for CreateDirectoryTool {
    fn name(&self) -> &str {
        "create_directory"
    }

    fn description(&self) -> &str {
        "Create a directory (and any missing parent directories). Succeeds silently if the directory already exists."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path of the directory to create"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let path_str = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("create_directory", "Missing required parameter: path"))?;

        let path = Path::new(path_str);
        let already_existed = path.exists();

        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| PhazeError::tool("create_directory", format!("Failed to create directory: {e}")))?;

        Ok(serde_json::json!({
            "success": true,
            "path": path_str,
            "already_existed": already_existed,
        }))
    }
}
