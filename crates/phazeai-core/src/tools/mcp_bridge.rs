/// MCP Tool Bridge — makes MCP server tools available as regular PhazeAI tools.
///
/// Each MCP tool discovered from a connected server gets wrapped in an
/// `McpToolBridge` that implements the `Tool` trait. This means the AI agent
/// can call MCP tools exactly the same way it calls built-in tools like
/// `read_file` or `bash` — no special handling needed.
use crate::error::PhazeError;
use crate::mcp::McpManager;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Wraps a single MCP tool so it implements the `Tool` trait.
/// The agent sees it as a normal tool with a prefixed name like `mcp__serverName__toolName`.
pub struct McpToolBridge {
    /// Name as seen by the agent (e.g., "mcp__github__create_issue")
    tool_name: String,
    /// The original MCP tool name (e.g., "create_issue")
    mcp_tool_name: String,
    /// Which MCP server this tool belongs to
    server_name: String,
    /// Description from the MCP server
    tool_description: String,
    /// JSON schema for the tool's input parameters
    input_schema: Value,
    /// Shared reference to the MCP manager for making calls
    manager: Arc<Mutex<McpManager>>,
}

impl McpToolBridge {
    fn new(
        server_name: String,
        mcp_tool_name: String,
        description: String,
        input_schema: Value,
        manager: Arc<Mutex<McpManager>>,
    ) -> Self {
        let tool_name = format!("mcp__{}__{}", server_name, mcp_tool_name);
        Self {
            tool_name,
            mcp_tool_name,
            server_name,
            tool_description: description,
            input_schema,
            manager,
        }
    }
}

#[async_trait::async_trait]
impl Tool for McpToolBridge {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters_schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let server_name = self.server_name.clone();
        let tool_name = self.mcp_tool_name.clone();
        let manager = self.manager.clone();

        // MCP calls use blocking_recv internally, so run on a blocking thread
        let result = tokio::task::spawn_blocking(move || {
            let mgr = manager
                .lock()
                .map_err(|e| PhazeError::tool("mcp", format!("Manager lock poisoned: {e}")))?;

            mgr.call_tool(&server_name, &tool_name, params)
                .map_err(|e| PhazeError::tool("mcp", e))
        })
        .await
        .map_err(|e| PhazeError::tool("mcp", format!("Task join error: {e}")))??;

        // Convert MCP result to JSON
        if result.is_error {
            let error_text = result
                .content
                .iter()
                .filter_map(|c| c.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n");
            return Err(PhazeError::tool("mcp", error_text));
        }

        // Collect all text content into a single result
        let texts: Vec<&str> = result
            .content
            .iter()
            .filter_map(|c| c.text.as_deref())
            .collect();

        if texts.len() == 1 {
            // Try to parse as JSON, otherwise return as string
            if let Ok(parsed) = serde_json::from_str::<Value>(texts[0]) {
                Ok(parsed)
            } else {
                Ok(serde_json::json!({ "result": texts[0] }))
            }
        } else {
            Ok(serde_json::json!({
                "results": texts,
                "content_count": result.content.len(),
            }))
        }
    }
}

/// Create Tool-trait-compatible bridges for all tools from an McpManager.
/// Returns boxed Tool objects that can be directly registered into a ToolRegistry.
pub fn create_mcp_tool_bridges(manager: Arc<Mutex<McpManager>>) -> Vec<Box<dyn Tool>> {
    let mgr = match manager.lock() {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("Failed to lock MCP manager: {e}");
            return Vec::new();
        }
    };

    let all_tools = mgr.all_tools();
    drop(mgr); // Release lock before creating bridges

    let mut bridges: Vec<Box<dyn Tool>> = Vec::new();

    for (server_name, tool_def) in all_tools {
        let bridge = McpToolBridge::new(
            server_name,
            tool_def.name,
            tool_def.description,
            tool_def.input_schema,
            manager.clone(),
        );
        bridges.push(Box::new(bridge));
    }

    bridges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpManager;

    #[test]
    fn test_tool_name_format() {
        let manager = Arc::new(Mutex::new(McpManager::new()));
        let bridge = McpToolBridge::new(
            "github".to_string(),
            "create_issue".to_string(),
            "Create a GitHub issue".to_string(),
            serde_json::json!({"type": "object"}),
            manager,
        );

        assert_eq!(bridge.name(), "mcp__github__create_issue");
        assert_eq!(bridge.description(), "Create a GitHub issue");
    }
}
