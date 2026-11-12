use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Hits a search returns at most; the result says when it stopped there.
const MAX_MATCHES: usize = 100;
/// Characters of a matching line that are kept (minified code and data
/// files have lines of many kilobytes).
const MAX_LINE_CHARS: usize = 300;

fn hit(file: &str, line_num: usize, line: &str) -> Value {
    let content = match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    };
    serde_json::json!({ "file": file, "line": line_num + 1, "content": content })
}

pub struct GrepTool;

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search files for a regex. Respects .gitignore. Returns at most 100 matching lines, grouped by file, with line numbers."
    }

    fn is_concurrency_safe(&self) -> bool {
        true
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

        // A relative path is read from the open project (see `resolve_relative`).
        let resolved = crate::tools::sandbox::resolve_relative(Path::new(search_path));
        let path = resolved.as_path();
        let mut matches = Vec::new();
        // Files the privacy policy hides; only the count is reported.
        let skipped = Arc::new(AtomicUsize::new(0));

        if path.is_file() {
            crate::privacy::tool_check("grep", path, search_path)?;
            if let Ok(content) = tokio::fs::read_to_string(path).await {
                for (line_num, line) in content.lines().enumerate() {
                    if regex.is_match(line) {
                        matches.push(hit(search_path, line_num, line));
                        if matches.len() > MAX_MATCHES {
                            break;
                        }
                    }
                }
                if !matches.is_empty() {
                    crate::privacy::audit::note_file(&crate::privacy::absolutize(path));
                }
            }
        } else {
            let mut builder = WalkBuilder::new(path);
            builder.hidden(false).git_ignore(true).git_global(true);
            {
                let policy = crate::privacy::active();
                let skipped = skipped.clone();
                builder.filter_entry(move |e| {
                    if crate::tools::render::is_vcs_dir(e) {
                        return false;
                    }
                    let denied =
                        e.depth() > 0 && policy.is_denied(&crate::privacy::absolutize(e.path()));
                    if denied {
                        skipped.fetch_add(1, Ordering::Relaxed);
                    }
                    !denied
                });
            }

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
                if let Ok(content) = tokio::fs::read_to_string(file_path).await {
                    let file_str = file_path.to_string_lossy();
                    let before = matches.len();
                    for (line_num, line) in content.lines().enumerate() {
                        if regex.is_match(line) {
                            matches.push(hit(&file_str, line_num, line));
                            if matches.len() > MAX_MATCHES {
                                break;
                            }
                        }
                    }
                    if matches.len() > before {
                        crate::privacy::audit::note_file(&crate::privacy::absolutize(file_path));
                    }
                }

                if matches.len() > MAX_MATCHES {
                    break;
                }
            }
        }

        // One hit past the cap was kept only to tell that there are more.
        let truncated = matches.len() > MAX_MATCHES;
        matches.truncate(MAX_MATCHES);
        let mut out = serde_json::json!({
            "matches": matches,
            "count": matches.len(),
            "pattern": pattern,
        });
        if truncated {
            out["truncated"] = Value::Bool(true);
            out["limit"] = Value::from(MAX_MATCHES);
        }
        let skipped = skipped.load(Ordering::Relaxed);
        if skipped > 0 {
            out["privacy_skipped"] = serde_json::json!(skipped);
        }
        Ok(out)
    }
}
