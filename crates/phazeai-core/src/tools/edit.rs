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
        "Make a surgical edit to a file by replacing old_text with new_text. If old_text appears multiple times, you MUST provide 'context' (an exact string of surrounding code) to disambiguate, or set 'replace_all' to true."
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
                },
                "context": {
                    "type": "string",
                    "description": "Surrounding context to disambiguate when old_text appears multiple times"
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

        let context = params
            .get("context")
            .and_then(|v| v.as_str());

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

        let new_content = if replace_all {
            content.replace(old_text, new_text)
        } else if match_count == 1 {
            content.replacen(old_text, new_text, 1)
        } else if let Some(ctx) = context {
            // Find the occurrence of old_text closest to the context region
            let ctx_offset = content.find(ctx).ok_or_else(|| {
                PhazeError::tool(
                    "edit_file",
                    "Provided 'context' string was not found in the file. Please provide an exact match of surrounding code.",
                )
            })?;
            let best_offset = content
                .match_indices(old_text)
                .min_by_key(|(idx, _)| {
                    let idx = *idx;
                    ctx_offset.abs_diff(idx)
                })
                .map(|(idx, _)| idx)
                .ok_or_else(|| PhazeError::tool("edit_file", "old_text not found"))?;
            format!(
                "{}{}{}",
                &content[..best_offset],
                new_text,
                &content[best_offset + old_text.len()..]
            )
        } else {
            return Err(PhazeError::tool(
                "edit_file",
                format!(
                    "old_text matches {} times in '{}'. Use 'replace_all: true' or provide a 'context' string (surrounding code) to disambiguate.",
                    match_count, path
                ),
            ));
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
