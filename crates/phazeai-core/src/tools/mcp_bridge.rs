/// MCP Tool Bridge — makes MCP server tools available as regular PhazeAI tools.
///
/// Each MCP tool discovered from a connected server gets wrapped in an
/// `McpToolBridge` that implements the `Tool` trait. This means the AI agent
/// can call MCP tools exactly the same way it calls built-in tools like
/// `read_file` or `bash` — no special handling needed.
///
/// Every call first goes through the privacy policy
/// (`crate::privacy::mcp::tool_check`): a call whose arguments name a denied
/// file is refused before anything reaches the server, and the server's text
/// result has denied paths and `git diff` sections filtered out (and, for the
/// official filesystem server, denied listing entries) before it reaches the
/// model.
use crate::agent_event::AgentEvent;
use crate::error::PhazeError;
use crate::mcp::{McpManager, McpToolResult};
use crate::privacy::mcp::McpServerPaths;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Shared handle for MCP bridges to emit [`AgentEvent::McpReconnected`] while a run is active.
pub type McpAgentEventSink = Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>>>;

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
    /// When the agent run has wired `run_with_events`, reconnects are surfaced to the UI/CLI.
    event_sink: Option<McpAgentEventSink>,
    /// Whether the server marks this tool read-only (privacy: a mutating tool
    /// may not touch `.phazeai/`).
    read_only: bool,
    /// How the server resolves the paths in this tool's arguments.
    paths: Arc<McpServerPaths>,
    /// Whether the server is the official filesystem server, whose listing
    /// output the privacy filter understands.
    fs_listings: bool,
}

