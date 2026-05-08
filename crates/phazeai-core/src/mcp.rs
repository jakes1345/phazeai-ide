/// Model Context Protocol (MCP) client for PhazeAI.
///
/// MCP allows PhazeAI to connect to external tool servers that expose
/// resources, tools, and prompts over a standardized JSON-RPC protocol.
/// This enables integration with databases, APIs, file systems, and
/// any MCP-compatible server without modifying PhazeAI's core.
///
/// Protocol spec: https://modelcontextprotocol.io/specification
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_RESTARTS_PER_WINDOW: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// An MCP tool definition received from a server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: serde_json::Value,
}

/// An MCP resource exposed by a server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "mimeType")]
    pub mime_type: Option<String>,
}

/// An MCP prompt template from a server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// Result of calling an MCP tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    #[serde(default)]
    pub content: Vec<McpContent>,
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpContent {
    #[serde(rename = "type")]
    pub content_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default, rename = "mimeType")]
    pub mime_type: Option<String>,
}

/// Server info returned by initialize
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub version: String,
}

/// Configuration for an MCP server connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Display name for this server
    pub name: String,
    /// Command to launch the server (e.g., "npx", "python3", "node")
    pub command: String,
    /// Arguments to pass to the command
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to set
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// An active connection to a single MCP server over stdio.
pub struct McpClient {
    name: String,
    process: Child,
    stdin: Arc<Mutex<Box<dyn Write + Send>>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>>,
    server_info: Option<McpServerInfo>,
    tools: Vec<McpToolDef>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
    alive: Arc<AtomicBool>,
}

