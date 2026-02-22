/// Lean LSP client that speaks the Language Server Protocol over stdio.
/// Inspired by Lapce's LSP client but simplified for PhazeAI's architecture.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use lsp_types::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// Convert a filesystem path to a file:// URI string
fn path_to_uri(path: &Path) -> Result<Uri, String> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let uri_str = format!("file://{}", abs.display());
    uri_str.parse().map_err(|e| format!("Invalid URI: {:?}", e))
}

/// Events emitted by the LSP client to the IDE
#[derive(Debug, Clone)]
pub enum LspEvent {
    /// Diagnostics (errors, warnings) for a file
    Diagnostics {
        uri: Uri,
        diagnostics: Vec<Diagnostic>,
    },
    /// Completion items ready to display
    Completions(Vec<CompletionItem>),
    /// Hover information
    Hover(Option<Hover>),
    /// Go-to-definition result
    Definition(Vec<Location>),
    /// Find references result
    References(Vec<Location>),
    /// Document formatting edits
    Formatting(Vec<TextEdit>),
    /// Server initialized successfully
    Initialized(String),
    /// Server exited
    Shutdown,
    /// Log message from server
    Log(String),
}

/// A single LSP client connected to one language server.
pub struct LspClient {
    id_counter: AtomicU64,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Value>>>>,
    server_name: String,
    child: Option<Child>,
    event_tx: mpsc::UnboundedSender<LspEvent>,
    capabilities: Arc<Mutex<Option<ServerCapabilities>>>,
}

impl LspClient {
    /// Start a language server process and connect to it via stdio.
    pub fn start(
        server_cmd: &str,
        server_args: &[String],
        workspace_root: &Path,
        event_tx: mpsc::UnboundedSender<LspEvent>,
    ) -> Result<Self, String> {
        let mut child = Command::new(server_cmd)
            .args(server_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(workspace_root)
            .spawn()
            .map_err(|e| format!("Failed to start LSP server '{}': {}", server_cmd, e))?;

        let stdin = child.stdin.take().ok_or("No stdin")?;
        let stdout = child.stdout.take().ok_or("No stdout")?;

        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(stdin)));
        let pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let server_name = server_cmd.to_string();

        let client = Self {
            id_counter: AtomicU64::new(1),
            writer: writer.clone(),
            pending: pending.clone(),
            server_name: server_name.clone(),
            child: Some(child),
            event_tx: event_tx.clone(),
            capabilities: Arc::new(Mutex::new(None)),
        };

        // Spawn reader thread to process LSP messages from stdout
        let event_tx_clone = event_tx.clone();
        let pending_clone = pending.clone();
        thread::spawn(move || {
            Self::reader_loop(stdout, event_tx_clone, pending_clone);
        });

