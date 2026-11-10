use crate::tools::traits::{Tool, ToolResult};
use ignore::WalkBuilder;
use serde_json::Value;

/// Entries a listing returns at most; the result says when it stopped there.
const MAX_ENTRIES: usize = 1000;

pub struct ListFilesTool;

#[async_trait::async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files and directories. Respects .gitignore. Supports recursive listing."
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list (default: current directory)"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to list recursively (default: false)",
                    "default": false
                }
            }
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        let recursive = params
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut files = Vec::new();

        let max_depth = if recursive { None } else { Some(1) };

        let mut builder = WalkBuilder::new(path);
        builder.hidden(false).git_ignore(true).git_global(true);
        let policy = crate::privacy::active();
        builder.filter_entry(move |e| {
            e.depth() == 0
                || (!crate::tools::render::is_vcs_dir(e)
                    && !policy.is_denied(&crate::privacy::absolutize(e.path())))
        });

        if let Some(depth) = max_depth {
            builder.max_depth(Some(depth));
        }

        for entry in builder.build().flatten() {
            // Skip the root directory itself
            if entry.depth() == 0 {
                continue;
            }

            let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
            let entry_path = entry.path();
            let relative = entry_path.strip_prefix(path).unwrap_or(entry_path);

            files.push(serde_json::json!({
                "name": relative.to_string_lossy(),
                "type": if is_dir { "directory" } else { "file" },
            }));

            if files.len() > MAX_ENTRIES {
                break;
            }
        }

        // One entry past the cap was kept only to tell that there are more.
        let truncated = files.len() > MAX_ENTRIES;
        files.truncate(MAX_ENTRIES);
        let mut out = serde_json::json!({
            "files": files,
            "path": path,
            "count": files.len(),
        });
        if truncated {
            out["truncated"] = Value::Bool(true);
            out["limit"] = Value::from(MAX_ENTRIES);
        }
        Ok(out)
    }
}
