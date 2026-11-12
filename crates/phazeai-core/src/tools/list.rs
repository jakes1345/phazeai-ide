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

        // A relative path is read from the open project (see `resolve_relative`).
        let root = crate::tools::sandbox::resolve_relative(std::path::Path::new(path));
        let mut builder = WalkBuilder::new(&root);
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
            let relative = entry_path.strip_prefix(&root).unwrap_or(entry_path);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::sandbox::{set_workspace_root, tests::SERIAL, workspace_root};

    #[test]
    fn a_relative_path_lists_the_open_project_not_the_process_directory() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/only_here.rs"), "").unwrap();

        let _lock = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let prev = workspace_root();
        set_workspace_root(Some(root.clone()));
        // A runtime of its own, so the lock is not held across an `.await`.
        let listed = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(ListFilesTool.execute(serde_json::json!({ "path": "src" })));
        set_workspace_root(prev);

        let listed = listed.unwrap();
        let names: Vec<&str> = listed["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["name"].as_str())
            .collect();
        assert_eq!(names, ["only_here.rs"], "{listed}");
    }
}