        Ok(client)
    }

    /// Send the LSP initialize request
    #[allow(deprecated)]
    pub async fn initialize(&self, workspace_root: &Path) -> Result<(), String> {
        let root_uri = path_to_uri(workspace_root)?;

        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    completion: Some(CompletionClientCapabilities {
                        completion_item: Some(CompletionItemCapability {
                            snippet_support: Some(false),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    hover: Some(HoverClientCapabilities {
                        ..Default::default()
                    }),
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                        ..Default::default()
                    }),
                    definition: Some(GotoCapability {
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = self.send_request::<request::Initialize>(params).await?;

        // Store server capabilities
        if let Ok(mut caps) = self.capabilities.lock() {
            *caps = Some(result.capabilities);
        }

        // Send initialized notification
        self.send_notification::<notification::Initialized>(InitializedParams {})?;

        let _ = self
            .event_tx
            .send(LspEvent::Initialized(self.server_name.clone()));
        Ok(())
    }

    /// Notify the server that a file was opened
    pub fn did_open(&self, path: &Path, language_id: &str, text: &str) -> Result<(), String> {
        let uri = path_to_uri(path)?;
        self.send_notification::<notification::DidOpenTextDocument>(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: language_id.to_string(),
                version: 0,
                text: text.to_string(),
            },
        })
    }

    /// Notify the server that a file changed
    pub fn did_change(&self, path: &Path, version: i32, text: &str) -> Result<(), String> {
        let uri = path_to_uri(path)?;
        self.send_notification::<notification::DidChangeTextDocument>(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri, version },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_string(),
            }],
        })
    }

    /// Notify the server that a file was saved
    pub fn did_save(&self, path: &Path, text: Option<&str>) -> Result<(), String> {
        let uri = path_to_uri(path)?;
        self.send_notification::<notification::DidSaveTextDocument>(DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
            text: text.map(|t| t.to_string()),
        })
    }

    /// Request completions at a position
    pub async fn completion(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<CompletionItem>, String> {
        let uri = path_to_uri(path)?;
        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            context: None,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let result = self.send_request::<request::Completion>(params).await?;
        match result {
            Some(CompletionResponse::Array(items)) => Ok(items),
            Some(CompletionResponse::List(list)) => Ok(list.items),
            None => Ok(vec![]),
        }
    }

    /// Request hover info at a position
    pub async fn hover(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<Hover>, String> {
        let uri = path_to_uri(path)?;
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
        };
        self.send_request::<request::HoverRequest>(params).await
    }

    /// Request go-to-definition at a position
    pub async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>, String> {
        let uri = path_to_uri(path)?;
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let result = self.send_request::<request::GotoDefinition>(params).await?;
        match result {
            Some(GotoDefinitionResponse::Scalar(loc)) => Ok(vec![loc]),
            Some(GotoDefinitionResponse::Array(locs)) => Ok(locs),
            Some(GotoDefinitionResponse::Link(_links)) => Ok(vec![]),
            None => Ok(vec![]),
        }
    }

    /// Request find references at a position
    pub async fn find_references(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>, String> {
        let uri = path_to_uri(path)?;
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position { line, character },
            },
            context: ReferenceContext {
                include_declaration: true,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let result = self.send_request::<request::References>(params).await?;
        Ok(result.unwrap_or_default())
    }

    /// Request document formatting
    pub async fn formatting(
        &self,
        path: &Path,
        tab_size: u32,
        insert_spaces: bool,
    ) -> Result<Vec<TextEdit>, String> {
        let uri = path_to_uri(path)?;
        let params = DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri },
            options: FormattingOptions {
                tab_size,
                insert_spaces,
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
        };
        let result = self.send_request::<request::Formatting>(params).await?;
        Ok(result.unwrap_or_default())
    }

    // ── Event sender helpers (used by app.rs after async requests) ──────────

    /// Send a hover result back through the event channel.
    pub fn send_hover_event(&self, hover: Option<lsp_types::Hover>) -> Result<(), String> {
        self.event_tx
            .send(LspEvent::Hover(hover))
            .map_err(|e| e.to_string())
    }

    /// Send definition locations back through the event channel.
    pub fn send_definition_event(&self, locations: Vec<lsp_types::Location>) -> Result<(), String> {
        self.event_tx
            .send(LspEvent::Definition(locations))
            .map_err(|e| e.to_string())
    }

    /// Send completion items back through the event channel.
    pub fn send_completions_event(
        &self,
        items: Vec<lsp_types::CompletionItem>,
    ) -> Result<(), String> {
        self.event_tx
            .send(LspEvent::Completions(items))
            .map_err(|e| e.to_string())
    }

    /// Send references back through the event channel.
    pub fn send_references_event(&self, locations: Vec<lsp_types::Location>) -> Result<(), String> {
        self.event_tx
            .send(LspEvent::References(locations))
            .map_err(|e| e.to_string())
    }

    /// Send formatting edits back through the event channel.
    pub fn send_formatting_event(&self, edits: Vec<lsp_types::TextEdit>) -> Result<(), String> {
        self.event_tx
            .send(LspEvent::Formatting(edits))
            .map_err(|e| e.to_string())
    }

    /// Shutdown the language server
    pub async fn shutdown(&mut self) -> Result<(), String> {
        let _ = self.send_request::<request::Shutdown>(()).await;
        self.send_notification::<notification::Exit>(())?;
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
        }
        let _ = self.event_tx.send(LspEvent::Shutdown);
        Ok(())
    }

    // ── Internal protocol methods ──────────────────────────────────

    /// Send a JSON-RPC request and wait for the response
    async fn send_request<R: request::Request>(
        &self,
        params: R::Params,
    ) -> Result<R::Result, String>
    where
        R::Params: Serialize,
        R::Result: for<'de> Deserialize<'de>,
    {
        let id = self.id_counter.fetch_add(1, Ordering::Relaxed);

        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": R::METHOD,
            "params": serde_json::to_value(&params).map_err(|e| e.to_string())?,
        });

        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut pending = self.pending.lock().map_err(|e| e.to_string())?;
            pending.insert(id, tx);
        }

        self.write_message(&msg)?;

        let result = rx.await.map_err(|_| "LSP response channel closed")?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse LSP response: {}", e))
    }

    /// Send a JSON-RPC notification (no response expected)
    fn send_notification<N: notification::Notification>(
        &self,
        params: N::Params,
    ) -> Result<(), String>
    where
        N::Params: Serialize,
    {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": N::METHOD,
            "params": serde_json::to_value(&params).map_err(|e| e.to_string())?,
        });
        self.write_message(&msg)
    }

    /// Write an LSP message (Content-Length header + JSON body)
    fn write_message(&self, msg: &Value) -> Result<(), String> {
        let body = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        let mut writer = self.writer.lock().map_err(|e| e.to_string())?;
        writer
            .write_all(header.as_bytes())
            .map_err(|e| e.to_string())?;
        writer
            .write_all(body.as_bytes())
            .map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Read loop: parse LSP messages from stdout and dispatch them
    fn reader_loop(
        stdout: impl Read + Send + 'static,
        event_tx: mpsc::UnboundedSender<LspEvent>,
        pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Value>>>>,
    ) {
        let mut reader = BufReader::new(stdout);

        while let Ok(content_length) = Self::read_content_length(&mut reader) {
            // Read the JSON body
            let mut body = vec![0u8; content_length];
            if reader.read_exact(&mut body).is_err() {
                break;
            }

            let body_str = match String::from_utf8(body) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let msg: Value = match serde_json::from_str(&body_str) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Dispatch: response or notification?
            if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                // It's a response to one of our requests
                if let Some(result) = msg.get("result") {
                    if let Ok(mut pending) = pending.lock() {
                        if let Some(tx) = pending.remove(&id) {
                            let _ = tx.send(result.clone());
                        }
                    }
                } else if let Some(error) = msg.get("error") {
                    tracing::warn!("LSP error for request {}: {:?}", id, error);
                    if let Ok(mut pending) = pending.lock() {
                        if let Some(tx) = pending.remove(&id) {
                            let _ = tx.send(Value::Null);
                        }
                    }
                }
            } else if let Some(method) = msg.get("method").and_then(|v| v.as_str()) {
                // It's a server notification
                match method {
                    "textDocument/publishDiagnostics" => {
                        if let Some(params) = msg.get("params") {
                            if let Ok(diag_params) =
                                serde_json::from_value::<PublishDiagnosticsParams>(params.clone())
                            {
                                let _ = event_tx.send(LspEvent::Diagnostics {
                                    uri: diag_params.uri,
                                    diagnostics: diag_params.diagnostics,
                                });
                            }
                        }
                    }
                    "window/logMessage" => {
                        if let Some(msg_str) = msg
                            .get("params")
                            .and_then(|p| p.get("message"))
                            .and_then(|m| m.as_str())
                        {
                            let _ = event_tx.send(LspEvent::Log(msg_str.to_string()));
                        }
                    }
                    _ => {
                        tracing::debug!("Unhandled LSP notification: {}", method);
                    }
                }
            }
        }
    }

    /// Parse the Content-Length header from LSP message stream
    fn read_content_length(reader: &mut impl BufRead) -> Result<usize, String> {
        let mut content_length: Option<usize> = None;

        loop {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|e| format!("Read error: {}", e))?;

            let line = line.trim();
            if line.is_empty() {
                break;
            }

            if let Some(len_str) = line.strip_prefix("Content-Length: ") {
                content_length = len_str.parse().ok();
            }
        }

        content_length.ok_or_else(|| "No Content-Length header".to_string())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
        }
    }
}
