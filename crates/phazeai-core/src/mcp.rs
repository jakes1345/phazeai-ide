use crate::process::ChildSlot;
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
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_RESTARTS_PER_WINDOW: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

mod http;

/// `serverInfo.name` of the official `@modelcontextprotocol/server-filesystem`,
/// whose listing output the privacy filter understands.
pub const FS_SERVER_NAME: &str = "secure-filesystem-server";

/// An MCP tool definition received from a server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    /// Optional in MCP: a tool without one is still listed, described as "".
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: serde_json::Value,
    /// The server's behaviour hints. Only `read_only_hint` is read, to decide
    /// whether a call may touch `.phazeai/` (privacy); it is a hint from an
    /// untrusted server, so it is used only to *tighten*, never to approve.
    #[serde(default)]
    pub annotations: Option<McpToolAnnotations>,
}

/// MCP tool annotations (a server's behaviour hints).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpToolAnnotations {
    #[serde(default, rename = "readOnlyHint")]
    pub read_only_hint: Option<bool>,
    #[serde(default, rename = "destructiveHint")]
    pub destructive_hint: Option<bool>,
    #[serde(default, rename = "openWorldHint")]
    pub open_world_hint: Option<bool>,
}

impl McpToolDef {
    /// Whether the server marks this tool `readOnlyHint: true`. A tool with no
    /// annotation counts as mutating (the MCP default), so the privacy checker
    /// protects `.phazeai/` from it.
    pub fn read_only(&self) -> bool {
        self.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true)
    }
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

/// Configuration for an MCP server connection: a command to run (stdio), or
/// the `url` of a remote server (Streamable HTTP).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Display name for this server
    pub name: String,
    /// Command to launch the server (e.g., "npx", "python3", "node")
    #[serde(default)]
    pub command: String,
    /// Arguments to pass to the command
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to set
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// A remote server's MCP endpoint (`https://…/mcp`), spoken to with the
    /// Streamable HTTP transport instead of running `command`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Headers sent with every request to `url` (an API key, say). `${VAR}`
    /// in a value is replaced with that environment variable, so the key
    /// itself need not be written in the file.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

impl McpServerConfig {
    /// How this server resolves the paths in its arguments, for the privacy
    /// checker. PhazeAI does not set the server's working directory, so it
    /// runs in PhazeAI's own cwd; relative paths resolve against that and
    /// against the existing directories named on the command line (the
    /// filesystem server's allowed directories). `~` is the server's `HOME`
    /// (its `env` entry if set, else this process's home).
    pub fn privacy_paths(&self) -> crate::privacy::mcp::McpServerPaths {
        let home = self
            .env
            .get("HOME")
            .map(PathBuf::from)
            .or_else(dirs::home_dir);
        let cwd = std::env::current_dir().ok();
        let mut bases: Vec<PathBuf> = Vec::new();
        if let Some(cwd) = &cwd {
            bases.push(cwd.clone());
        }
        for arg in &self.args {
            let expanded = if arg == "~" {
                home.clone()
            } else if let Some(rest) = arg.strip_prefix("~/") {
                home.as_ref().map(|h| h.join(rest))
            } else {
                Some(PathBuf::from(arg))
            };
            let Some(expanded) = expanded else { continue };
            let abs = if expanded.is_absolute() {
                expanded
            } else if let Some(cwd) = &cwd {
                cwd.join(expanded)
            } else {
                expanded
            };
            // Only existing directories are allowed-dirs the server resolves
            // against; canonicalize so a symlinked path matches the root.
            if abs.is_dir() {
                let abs = abs.canonicalize().unwrap_or(abs);
                if !bases.contains(&abs) {
                    bases.push(abs);
                }
            }
        }
        crate::privacy::mcp::McpServerPaths {
            bases,
            home,
            tree_dirs: false,
        }
    }
}

/// Everything the bridge needs to wrap one MCP tool: its server, definition,
/// how that server resolves paths, and whether it is the official filesystem
/// server (whose listings can be filtered).
pub struct McpBridgeSpec {
    pub server_name: String,
    pub tool: McpToolDef,
    pub paths: crate::privacy::mcp::McpServerPaths,
    pub fs_listings: bool,
}

/// How messages reach the server.
enum Transport {
    /// A child process: one JSON message per line on its stdin and stdout.
    Stdio {
        process: ChildSlot,
        stdin: Arc<Mutex<Box<dyn Write + Send>>>,
    },
    /// A remote server: each message is an HTTP POST (see [`http`]).
    Http(http::HttpTransport),
}

