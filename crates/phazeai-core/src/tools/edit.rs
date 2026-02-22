use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;

/// Surgical text replacement tool - finds old_text and replaces with new_text.
pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Make a surgical edit to a file by replacing old_text with new_text. The old_text must be unique in the file."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "The text to replace old_text with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)",
                    "default": false
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("edit_file", "Missing required parameter: path"))?;

        let old_text = params
            .get("old_text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("edit_file", "Missing required parameter: old_text"))?;

        let new_text = params
            .get("new_text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("edit_file", "Missing required parameter: new_text"))?;

        let replace_all = params
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            PhazeError::tool("edit_file", format!("Failed to read '{}': {}", path, e))
        })?;

        let match_count = content.matches(old_text).count();

        if match_count == 0 {
            return Err(PhazeError::tool(
                "edit_file",
                format!("old_text not found in '{}'", path),
            ));
        }

        if !replace_all && match_count > 1 {
            return Err(PhazeError::tool(
                "edit_file",
                format!(
                    "old_text matches {} times in '{}'. Use replace_all=true or provide more context.",
                    match_count, path
                ),
            ));
        }

        let new_content = if replace_all {
            content.replace(old_text, new_text)
        } else {
            content.replacen(old_text, new_text, 1)
        };

        tokio::fs::write(path, &new_content).await.map_err(|e| {
            PhazeError::tool("edit_file", format!("Failed to write '{}': {}", path, e))
        })?;

        Ok(serde_json::json!({
            "path": path,
            "replacements": if replace_all { match_count } else { 1 },
            "success": true,
        }))
    }
}