impl McpClient {
    /// Connect to an MCP server by spawning the process and initializing.
    pub fn connect(config: &McpServerConfig) -> Result<Self, String> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        let mut process = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server '{}': {e}", config.name))?;

        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| "Failed to get stdin of MCP server".to_string())?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| "Failed to get stdout of MCP server".to_string())?;
        let stderr = process
            .stderr
            .take()
            .ok_or_else(|| "Failed to get stderr of MCP server".to_string())?;

        let stdin = Arc::new(Mutex::new(Box::new(stdin) as Box<dyn Write + Send>));
        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Spawn reader thread for stdout (JSON-RPC framing).
        let pending_clone = pending.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let alive_clone = alive.clone();
        let name_clone = config.name.clone();
        std::thread::spawn(move || {
            Self::read_loop(stdout, pending_clone);
            alive_clone.store(false, Ordering::SeqCst);
            tracing::warn!("MCP server '{name_clone}' reader loop exited (EOF)");
        });

        // Drain stderr into the structured log so server crashes/diagnostics
        // are visible. Without this the pipe fills and the child blocks on
        // its own stderr writes; the only way to see MCP failures becomes
        // staring at "no tools registered" with no actionable error.
        let stderr_name = config.name.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    tracing::warn!(
                        target: "phazeai_core::mcp::stderr",
                        server = %stderr_name,
                        "{line}"
                    );
                }
            }
        });

        let mut client = Self {
            name: config.name.clone(),
            process,
            stdin,
            next_id: AtomicU64::new(1),
            pending,
            server_info: None,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
            alive,
        };

        client.initialize()?;
        Ok(client)
    }

    /// Send the MCP initialize handshake
    fn initialize(&mut self) -> Result<(), String> {
        let result = self.send_request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "roots": { "listChanged": true }
                },
                "clientInfo": {
                    "name": "PhazeAI",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;

        if let Some(info) = result.get("serverInfo") {
            self.server_info = serde_json::from_value(info.clone()).ok();
        }

        // Send initialized notification
        self.send_notification("notifications/initialized", serde_json::json!({}))?;

        // Discover capabilities
        self.refresh_tools()?;
        self.refresh_resources()?;
        self.refresh_prompts()?;

        tracing::info!(
            "MCP server '{}' initialized: {} tools, {} resources, {} prompts",
            self.name,
            self.tools.len(),
            self.resources.len(),
            self.prompts.len(),
        );

        Ok(())
    }

    /// Refresh the list of available tools from the server
    pub fn refresh_tools(&mut self) -> Result<(), String> {
        let result = self.send_request("tools/list", serde_json::json!({}))?;
        if let Some(tools) = result.get("tools") {
            self.tools = serde_json::from_value(tools.clone())
                .map_err(|e| format!("Failed to parse tools: {e}"))?;
        }
        Ok(())
    }

    /// Refresh the list of available resources
    pub fn refresh_resources(&mut self) -> Result<(), String> {
        let result = self.send_request("resources/list", serde_json::json!({}))?;
        if let Some(resources) = result.get("resources") {
            self.resources = serde_json::from_value(resources.clone())
                .map_err(|e| format!("Failed to parse resources: {e}"))?;
        }
        Ok(())
    }

    /// Refresh the list of available prompts
    pub fn refresh_prompts(&mut self) -> Result<(), String> {
        let result = self.send_request("prompts/list", serde_json::json!({}))?;
        if let Some(prompts) = result.get("prompts") {
            self.prompts = serde_json::from_value(prompts.clone())
                .map_err(|e| format!("Failed to parse prompts: {e}"))?;
        }
        Ok(())
    }

    /// Call a tool on the MCP server
    pub fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, String> {
        let result = self.send_request(
            "tools/call",
            serde_json::json!({
                "name": name,
                "arguments": arguments,
            }),
        )?;

        serde_json::from_value(result).map_err(|e| format!("Failed to parse tool result: {e}"))
    }

    /// Read a resource from the MCP server
    pub fn read_resource(&self, uri: &str) -> Result<Vec<McpContent>, String> {
        let result = self.send_request("resources/read", serde_json::json!({ "uri": uri }))?;

        if let Some(contents) = result.get("contents") {
            serde_json::from_value(contents.clone())
                .map_err(|e| format!("Failed to parse resource: {e}"))
        } else {
            Ok(Vec::new())
        }
    }

    /// Get a prompt from the MCP server
    pub fn get_prompt(
        &self,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let result = self.send_request(
            "prompts/get",
            serde_json::json!({
                "name": name,
                "arguments": arguments,
            }),
        )?;

        if let Some(messages) = result.get("messages") {
            serde_json::from_value(messages.clone())
                .map_err(|e| format!("Failed to parse prompt messages: {e}"))
        } else {
            Ok(Vec::new())
        }
    }

    /// Get the list of tools this server provides
    pub fn tools(&self) -> &[McpToolDef] {
        &self.tools
    }

    /// Get the list of resources this server provides
    pub fn resources(&self) -> &[McpResource] {
        &self.resources
    }

    /// Get the list of prompts this server provides
    pub fn prompts(&self) -> &[McpPrompt] {
        &self.prompts
    }

    /// Server name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Server info (if available after initialization)
    pub fn server_info(&self) -> Option<&McpServerInfo> {
        self.server_info.as_ref()
    }

    /// True if the underlying server process and read loop are still running.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    // ── JSON-RPC Transport ────────────────────────────────────────────

    fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let (tx, rx) = mpsc::channel();

        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|e| format!("Lock poisoned: {e}"))?;
            pending.insert(id, tx);
        }

        if let Err(e) = self.send_raw(&request) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(e);
        }

        // Block waiting for response with a real timeout.
        let response = rx.recv_timeout(REQUEST_TIMEOUT).map_err(|_| {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            // Mark unhealthy so `health_check` can recycle wedged-but-alive processes.
            self.alive.store(false, Ordering::SeqCst);
            format!(
                "MCP server '{}' timed out after {}s while calling '{}'",
                self.name,
                REQUEST_TIMEOUT.as_secs(),
                method
            )
        })?;

        // Check for JSON-RPC error
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            return Err(format!("MCP error ({code}): {message}"));
        }

        Ok(response
            .get("result")
            .cloned()
            .unwrap_or(serde_json::json!({})))
    }

    fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<(), String> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        self.send_raw(&notification)
    }

    fn send_raw(&self, message: &serde_json::Value) -> Result<(), String> {
        let body = serde_json::to_string(message)
            .map_err(|e| format!("Failed to serialize message: {e}"))?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        let mut stdin = self
            .stdin
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;
        stdin
            .write_all(header.as_bytes())
            .map_err(|e| format!("Failed to write to MCP server: {e}"))?;
        stdin
            .write_all(body.as_bytes())
            .map_err(|e| format!("Failed to write to MCP server: {e}"))?;
        stdin
            .flush()
            .map_err(|e| format!("Failed to flush MCP server stdin: {e}"))?;

        Ok(())
    }

    fn read_loop(
        stdout: impl std::io::Read + Send + 'static,
        pending: Arc<Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>>,
    ) {
        let mut reader = BufReader::new(stdout);

        while let Ok(content_length) = Self::read_content_length(&mut reader) {
            // Read the body
            let mut body = vec![0u8; content_length];
            if reader.read_exact(&mut body).is_err() {
                break;
            }

            let response: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Failed to parse MCP response: {e}");
                    continue;
                }
            };

            // Match response to pending request
            if let Some(id) = response.get("id").and_then(|i| i.as_u64()) {
                let sender = {
                    let mut pending = match pending.lock() {
                        Ok(p) => p,
                        Err(_) => break,
                    };
                    pending.remove(&id)
                };
                if let Some(sender) = sender {
                    let _ = sender.send(response);
                }
            }
            // Notifications from server (no id) — log them
            else if let Some(method) = response.get("method").and_then(|m| m.as_str()) {
                tracing::debug!("MCP notification: {method}");
            }
        }
    }

    fn read_content_length(reader: &mut impl BufRead) -> Result<usize, String> {
        let mut header_line = String::new();
        loop {
            header_line.clear();
            let bytes_read = reader
                .read_line(&mut header_line)
                .map_err(|e| format!("Read error: {e}"))?;
            if bytes_read == 0 {
                return Err("EOF".into());
            }

            let trimmed = header_line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                let len: usize = len_str
                    .trim()
                    .parse()
                    .map_err(|e| format!("Invalid Content-Length: {e}"))?;

                // Read the blank line after headers
                let mut blank = String::new();
                let _ = reader.read_line(&mut blank);

                return Ok(len);
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.process.kill();
    }
}