/// An active connection to a single MCP server, over stdio or HTTP.
pub struct McpClient {
    name: String,
    transport: Transport,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>>,
    server_info: Option<McpServerInfo>,
    tools: Vec<McpToolDef>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
    alive: Arc<AtomicBool>,
}

impl McpClient {
    /// Connect to an MCP server: spawn its process (or reach its `url`) and
    /// initialize.
    pub fn connect(config: &McpServerConfig) -> Result<Self, String> {
        if let Some(url) = config
            .url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            return Self::connect_http(config, url);
        }
        if config.command.trim().is_empty() {
            return Err(format!(
                "MCP server '{}' needs a `command` to run or a `url` to reach",
                config.name
            ));
        }
        let mut cmd = crate::process::std_command(&config.command);
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
        let process = ChildSlot::new(process);
        let reader_process = process.clone();
        let reply_stdin = stdin.clone();
        std::thread::spawn(move || {
            Self::read_loop(stdout, pending_clone.clone(), |answer| {
                if let Err(e) = write_line(&reply_stdin, answer) {
                    tracing::warn!("MCP server '{name_clone}': could not answer its request: {e}");
                }
            });
            // The server exited or closed its output: reap it before
            // `is_alive` turns false, then fail every request still waiting
            // for an answer instead of leaving each to time out.
            reader_process.end();
            Self::server_gone(&alive_clone, &pending_clone);
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
            transport: Transport::Stdio { process, stdin },
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

    /// Reach a remote server at `url` with the Streamable HTTP transport.
    fn connect_http(config: &McpServerConfig, url: &str) -> Result<Self, String> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| format!("MCP server '{}': `{url}` is not a URL: {e}", config.name))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!(
                "MCP server '{}': only http and https URLs are supported",
                config.name
            ));
        }
        let host = parsed.host_str().unwrap_or_default().to_string();
        let local = crate::privacy::is_loopback_host(&host);
        if !local && crate::privacy::active().local_only() {
            return Err(format!(
                "MCP server '{}' at {host} is not on this machine, and this project's \
                 privacy policy is local-only",
                config.name
            ));
        }
        if !local && parsed.scheme() == "http" && !config.headers.is_empty() {
            return Err(format!(
                "MCP server '{}': its headers would be sent unencrypted to {host}; use https",
                config.name
            ));
        }
        let mut headers = Vec::new();
        for (name, value) in &config.headers {
            let value = http::expand_env(value).map_err(|var| {
                format!(
                    "MCP server '{}': header `{name}` uses ${{{var}}}, which is not set",
                    config.name
                )
            })?;
            headers.push((name.clone(), value));
        }
        let alive = Arc::new(AtomicBool::new(true));
        let transport =
            http::HttpTransport::start(&config.name, parsed, headers, local, alive.clone())?;
        let mut client = Self {
            name: config.name.clone(),
            transport: Transport::Http(transport),
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
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
        // Streamable HTTP came with 2025-03-26; stdio servers keep the
        // version they have always been sent.
        let version = match self.transport {
            Transport::Http(_) => http::PROTOCOL_VERSION,
            Transport::Stdio { .. } => "2024-11-05",
        };
        let result = self.send_request(
            "initialize",
            serde_json::json!({
                "protocolVersion": version,
                // None. The client answers a server's `ping` and turns down
                // every other request it sends (`answer_server_request`), so
                // it must not offer `roots`: a server offered roots asks for
                // them with `roots/list`, and the official filesystem server
                // holds every tool call until that is answered with a list.
                "capabilities": {},
                "clientInfo": {
                    "name": "PhazeAI",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;

        if let Some(info) = result.get("serverInfo") {
            self.server_info = serde_json::from_value(info.clone()).ok();
        }
        if let Transport::Http(http) = &self.transport {
            if let Some(v) = result.get("protocolVersion").and_then(|v| v.as_str()) {
                http.set_protocol_version(v);
            }
        }

        // Send initialized notification
        self.send_notification("notifications/initialized", serde_json::json!({}))?;

        // Discover only what the server declared. A server without, say,
        // resources answers `resources/list` with "Method not found", which
        // must not fail the whole connection.
        let capabilities = result.get("capabilities").cloned().unwrap_or_default();
        if capabilities.get("tools").is_some() {
            self.refresh_tools()?;
        }
        if capabilities.get("resources").is_some() {
            self.refresh_resources()?;
        }
        if capabilities.get("prompts").is_some() {
            self.refresh_prompts()?;
        }

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

        if let Transport::Http(http) = &self.transport {
            let response = http
                .request(request, id)
                .map_err(|e| format!("MCP server '{}' while calling '{method}': {e}", self.name))?;
            return Self::result_of(response);
        }

        let (tx, rx) = mpsc::channel();

        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|e| format!("Lock poisoned: {e}"))?;
            pending.insert(id, tx);
        }

        // Checked after the request is listed: the reader marks the server
        // gone before it clears the list, so either this sees it gone or the
        // reader's clearing drops this request's sender, and the wait below
        // ends at once.
        if !self.is_alive() {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(format!(
                "MCP server '{}' is not running (it exited or stopped answering), so \
                 '{method}' was not sent",
                self.name
            ));
        }

        if let Err(e) = self.send_raw(&request) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(e);
        }

        // Block waiting for response with a real timeout.
        let response = rx.recv_timeout(REQUEST_TIMEOUT).map_err(|e| {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            match e {
                mpsc::RecvTimeoutError::Disconnected => format!(
                    "MCP server '{}' exited before answering '{method}'",
                    self.name
                ),
                mpsc::RecvTimeoutError::Timeout => {
                    // Mark unhealthy so `health_check` can recycle
                    // wedged-but-alive processes.
                    self.alive.store(false, Ordering::SeqCst);
                    format!(
                        "MCP server '{}' timed out after {}s while calling '{}'",
                        self.name,
                        REQUEST_TIMEOUT.as_secs(),
                        method
                    )
                }
            }
        })?;

        Self::result_of(response)
    }

    /// A JSON-RPC response's `result`, or its `error` as a message.
    fn result_of(response: serde_json::Value) -> Result<serde_json::Value, String> {
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

    /// MCP's stdio transport frames each message as one line of JSON with no
    /// embedded newlines (`serde_json::to_string` escapes them). It is not
    /// LSP's Content-Length framing: a server reads up to `\n`, so a header
    /// is a parse error and an unterminated body is never read at all.
    fn send_raw(&self, message: &serde_json::Value) -> Result<(), String> {
        match &self.transport {
            Transport::Http(http) => http.notify(message.clone()),
            Transport::Stdio { stdin, .. } => write_line(stdin, message),
        }
    }

    /// The server's output has ended: mark it gone, then drop every waiting
    /// request's sender so each wait ends at once with "exited" rather than
    /// after [`REQUEST_TIMEOUT`]. In that order: see `send_request`.
    fn server_gone(
        alive: &AtomicBool,
        pending: &Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>,
    ) {
        alive.store(false, Ordering::SeqCst);
        pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Reads one JSON-RPC message per line (see `send_raw`) until EOF. A
    /// request from the server is answered through `reply`
    /// ([`answer_server_request`]); a response goes to the request waiting
    /// for it.
    fn read_loop(
        stdout: impl std::io::Read + Send + 'static,
        pending: Arc<Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>>,
        reply: impl Fn(&serde_json::Value),
    ) {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();

        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }

            let response: serde_json::Value = match serde_json::from_slice(&line) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Failed to parse MCP response: {e}");
                    continue;
                }
            };

            // A request from the server. Its id is the server's own, so it
            // must not be taken for the answer to one of ours.
            if let Some(answer) = answer_server_request(&response) {
                reply(&answer);
                continue;
            }

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
}

