use crate::error::PhazeError;
use crate::tools::sandbox;
use crate::tools::traits::{Tool, ToolResult};
use regex::Regex;
use serde_json::Value;

/// Files larger than this are skipped rather than read into memory.
const MAX_GREP_FILE_BYTES: u64 = 10 * 1024 * 1024;

fn small_enough(path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_GREP_FILE_BYTES)
}

pub struct GrepTool;

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern in files. Respects .gitignore. Returns matching lines with file paths and line numbers."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (default: current directory)"
                },
                "include": {
                    "type": "string",
                    "description": "File glob pattern to include (e.g., '*.rs', '*.py')"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let pattern = params
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("grep", "Missing required parameter: pattern"))?;

        let search_path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        let include_pattern = params.get("include").and_then(|v| v.as_str());

        let regex = Regex::new(pattern)
            .map_err(|e| PhazeError::tool("grep", format!("Invalid regex: {e}")))?;

        let resolved = sandbox::resolve_within_workspace("grep", search_path)?;
        let path = resolved.as_path();
        let mut matches = Vec::new();

        if path.is_file() {
            if !small_enough(path) {
                return Err(PhazeError::tool(
                    "grep",
                    format!("'{search_path}' is larger than 10 MiB; refusing to search it"),
                ));
            }
            if let Ok(content) = tokio::fs::read_to_string(path).await {
                for (line_num, line) in content.lines().enumerate() {
                    if regex.is_match(line) {
                        matches.push(serde_json::json!({
                            "file": search_path,
                            "line": line_num + 1,
                            "content": line,
                        }));
                    }
                }
            }
        } else {
            let mut builder = sandbox::search_walker(path);

            if let Some(glob) = include_pattern {
                let mut types = ignore::types::TypesBuilder::new();
                types.add("custom", glob).ok();
                types.select("custom");
                if let Ok(types) = types.build() {
                    builder.types(types);
                }
            }

            let walker = builder.build();
            for entry in walker.flatten() {
                if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                    continue;
                }

                let file_path = entry.path();
                if !small_enough(file_path) {
                    continue;
                }
                if let Ok(content) = tokio::fs::read_to_string(file_path).await {
                    let file_str = file_path.to_string_lossy();
                    for (line_num, line) in content.lines().enumerate() {
                        if regex.is_match(line) {
                            matches.push(serde_json::json!({
                                "file": file_str.as_ref(),
                                "line": line_num + 1,
                                "content": line,
                            }));
                        }
                    }
                }

                if matches.len() >= 500 {
                    break;
                }
            }
        }

        Ok(serde_json::json!({
            "matches": matches,
            "count": matches.len(),
            "pattern": pattern,
        }))
    }
}