// ── MCP Manager ──────────────────────────────────────────────────────

/// Manages multiple MCP server connections.
/// Loads server configs from `.phazeai/mcp.json` and connects to them.
pub struct McpManager {
    /// Each server is isolated behind its own mutex so one slow/hung tool call
    /// cannot block every other MCP server while waiting on stdio.
    clients: HashMap<String, Arc<Mutex<McpClient>>>,
    /// Configs kept by name so dead servers can be restarted with the same launch params.
    configs: HashMap<String, McpServerConfig>,
    /// Restart timestamps per server-name for rate-cap.
    restart_history: HashMap<String, VecDeque<Instant>>,
    /// Servers we've stopped retrying after exceeding the cap.
    blocked: HashMap<String, Instant>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            configs: HashMap::new(),
            restart_history: HashMap::new(),
            blocked: HashMap::new(),
        }
    }

    /// Load MCP server configs from the project's `.phazeai/mcp.json`
    pub fn load_config(project_root: &Path) -> Vec<McpServerConfig> {
        let config_path = project_root.join(".phazeai").join("mcp.json");
        if !config_path.exists() {
            return Vec::new();
        }

        let content = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read MCP config: {e}");
                return Vec::new();
            }
        };

        #[derive(Deserialize)]
        struct McpConfigFile {
            #[serde(default)]
            servers: Vec<McpServerConfig>,
        }

        match serde_json::from_str::<McpConfigFile>(&content) {
            Ok(config) => config.servers,
            Err(e) => {
                tracing::warn!("Failed to parse MCP config: {e}");
                Vec::new()
            }
        }
    }

    /// Connect to all configured MCP servers
    pub fn connect_all(&mut self, configs: &[McpServerConfig]) {
        for config in configs {
            self.configs.insert(config.name.clone(), config.clone());
            match McpClient::connect(config) {
                Ok(client) => {
                    tracing::info!(
                        "Connected to MCP server '{}': {} tools",
                        config.name,
                        client.tools().len()
                    );
                    self.clients
                        .insert(config.name.clone(), Arc::new(Mutex::new(client)));
                }
                Err(e) => {
                    tracing::error!("Failed to connect to MCP server '{}': {e}", config.name);
                }
            }
        }
    }

    /// Detect dead MCP clients and respawn them (rate-limited).
    /// Returns server names that were successfully reconnected this call.
    /// Capabilities (tools/resources/prompts) are re-discovered automatically by
    /// `McpClient::connect` → `initialize`.
    pub fn health_check(&mut self) -> Vec<String> {
        let mut reconnected = Vec::new();
        let dead: Vec<String> = self
            .clients
            .iter()
            .filter(|(_, arc)| arc.lock().map(|c| !c.is_alive()).unwrap_or(true))
            .map(|(name, _)| name.clone())
            .collect();

        for name in dead {
            tracing::warn!("MCP server '{name}' died; attempting restart");
            self.clients.remove(&name);

            if !self.allow_restart(&name) {
                tracing::error!(
                    "MCP server '{name}' exceeded {} restarts in {}s; giving up",
                    MAX_RESTARTS_PER_WINDOW,
                    RESTART_WINDOW.as_secs()
                );
                self.blocked.insert(name.clone(), Instant::now());
                continue;
            }

            let Some(config) = self.configs.get(&name).cloned() else {
                tracing::error!("No stored config for MCP server '{name}'; cannot restart");
                continue;
            };

            match McpClient::connect(&config) {
                Ok(client) => {
                    tracing::info!(
                        "Reconnected MCP server '{name}': {} tools",
                        client.tools().len()
                    );
                    reconnected.push(name.clone());
                    self.clients.insert(name, Arc::new(Mutex::new(client)));
                }
                Err(e) => {
                    tracing::error!("Failed to restart MCP server '{name}': {e}");
                }
            }
        }
        reconnected
    }

    fn allow_restart(&mut self, name: &str) -> bool {
        let now = Instant::now();
        let history = self.restart_history.entry(name.to_string()).or_default();
        while let Some(&front) = history.front() {
            if now.duration_since(front) > RESTART_WINDOW {
                history.pop_front();
            } else {
                break;
            }
        }
        if history.len() >= MAX_RESTARTS_PER_WINDOW {
            return false;
        }
        history.push_back(now);
        true
    }

    /// Get all tools from all connected MCP servers (prefixed with server name)
    pub fn all_tools(&self) -> Vec<(String, McpToolDef)> {
        let mut tools = Vec::new();
        for (server_name, arc) in &self.clients {
            let Ok(client) = arc.lock() else {
                continue;
            };
            for tool in client.tools() {
                tools.push((server_name.clone(), tool.clone()));
            }
        }
        tools
    }

    /// Call a tool on a specific server
    pub fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResult, String> {
        let arc = self.client_handle(server_name)?;
        let client = arc
            .lock()
            .map_err(|e| format!("MCP client mutex poisoned ({server_name}): {e}"))?;
        client.call_tool(tool_name, arguments)
    }

    /// Resolve a connected server handle without invoking tools — callers should
    /// drop the manager lock before blocking on per-client I/O.
    pub fn client_handle(&self, server_name: &str) -> Result<Arc<Mutex<McpClient>>, String> {
        self.clients
            .get(server_name)
            .cloned()
            .ok_or_else(|| format!("No MCP server connected with name '{server_name}'"))
    }

    /// Get all resources from all connected MCP servers
    pub fn all_resources(&self) -> Vec<(String, McpResource)> {
        let mut resources = Vec::new();
        for (server_name, arc) in &self.clients {
            let Ok(client) = arc.lock() else {
                continue;
            };
            for resource in client.resources() {
                resources.push((server_name.clone(), resource.clone()));
            }
        }
        resources
    }

    /// Disconnect all servers
    pub fn disconnect_all(&mut self) {
        self.clients.clear();
    }

    /// Check if any MCP servers are connected
    pub fn has_connections(&self) -> bool {
        !self.clients.is_empty()
    }

    /// Number of connected servers
    pub fn connection_count(&self) -> usize {
        self.clients.len()
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

use std::io::Read;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_restart_caps_at_three_per_window() {
        let mut m = McpManager::new();
        assert!(m.allow_restart("server-a"));
        assert!(m.allow_restart("server-a"));
        assert!(m.allow_restart("server-a"));
        assert!(!m.allow_restart("server-a"));
    }

    #[test]
    fn allow_restart_is_per_server_name() {
        let mut m = McpManager::new();
        for _ in 0..MAX_RESTARTS_PER_WINDOW {
            assert!(m.allow_restart("a"));
        }
        assert!(!m.allow_restart("a"));
        assert!(m.allow_restart("b"));
    }
}
