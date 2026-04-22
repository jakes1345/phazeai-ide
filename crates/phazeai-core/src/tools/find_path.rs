use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::Value;

pub struct FindPathTool;

#[async_trait::async_trait]
impl Tool for FindPathTool {
    fn name(&self) -> &str {
        "find_path"
    }

    fn description(&self) -> &str {
        "Find files and directories by name or regex pattern. Walks the directory tree respecting .gitignore. Use this to locate files when you know part of the filename but not the full path."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to match against file/directory names"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search in (default: current directory)"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum directory depth to search (default: unlimited)"
                },
                "type": {
                    "type": "string",
                    "enum": ["file", "directory", "any"],
                    "description": "Filter by type (default: 'any')"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let pattern = params
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("find_path", "Missing required parameter: pattern"))?;

        let base_path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        let max_depth = params
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|d| d as usize);

        let type_filter = params.get("type").and_then(|v| v.as_str()).unwrap_or("any");

        let regex = Regex::new(pattern)
            .map_err(|e| PhazeError::tool("find_path", format!("Invalid regex: {e}")))?;

        let mut builder = WalkBuilder::new(base_path);
        builder.hidden(false).git_ignore(true).git_global(true);

        if let Some(depth) = max_depth {
            builder.max_depth(Some(depth));
        }

        let mut matches = Vec::new();
        for entry in builder.build().flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();

            // Type filtering
            if let Some(ft) = entry.file_type() {
                match type_filter {
                    "file" if !ft.is_file() => continue,
                    "directory" if !ft.is_dir() => continue,
                    _ => {}
                }
            }

            if regex.is_match(&name) {
                matches.push(serde_json::json!({
                    "path": path.to_string_lossy(),
                    "name": name,
                    "is_dir": entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false),
                }));
            }

            if matches.len() >= 500 {
                break;
            }
        }

        Ok(serde_json::json!({
            "matches": matches,
            "count": matches.len(),
            "pattern": pattern,
            "truncated": matches.len() >= 500,
        }))
    }
}