impl McpToolBridge {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        server_name: String,
        mcp_tool_name: String,
        description: String,
        input_schema: Value,
        manager: Arc<Mutex<McpManager>>,
        event_sink: Option<McpAgentEventSink>,
        read_only: bool,
        paths: Arc<McpServerPaths>,
        fs_listings: bool,
    ) -> Self {
        let tool_name = format!("mcp__{}__{}", server_name, mcp_tool_name);
        Self {
            tool_name,
            mcp_tool_name,
            server_name,
            tool_description: description,
            input_schema,
            manager,
            event_sink,
            read_only,
            paths,
            fs_listings,
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
        let full_name = self.tool_name.clone();
        let manager = self.manager.clone();
        let event_sink = self.event_sink.clone();
        let read_only = self.read_only;
        let paths = self.paths.clone();
        let fs_listings = self.fs_listings;

        // MCP transport is synchronous stdio; execute on a blocking worker thread.
        // Never hold the global manager mutex across a blocking tool call — another
        // server's MCP bridge must remain callable while one server is slow or wedged.
        let (result, checked, hidden) = tokio::task::spawn_blocking(move || {
            // Privacy first: a refused call never reaches the server (and no
            // restart happens for it).
            let checked = crate::privacy::mcp::tool_check(&full_name, read_only, &params, &paths)?;

            let restarted: Vec<String> = {
                let mut mgr = manager
                    .lock()
                    .map_err(|e| PhazeError::tool("mcp", format!("Manager lock poisoned: {e}")))?;
                mgr.health_check()
            };

            if !restarted.is_empty() {
                if let Some(sink) = event_sink.as_ref() {
                    if let Ok(g) = sink.lock() {
                        if let Some(tx) = g.as_ref() {
                            let _ = tx.send(AgentEvent::McpReconnected { servers: restarted });
                        }
                    }
                }
            }

            let client_arc = {
                let mgr = manager
                    .lock()
                    .map_err(|e| PhazeError::tool("mcp", format!("Manager lock poisoned: {e}")))?;
                mgr.client_handle(&server_name)
                    .map_err(|e| PhazeError::tool("mcp", e))?
            };

            // Call the server, then release the client lock *before* filtering,
            // so a failure (or panic) in the filter can never poison the client
            // mutex — a poisoned client is treated as dead by health_check and
            // would burn a restart, eventually dropping the server.
            let result = {
                let client_guard = client_arc.lock().map_err(|e| {
                    PhazeError::tool(
                        "mcp",
                        format!("MCP client mutex poisoned ({server_name}): {e}"),
                    )
                })?;
                client_guard
                    .call_tool(&tool_name, params.clone())
                    .map_err(|e| PhazeError::tool("mcp", e))?
            };

            // Privacy: filter what the model will see (file contents from the
            // filesystem server's read tools pass through — their paths were
            // already checked).
            let (result, hidden) =
                filter_result(&full_name, &tool_name, fs_listings, &params, &paths, result)?;
            Ok::<_, PhazeError>((result, checked, hidden))
        })
        .await
        .map_err(|e| PhazeError::tool("mcp", format!("Task join error: {e}")))??;

        if result.is_error {
            let error_text = result
                .content
                .iter()
                .filter_map(|c| c.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n");
            return Err(PhazeError::tool("mcp", error_text));
        }

        // Record the files this call named in the audit scope (this runs on
        // the agent's task, inside its audit scope, unlike the blocking work
        // above). Shows them in Settings › Privacy as sent.
        for f in &checked.files {
            crate::privacy::audit::note_file(f);
        }

        // Collect all text content into a single result.
        let texts: Vec<&str> = result
            .content
            .iter()
            .filter_map(|c| c.text.as_deref())
            .collect();

        let mut out = if texts.len() == 1 {
            // Try to parse as JSON, otherwise return as string.
            serde_json::from_str::<Value>(texts[0])
                .unwrap_or_else(|_| serde_json::json!({ "result": texts[0] }))
        } else {
            serde_json::json!({
                "results": texts,
                "content_count": result.content.len(),
            })
        };
        if hidden > 0 {
            match &mut out {
                Value::Object(map) => {
                    map.insert("privacy_skipped".to_string(), serde_json::json!(hidden));
                }
                other => {
                    out = serde_json::json!({ "result": other.clone(), "privacy_skipped": hidden });
                }
            }
        }
        Ok(out)
    }
}

/// Left in an unrecognized server's text where a denied whole-line path was
/// removed (and in a filesystem-server listing error, where a denied path it
/// quoted was), so the model sees that something was withheld rather than the
/// line vanishing silently from what might be file content.
const HIDDEN_PATH_PLACEHOLDER: &str = "[path hidden by the project's privacy policy]";

/// Filter what the model will see from an MCP tool result's text blocks.
///
/// For the official filesystem server: listing tools lose their denied entries
/// (a listing not in the expected format is refused — fail closed — and the
/// refusal logged), their error results are the server's messages and keep
/// only the paths the policy allows, the path-list tools (`search_files`,
/// `list_allowed_directories`) lose denied paths, and the read/write/edit/move
/// tools pass through unchanged, because their path arguments were already
/// checked and the result is either an allowed file's exact contents or a
/// status line. For any other server the format is unknown, so `git diff`
/// sections naming denied files are dropped and denied whole-line paths are
/// replaced with a visible placeholder.
///
/// Returns the filtered result and how many items were removed, or an error
/// when a filesystem-server listing is not in the expected format.
pub(crate) fn filter_result(
    full_name: &str,
    bare_tool: &str,
    fs_listings: bool,
    args: &Value,
    paths: &McpServerPaths,
    mut result: McpToolResult,
) -> Result<(McpToolResult, usize), PhazeError> {
    let policy = crate::privacy::active();
    let bases = crate::privacy::mcp::result_bases(&policy, paths, args);
    let home = paths.home.as_deref();
    let is_error = result.is_error;
    let mut hidden = 0;

    for block in &mut result.content {
        let Some(text) = block.text.as_mut() else {
            // Non-text blocks (image/audio/resource) are not forwarded to the
            // model today; if that ever changes, filter them by `uri` here.
            continue;
        };

        if fs_listings {
            // A listing tool's error result is the server's message, not a
            // listing: it is not parsed as one (a plain ENOENT must reach the
            // model as an error, not a privacy refusal), but loses any denied
            // path it names.
            if is_error && crate::privacy::mcp::is_fs_listing_tool(bare_tool) {
                let (filtered, n) = crate::privacy::mcp::filter_fs_error(
                    &policy,
                    &bases,
                    home,
                    text,
                    HIDDEN_PATH_PLACEHOLDER,
                );
                *text = filtered;
                hidden += n;
                continue;
            }
            // A listing tool: drop denied entries with the structured filter.
            if let Some(outcome) =
                crate::privacy::mcp::filter_fs_listing(&policy, paths, bare_tool, args, text)
            {
                match outcome {
                    Ok((filtered, n)) => {
                        *text = filtered;
                        hidden += n;
                    }
                    Err(why) => {
                        // Fail closed, and log it like any other refusal.
                        let shown = args.get("path").and_then(Value::as_str).map_or_else(
                            || "the listing".to_string(),
                            |p| policy.display_path(Path::new(p)),
                        );
                        crate::privacy::mcp::record_unchecked(&shown, &why, full_name);
                        return Err(PhazeError::tool(
                            full_name,
                            format!(
                                "REFUSED: PhazeAI could not check this listing against the \
                                 project's privacy policy ({why}). Use list_files instead."
                            ),
                        ));
                    }
                }
                continue;
            }
            // A tool whose output is a list of absolute paths: hide denied ones
            // (a dropped entry is expected here, so delete the line).
            if matches!(bare_tool, "search_files" | "list_allowed_directories") {
                let (filtered, n) =
                    crate::privacy::mcp::filter_text(&policy, &bases, home, text, None);
                *text = filtered;
                hidden += n;
                continue;
            }
            // A read/write/edit/move/create/get_file_info tool: the path
            // arguments were already checked, so the content and status are
            // from allowed files — pass them through unchanged, so an allowed
            // file's exact bytes always reach the model (a `.gitignore` line
            // such as `/.env` is a mention, not the secret, and is kept).
            if is_fs_passthrough_tool(bare_tool) {
                continue;
            }
            // An unexpected filesystem-server tool falls through to the
            // conservative filter below.
        }

        // An unrecognized server (or an unexpected filesystem-server tool): the
        // output's shape is unknown, so drop `git diff` sections naming denied
        // files and replace denied whole-line paths with a placeholder rather
        // than silently deleting lines from what might be file content.
        let (filtered, n) = crate::privacy::mcp::filter_text(
            &policy,
            &bases,
            home,
            text,
            Some(HIDDEN_PATH_PLACEHOLDER),
        );
        *text = filtered;
        hidden += n;
    }
    Ok((result, hidden))
}

/// Whether `tool` is a filesystem-server tool whose output is an allowed file's
/// content or a status line (its path arguments were already privacy-checked),
/// so the result is passed to the model unchanged.
fn is_fs_passthrough_tool(tool: &str) -> bool {
    matches!(
        tool,
        "read_text_file"
            | "read_file"
            | "read_media_file"
            | "read_multiple_files"
            | "write_file"
            | "edit_file"
            | "move_file"
            | "create_directory"
            | "get_file_info"
    )
}

/// Create Tool-trait-compatible bridges for all tools from an McpManager.
/// Returns boxed Tool objects that can be directly registered into a ToolRegistry.
pub fn create_mcp_tool_bridges(
    manager: Arc<Mutex<McpManager>>,
    event_sink: Option<McpAgentEventSink>,
) -> Vec<Box<dyn Tool>> {
    let specs = match manager.lock() {
        Ok(m) => m.bridge_specs(),
        Err(e) => {
            tracing::error!("Failed to lock MCP manager: {e}");
            return Vec::new();
        }
    };

    let mut bridges: Vec<Box<dyn Tool>> = Vec::new();
    for spec in specs {
        let read_only = spec.tool.read_only();
        let paths = Arc::new(spec.paths);
        let bridge = McpToolBridge::new(
            spec.server_name,
            spec.tool.name,
            spec.tool.description,
            spec.tool.input_schema,
            manager.clone(),
            event_sink.clone(),
            read_only,
            paths,
            spec.fs_listings,
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
            None,
            false,
            Arc::new(McpServerPaths::default()),
            false,
        );

        assert_eq!(bridge.name(), "mcp__github__create_issue");
        assert_eq!(bridge.description(), "Create a GitHub issue");
    }
}
