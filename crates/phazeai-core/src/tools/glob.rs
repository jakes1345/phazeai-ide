use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use globset::{Glob as GlobPattern, GlobSetBuilder};
use ignore::WalkBuilder;
use serde_json::Value;

pub struct GlobTool;

#[async_trait::async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern (e.g., '**/*.rs', 'src/**/*.ts'). Respects .gitignore."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files against"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search in (default: current directory)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let pattern = params
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("glob", "Missing required parameter: pattern"))?;

        let base_path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        let glob = GlobPattern::new(pattern)
            .map_err(|e| PhazeError::tool("glob", format!("Invalid glob pattern: {e}")))?;

        let mut glob_set_builder = GlobSetBuilder::new();
        glob_set_builder.add(glob);
        let glob_set = glob_set_builder
            .build()
            .map_err(|e| PhazeError::tool("glob", format!("Failed to build glob set: {e}")))?;

        let mut matches = Vec::new();

        let walker = WalkBuilder::new(base_path)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .build();

        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }

            let entry_path = entry.path();
            let relative = entry_path.strip_prefix(base_path).unwrap_or(entry_path);

            if glob_set.is_match(relative) {
                matches.push(entry_path.to_string_lossy().to_string());
            }

            if matches.len() >= 1000 {
                break;
            }
        }

        Ok(serde_json::json!({
            "matches": matches,
            "count": matches.len(),
            "pattern": pattern,
        }))
    }
}
