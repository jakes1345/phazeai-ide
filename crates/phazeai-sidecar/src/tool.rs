use crate::SidecarClient;
use phazeai_core::PhazeError;
use phazeai_core::{Tool, ToolResult};
use serde_json::Value;
use std::sync::Arc;

/// Semantic search tool powered by the Python sidecar's embedding index.
/// Falls back to a helpful error if the sidecar is unavailable.
pub struct SemanticSearchTool {
    client: Arc<SidecarClient>,
}

impl SemanticSearchTool {
    pub fn new(client: Arc<SidecarClient>) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Tool for SemanticSearchTool {
    fn name(&self) -> &str {
        "semantic_search"
    }

    fn description(&self) -> &str {
        "Search the codebase using natural language semantic search powered by embeddings. \
         Use this when grep is insufficient - e.g. finding code by concept rather than exact text. \
         Returns ranked file snippets matching the query by meaning."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language description of what to search for"
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
            .ok_or_else(|| {
                PhazeError::tool("semantic_search", "Missing required parameter: query")
            })?;

        let top_k = params
            .get("top_k")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(20) as usize;

        let result = self
            .client
            .search_embeddings(query, top_k)
            .await
            .map_err(|e| PhazeError::tool("semantic_search", format!("Sidecar error: {e}")))?;

        Ok(result)
    }
}

/// Tool to build the semantic search index for the project.
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
        "Build or rebuild the semantic search index for the project. \
         Call this before using semantic_search if search returns no results, \
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
