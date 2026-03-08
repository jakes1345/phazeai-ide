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
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
    server_info: Option<McpServerInfo>,
    tools: Vec<McpToolDef>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
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

        let stdin = Arc::new(Mutex::new(Box::new(stdin) as Box<dyn Write + Send>));
        let pending: Arc<
            Mutex<HashMap<u64, tokio::sync::oneshot::Sender<serde_json::Value>>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        // Spawn reader thread
        let pending_clone = pending.clone();
        std::thread::spawn(move || {
            Self::read_loop(stdout, pending_clone);
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
        let result = self.send_request(
            "resources/read",
            serde_json::json!({ "uri": uri }),
        )?;

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

        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|e| format!("Lock poisoned: {e}"))?;
            pending.insert(id, tx);
        }

        self.send_raw(&request)?;

        // Block waiting for response (with timeout)
        let response = rx
            .blocking_recv()
            .map_err(|_| format!("MCP server '{}' did not respond to '{method}'", self.name))?;

        // Check for JSON-RPC error
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            return Err(format!("MCP error ({code}): {message}"));
        }

        Ok(response.get("result").cloned().unwrap_or(serde_json::json!({})))
    }

    fn send_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), String> {
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
        pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<serde_json::Value>>>>,
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
    clients: HashMap<String, McpClient>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
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
            match McpClient::connect(config) {
                Ok(client) => {
                    tracing::info!(
                        "Connected to MCP server '{}': {} tools",
                        config.name,
                        client.tools().len()
                    );
                    self.clients.insert(config.name.clone(), client);
                }
                Err(e) => {
                    tracing::error!("Failed to connect to MCP server '{}': {e}", config.name);
                }
            }
        }
    }

    /// Get all tools from all connected MCP servers (prefixed with server name)
    pub fn all_tools(&self) -> Vec<(String, McpToolDef)> {
        let mut tools = Vec::new();
        for (server_name, client) in &self.clients {
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
        let client = self
            .clients
            .get(server_name)
            .ok_or_else(|| format!("No MCP server connected with name '{server_name}'"))?;
        client.call_tool(tool_name, arguments)
    }

    /// Get all resources from all connected MCP servers
    pub fn all_resources(&self) -> Vec<(String, McpResource)> {
        let mut resources = Vec::new();
        for (server_name, client) in &self.clients {
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
