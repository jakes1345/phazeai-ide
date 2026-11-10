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
    /// One model call's token counts, sent as soon as the provider reports
    /// them, so a meter can count a run as it goes (and a run that stops or
    /// fails early). `TokenUsage` at the end has the run's totals.
    CallUsage(crate::llm::Usage),
    /// One model call of a `subagent` helper, on its own (cheaper) model,
    /// so a meter prices it at that model's rates. Not part of `TokenUsage`.
    SubagentUsage {
        model: String,
        usage: crate::llm::Usage,
    },
    /// The run's token counts. `input_tokens` includes the prompt tokens the
    /// provider served from its cache (`cache_read_tokens`) or wrote to it
    /// (`cache_write_tokens`).
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
    },
    Error(String),
    /// Older messages were summarised to stay within the context budget.
    ContextCompacted {
        messages_folded: usize,
        tokens_before: usize,
        tokens_after: usize,
    },
    /// One or more MCP stdio servers were unhealthy and were respawned.
    McpReconnected {
        servers: Vec<String>,
    },
    /// A file write is about to be shown to the diff hook for review (only
    /// with one; see [`crate::DiffHookFn`]): the file, what it holds now and
    /// exactly what it will hold if approved.
    FilePatch {
        path: String,
        before: String,
        after: String,
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
