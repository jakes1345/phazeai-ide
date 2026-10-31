//! The agent-facing tools: `code_search` and `build_search_index`.

use super::{CodeSearch, SearchError};
use crate::error::PhazeError;
use crate::tools::{Tool, ToolResult};
use serde_json::Value;
use std::path::PathBuf;

const DEFAULT_TOP_K: u64 = 5;
const MAX_TOP_K: u64 = 20;

fn tool_error(tool: &str, e: SearchError) -> PhazeError {
    PhazeError::tool(tool, e.to_string())
}

/// `top_k` as models send it: an integer, sometimes a float or a numeric string.
fn top_k_param(params: &Value) -> usize {
    let n = match params.get("top_k") {
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    };
    n.unwrap_or(DEFAULT_TOP_K).clamp(1, MAX_TOP_K) as usize
}

/// `paths` as an array of strings; a lone string counts as one path. Nothing
/// usable means the whole workspace.
fn paths_param(params: &Value) -> Vec<PathBuf> {
    match params.get("paths") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(PathBuf::from))
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![PathBuf::from(s)],
        _ => Vec::new(),
    }
}

/// Keyword search over the workspace's code, ranked with BM25.
pub struct CodeSearchTool {
    search: CodeSearch,
}

impl CodeSearchTool {
    pub fn new(search: CodeSearch) -> Self {
        Self { search }
    }
}

#[async_trait::async_trait]
impl Tool for CodeSearchTool {
    fn name(&self) -> &str {
        "code_search"
    }

    fn description(&self) -> &str {
        "Search the codebase by keywords. Source files are cut into ~40-line chunks and ranked with \
         BM25 keyword scoring; identifiers are split into words, so 'json config' finds \
         parseJsonConfig and parse_json_config, and file and folder names count as keywords too. \
         It matches the words you give it, not their meaning: use words that appear in the code. \
         Respects .gitignore. Returns up to top_k files, each with its best matching line range \
         (start_line..end_line), a score and a snippet."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords or a short phrase describing what to search for"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Number of results to return (default: 5, max: 20)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("code_search", "Missing required parameter: query"))?;
        let results = self
            .search
            .search(query, top_k_param(&params))
            .await
            .map_err(|e| tool_error("code_search", e))?;
        let mut out = serde_json::to_value(&results)?;
        if results.matches.is_empty() && super::tokenize::query_terms(query).is_empty() {
            out["note"] = Value::from(
                "The query has no searchable words (only stop words, single characters or punctuation).",
            );
        } else if results.index_truncated {
            out["note"] = Value::from(
                "The search index hit its size limits, so some files are not searchable; \
                 results may be incomplete.",
            );
        }
        Ok(out)
    }
}

/// Bring the search index up to date with the files on disk.
pub struct BuildIndexTool {
    search: CodeSearch,
}

impl BuildIndexTool {
    pub fn new(search: CodeSearch) -> Self {
        Self { search }
    }
}

#[async_trait::async_trait]
impl Tool for BuildIndexTool {
    fn name(&self) -> &str {
        "build_search_index"
    }

    fn description(&self) -> &str {
        "Build or refresh the keyword search index used by code_search. It is incremental: only \
         new, changed and deleted files are processed. code_search builds the index on first use \
         and refreshes files it finds out of date, so this is only needed after many files \
         changed at once (a branch switch, a generated directory) or to index a specific path."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File or directory paths to index (default: the whole workspace)"
                }
            }
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let stats = self
            .search
            .build(&paths_param(&params))
            .await
            .map_err(|e| tool_error("build_search_index", e))?;
        let mut out = serde_json::to_value(&stats)?;
        if stats.truncated {
            out["message"] = Value::from(
                "The index limit (maximum files or total size) was reached; the remaining files \
                 were not indexed.",
            );
        }
        Ok(out)
    }
}
