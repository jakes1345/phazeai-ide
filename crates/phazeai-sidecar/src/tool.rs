use crate::SidecarClient;
use phazeai_core::PhazeError;
use phazeai_core::{Tool, ToolResult};
use serde_json::Value;
use std::sync::Arc;

/// Keyword-based code search tool backed by the Python sidecar's TF-IDF index.
/// Falls back to a helpful error if the sidecar is unavailable.
pub struct CodeSearchTool {
    client: Arc<SidecarClient>,
}

impl CodeSearchTool {
    pub fn new(client: Arc<SidecarClient>) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Tool for CodeSearchTool {
    fn name(&self) -> &str {
        "code_search"
    }

    fn description(&self) -> &str {
        "Search the codebase using keyword matching (TF-IDF ranking). \
         Use this when exact grep patterns are too rigid and you need concept-adjacent matches. \
         Returns ranked file snippets by textual relevance."
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

        let top_k = params
            .get("top_k")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(20) as usize;

        let result = match self.client.search_code(query, top_k).await {
            Ok(result) => result,
            Err(e) if e.contains("Index not built") => {
                self.client
                    .build_index(&[".".to_string()])
                    .await
                    .map_err(|idx_err| {
                        PhazeError::tool(
                            "code_search",
                            format!("Failed to build search index automatically: {idx_err}"),
                        )
                    })?;
                self.client
                    .search_code(query, top_k)
                    .await
                    .map_err(|retry_err| {
                        PhazeError::tool(
                            "code_search",
                            format!("Sidecar search failed after auto-indexing: {retry_err}"),
                        )
                    })?
            }
            Err(e) => {
                return Err(PhazeError::tool(
                    "code_search",
                    format!("Sidecar error: {e}"),
                ));
            }
        };

        Ok(result)
    }
}

/// Tool to build the sidecar TF-IDF search index for the project.
pub struct BuildIndexTool {
    client: Arc<SidecarClient>,
}

impl BuildIndexTool {
    pub fn new(client: Arc<SidecarClient>) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Tool for BuildIndexTool {
    fn name(&self) -> &str {
        "build_search_index"
    }

    fn description(&self) -> &str {
        "Build or rebuild the sidecar TF-IDF search index for the project. \
         Call this before using code_search if search returns no results, \
         or after significant code changes."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File or directory paths to index (default: current directory)"
                }
            }
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let paths: Vec<String> = params
            .get("paths")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| vec![".".to_string()]);

        let result =
            self.client.build_index(&paths).await.map_err(|e| {
                PhazeError::tool("build_search_index", format!("Sidecar error: {e}"))
            })?;

        Ok(result)
    }
}