/// Writes `message` as one line of JSON (MCP's stdio framing; see
/// `McpClient::send_raw`).
fn write_line(
    stdin: &Mutex<Box<dyn Write + Send>>,
    message: &serde_json::Value,
) -> Result<(), String> {
    let mut line =
        serde_json::to_string(message).map_err(|e| format!("Failed to serialize message: {e}"))?;
    line.push('\n');

    let mut stdin = stdin.lock().map_err(|e| format!("Lock poisoned: {e}"))?;
    stdin
        .write_all(line.as_bytes())
        .map_err(|e| format!("Failed to write to MCP server: {e}"))?;
    stdin
        .flush()
        .map_err(|e| format!("Failed to flush MCP server stdin: {e}"))?;
    Ok(())
}

/// The answer to `message` when it is a request from the server (it has a
/// `method` and an `id`), or `None` for a response or a notification.
/// `ping` gets the empty result MCP requires: a server that pings to see
/// whether the client is still there may otherwise drop the connection.
/// Anything else gets "Method not found", as the client offers no
/// capability (roots, sampling, elicitation) a server may ask it to use;
/// an answer, even a refusal, keeps the server from waiting on one.
fn answer_server_request(message: &serde_json::Value) -> Option<serde_json::Value> {
    let method = message.get("method")?.as_str()?;
    let id = message
        .get("id")
        .filter(|id| id.is_string() || id.is_number())?;
    Some(if method == "ping" {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}})
    } else {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("Method not found: {method}")},
        })
    })
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // An HTTP transport ends its session when its thread sees the
        // channel close.
        if let Transport::Stdio { process, .. } = &self.transport {
            process.end();
        }
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

    /// One [`McpBridgeSpec`] per tool of every connected server, carrying the
    /// server's path-resolution context so the bridge can enforce the privacy
    /// policy on calls and results.
    pub fn bridge_specs(&self) -> Vec<McpBridgeSpec> {
        let mut specs = Vec::new();
        for (server_name, arc) in &self.clients {
            let Ok(client) = arc.lock() else {
                continue;
            };
            let paths = self
                .configs
                .get(server_name)
                .map(|c| c.privacy_paths())
                .unwrap_or_default();
            let fs_listings = client
                .server_info()
                .is_some_and(|i| i.name == FS_SERVER_NAME);
            for tool in client.tools() {
                specs.push(McpBridgeSpec {
                    server_name: server_name.clone(),
                    tool: tool.clone(),
                    paths: paths.clone(),
                    fs_listings,
                });
            }
        }
        specs
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

    #[test]
    fn read_loop_takes_one_json_message_per_line() {
        // What a stdio server writes, with a blank line, a stray log line and
        // a notification around the answer. The `\n` inside the result is
        // JSON-escaped, so it does not end the line.
        let stdout = [
            "",
            "not JSON: a log line on the wrong stream",
            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":{"text":"two\nlines"}}"#,
        ]
        .map(|line| format!("{line}\n"))
        .concat();
        let (tx, rx) = mpsc::channel();
        let pending = Arc::new(Mutex::new(HashMap::from([(7, tx)])));

        McpClient::read_loop(
            std::io::Cursor::new(stdout.into_bytes()),
            pending.clone(),
            |answer| panic!("nothing here is a request, yet it answered {answer}"),
        );

        let response = rx.try_recv().expect("the answer to request 7");
        assert_eq!(response["result"]["text"], "two\nlines");
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn the_server_s_ping_is_answered_and_its_ids_are_not_taken_for_ours() {
        // The server pings with id 7 while our request 7 is waiting, asks for
        // roots (which the client doesn't offer), then answers request 7.
        let stdout = [
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":"r-1","method":"roots/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#,
        ]
        .map(|line| format!("{line}\n"))
        .concat();
        let (tx, rx) = mpsc::channel();
        let pending = Arc::new(Mutex::new(HashMap::from([(7, tx)])));
        let answers = Mutex::new(Vec::new());

        McpClient::read_loop(
            std::io::Cursor::new(stdout.into_bytes()),
            pending.clone(),
            |answer| answers.lock().unwrap().push(answer.clone()),
        );

        assert_eq!(
            answers.into_inner().unwrap(),
            vec![
                serde_json::json!({"jsonrpc": "2.0", "id": 7, "result": {}}),
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": "r-1",
                    "error": {"code": -32601, "message": "Method not found: roots/list"},
                }),
            ]
        );
        // Our request 7 got its own answer, not the server's ping.
        assert_eq!(rx.try_recv().unwrap()["result"]["ok"], true);
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn responses_and_notifications_are_not_requests() {
        for message in [
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": 1, "message": "x"}}),
            serde_json::json!({"jsonrpc": "2.0", "method": "notifications/progress"}),
            serde_json::json!({"jsonrpc": "2.0", "id": null, "method": "ping"}),
        ] {
            assert_eq!(answer_server_request(&message), None, "{message}");
        }
    }

    #[test]
    fn requests_waiting_on_a_server_that_exits_fail_at_once() {
        let (tx, rx) = mpsc::channel::<serde_json::Value>();
        let pending = Arc::new(Mutex::new(HashMap::from([(3, tx)])));
        let alive = AtomicBool::new(true);

        McpClient::server_gone(&alive, &pending);

        assert!(!alive.load(Ordering::SeqCst));
        assert!(pending.lock().unwrap().is_empty());
        // No 30 s wait: the sender is gone, so the wait ends now.
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn a_tool_without_a_description_is_still_listed() {
        let tool: McpToolDef = serde_json::from_value(serde_json::json!({
            "name": "now",
            "inputSchema": {"type": "object"},
        }))
        .unwrap();
        assert_eq!(tool.name, "now");
        assert_eq!(tool.description, "");
    }
}
