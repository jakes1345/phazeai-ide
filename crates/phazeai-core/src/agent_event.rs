/// Streaming surface for a single [`crate::agent::Agent`] run. Lives in its own
/// module so MCP tool bridges can emit events without an `agent` ↔ `tools`
/// dependency cycle.
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Thinking {
        iteration: usize,
    },
    TextDelta(String),
    ToolApprovalRequest {
        name: String,
        params: Value,
    },
    ToolStart {
        name: String,
    },
    ToolResult {
        name: String,
        success: bool,
        summary: String,
    },
    Complete {
        iterations: usize,
    },
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Error(String),
    /// One or more MCP stdio servers were unhealthy and were respawned.
    McpReconnected {
        servers: Vec<String>,
    },
    // Browser Integration
    BrowserFetchStart {
        url: String,
    },
    BrowserFetchComplete {
        url: String,
        title: String,
        content: String,
    },
    BrowserFetchError {
        url: String,
        error: String,
    },
}
