/// LSP Manager — auto-detects and spawns the right language server
/// for a given project type. Inspired by Lapce's plugin catalog.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::mpsc;

use super::client::{LspClient, LspEvent};
use crate::project::workspace::ProjectType;

/// Known language server configurations
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub language_ids: Vec<String>,
}

/// Manages multiple LSP clients for different languages in a workspace.
pub struct LspManager {
    clients: HashMap<String, std::sync::Arc<LspClient>>,
    workspace_root: PathBuf,
    event_tx: mpsc::UnboundedSender<LspEvent>,
}

impl LspManager {
    pub fn new(workspace_root: PathBuf, event_tx: mpsc::UnboundedSender<LspEvent>) -> Self {
        Self {
            clients: HashMap::new(),
            workspace_root,
            event_tx,
        }
    }

    /// Get the known LSP server configs for common languages
    pub fn default_configs() -> Vec<LspServerConfig> {
        vec![
            LspServerConfig {
                command: "rust-analyzer".into(),
                args: vec![],
                language_ids: vec!["rust".into()],
            },
            LspServerConfig {
                command: "pyright-langserver".into(),
                args: vec!["--stdio".into()],
                language_ids: vec!["python".into()],
            },
            LspServerConfig {
                command: "typescript-language-server".into(),
                args: vec!["--stdio".into()],
                language_ids: vec![
                    "typescript".into(),
                    "javascript".into(),
                    "typescriptreact".into(),
                    "javascriptreact".into(),
                ],
            },
            LspServerConfig {
                command: "gopls".into(),
                args: vec![],
                language_ids: vec!["go".into()],
            },
            LspServerConfig {
                command: "clangd".into(),
                args: vec![],
                language_ids: vec!["c".into(), "cpp".into()],
            },
        ]
    }

    /// Detect which language servers are available on the system
    pub fn detect_available_servers() -> Vec<LspServerConfig> {
        Self::default_configs()
            .into_iter()
            .filter(|config| {
                std::process::Command::new("which")
                    .arg(&config.command)
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Start the appropriate LSP server for a file based on its extension
    pub async fn ensure_server_for_file(&mut self, path: &Path) -> Result<(), String> {
        let language_id = Self::language_id_from_path(path);

        if self.clients.contains_key(&language_id) {
            return Ok(());
        }

        let configs = Self::detect_available_servers();
        let config = configs
            .iter()
            .find(|c| c.language_ids.contains(&language_id))
            .ok_or_else(|| format!("No LSP server available for language: {}", language_id))?;

        tracing::info!(
            "Starting LSP server '{}' for language '{}'",
            config.command,
            language_id
        );

        let client = LspClient::start(
            &config.command,
            &config.args,
            &self.workspace_root,
            self.event_tx.clone(),
        )?;

        client.initialize(&self.workspace_root).await?;

        self.clients
            .insert(language_id, std::sync::Arc::new(client));
        Ok(())
    }

    /// Get the LSP client for a given language
    pub fn client_for_language(&self, language_id: &str) -> Option<&std::sync::Arc<LspClient>> {
        self.clients.get(language_id)
    }

    /// Get the LSP client for a file based on its extension
    pub fn client_for_file(&self, path: &Path) -> Option<&std::sync::Arc<LspClient>> {
        let lang_id = Self::language_id_from_path(path);
        self.clients.get(&lang_id)
    }

    /// Same as `client_for_file` but path-based (alias for app.rs compatibility)
    pub fn client_for_path(&self, path: &Path) -> Option<&std::sync::Arc<LspClient>> {
        self.client_for_file(path)
    }

    /// Notify all relevant servers that a file was opened
    pub fn did_open(&self, path: &Path, text: &str) {
        let language_id = Self::language_id_from_path(path);
        if let Some(client) = self.clients.get(&language_id) {
            if let Err(e) = client.did_open(path, &language_id, text) {
                tracing::warn!("LSP didOpen failed: {}", e);
            }
        }
    }

    /// Notify all relevant servers that a file changed
    pub fn did_change(&self, path: &Path, version: i32, text: &str) {
        let language_id = Self::language_id_from_path(path);
        if let Some(client) = self.clients.get(&language_id) {
            if let Err(e) = client.did_change(path, version, text) {
                tracing::warn!("LSP didChange failed: {}", e);
            }
        }
    }

    /// Notify all relevant servers that a file was saved (textDocument/didSave)
    pub fn did_save(&self, path: &Path) {
        let language_id = Self::language_id_from_path(path);
        if let Some(client) = self.clients.get(&language_id) {
            if let Err(e) = client.did_save(path, None) {
                tracing::warn!("LSP didSave failed: {}", e);
            }
        }
    }

    /// Shutdown all language servers
    pub async fn shutdown_all(&mut self) {
        for (lang, arc_client) in self.clients.drain() {
            tracing::info!("Shutting down LSP server for {}", lang);
            // Try to unwrap the Arc to get exclusive access for shutdown
            match std::sync::Arc::try_unwrap(arc_client) {
                Ok(mut client) => {
                    let _ = client.shutdown().await;
                }
                Err(_) => {
                    // Other references exist; best-effort: send exit notification
                    tracing::warn!(
                        "Could not shutdown LSP client for '{}': Arc still shared",
                        lang
                    );
                }
            }
        }
    }

    /// Map file extension → LSP language ID
    pub fn language_id_from_path(path: &Path) -> String {
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => "rust".into(),
            Some("py") | Some("pyw") => "python".into(),
            Some("js") | Some("mjs") | Some("cjs") => "javascript".into(),
            Some("jsx") => "javascriptreact".into(),
            Some("ts") | Some("mts") => "typescript".into(),
            Some("tsx") => "typescriptreact".into(),
            Some("go") => "go".into(),
            Some("c") | Some("h") => "c".into(),
            Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => "cpp".into(),
            Some("java") => "java".into(),
            Some("rb") => "ruby".into(),
            Some("lua") => "lua".into(),
            Some("sh") | Some("bash") => "shellscript".into(),
            Some("json") => "json".into(),
            Some("yaml") | Some("yml") => "yaml".into(),
            Some("toml") => "toml".into(),
            Some("md") => "markdown".into(),
            Some("html") | Some("htm") => "html".into(),
            Some("css") => "css".into(),
            _ => "plaintext".into(),
        }
    }

    /// Get recommended servers based on project type
    pub fn servers_for_project(project_type: &ProjectType) -> Vec<&'static str> {
        match project_type {
            ProjectType::Rust => vec!["rust-analyzer"],
            ProjectType::Node => vec!["typescript-language-server"],
            ProjectType::Python => vec!["pyright-langserver"],
            ProjectType::Go => vec!["gopls"],
            _ => vec![],
        }
    }
}
