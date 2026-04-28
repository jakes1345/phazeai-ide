/// LSP Manager — auto-detects and spawns the right language server
/// for a given project type. Inspired by Lapce's plugin catalog.
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::client::{LspClient, LspEvent};
use crate::project::workspace::ProjectType;

const MAX_RESTARTS_PER_WINDOW: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Known language server configurations
#[derive(Debug, Clone)]
pub struct LspServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub language_ids: Vec<String>,
}

/// Cached open document — kept by the manager so a restarted server can be
/// brought back to the same `did_open` state.
#[derive(Clone)]
struct OpenDoc {
    language_id: String,
    text: String,
    version: i32,
}

/// Manages multiple LSP clients for different languages in a workspace.
pub struct LspManager {
    clients: HashMap<String, std::sync::Arc<LspClient>>,
    workspace_root: PathBuf,
    event_tx: mpsc::UnboundedSender<LspEvent>,
    /// Last-known text+version per open path so we can replay did_open after restart.
    open_docs: HashMap<PathBuf, OpenDoc>,
    /// Restart timestamps per language for rate-cap.
    restart_history: HashMap<String, VecDeque<Instant>>,
    /// Languages we've given up on after exceeding the restart cap.
    blocked: HashMap<String, Instant>,
}

impl LspManager {
    pub fn new(workspace_root: PathBuf, event_tx: mpsc::UnboundedSender<LspEvent>) -> Self {
        Self {
            clients: HashMap::new(),
            workspace_root,
            event_tx,
            open_docs: HashMap::new(),
            restart_history: HashMap::new(),
            blocked: HashMap::new(),
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
    pub fn did_open(&mut self, path: &Path, text: &str) {
        let language_id = Self::language_id_from_path(path);
        self.open_docs.insert(
            path.to_path_buf(),
            OpenDoc {
                language_id: language_id.clone(),
                text: text.to_string(),
                version: 0,
            },
        );
        if let Some(client) = self.clients.get(&language_id) {
            if let Err(e) = client.did_open(path, &language_id, text) {
                tracing::warn!("LSP didOpen failed: {}", e);
            }
        }
    }

    /// Notify all relevant servers that a file changed
    pub fn did_change(&mut self, path: &Path, version: i32, text: &str) {
        let language_id = Self::language_id_from_path(path);
        if let Some(doc) = self.open_docs.get_mut(path) {
            doc.text = text.to_string();
            doc.version = version;
        }
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

    /// Inspect every connected client; for any whose process has died, drop it
    /// and respawn (rate-limited), then re-send `did_open` for that language's
    /// tracked documents. Intended to be called periodically by the UI.
    pub async fn health_check(&mut self) {
        let dead: Vec<String> = self
            .clients
            .iter()
            .filter(|(_, client)| !client.is_alive())
            .map(|(lang, _)| lang.clone())
            .collect();

        for lang in dead {
            tracing::warn!(
                "LSP server for '{lang}' died; attempting restart",
                lang = lang
            );
            self.clients.remove(&lang);

            if !self.allow_restart(&lang) {
                tracing::error!(
                    "LSP server for '{lang}' exceeded {} restarts in {}s; giving up",
                    MAX_RESTARTS_PER_WINDOW,
                    RESTART_WINDOW.as_secs()
                );
                self.blocked.insert(lang.clone(), Instant::now());
                continue;
            }

            if let Err(e) = self.spawn_for_language(&lang).await {
                tracing::error!("Failed to restart LSP for '{lang}': {e}");
                continue;
            }

            // Replay open docs for this language so the new server has state.
            let docs: Vec<(PathBuf, OpenDoc)> = self
                .open_docs
                .iter()
                .filter(|(_, d)| d.language_id == lang)
                .map(|(p, d)| (p.clone(), d.clone()))
                .collect();
            if let Some(client) = self.clients.get(&lang) {
                for (path, doc) in docs {
                    if let Err(e) = client.did_open(&path, &doc.language_id, &doc.text) {
                        tracing::warn!("Replay didOpen failed for {}: {e}", path.display());
                    }
                    if doc.version > 0 {
                        if let Err(e) = client.did_change(&path, doc.version, &doc.text) {
                            tracing::warn!("Replay didChange failed for {}: {e}", path.display());
                        }
                    }
                }
            }
        }
    }

    fn allow_restart(&mut self, language_id: &str) -> bool {
        let now = Instant::now();
        let history = self
            .restart_history
            .entry(language_id.to_string())
            .or_default();
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

    async fn spawn_for_language(&mut self, language_id: &str) -> Result<(), String> {
        let configs = Self::detect_available_servers();
        let config = configs
            .iter()
            .find(|c| c.language_ids.iter().any(|l| l == language_id))
            .ok_or_else(|| format!("No LSP server available for language: {language_id}"))?;

        tracing::info!(
            "Restarting LSP server '{}' for '{language_id}'",
            config.command
        );

        let client = LspClient::start(
            &config.command,
            &config.args,
            &self.workspace_root,
            self.event_tx.clone(),
        )?;
        client.initialize(&self.workspace_root).await?;
        self.clients
            .insert(language_id.to_string(), std::sync::Arc::new(client));
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_manager() -> LspManager {
        let (tx, _rx) = mpsc::unbounded_channel();
        LspManager::new(PathBuf::from("/tmp"), tx)
    }

    #[test]
    fn allow_restart_caps_at_three_per_window() {
        let mut m = empty_manager();
        // First three are allowed, fourth is denied within the same window.
        assert!(m.allow_restart("rust"));
        assert!(m.allow_restart("rust"));
        assert!(m.allow_restart("rust"));
        assert!(!m.allow_restart("rust"));
    }

    #[test]
    fn allow_restart_is_per_language() {
        let mut m = empty_manager();
        // Burning the rust budget should not affect python.
        for _ in 0..MAX_RESTARTS_PER_WINDOW {
            assert!(m.allow_restart("rust"));
        }
        assert!(!m.allow_restart("rust"));
        assert!(m.allow_restart("python"));
    }

    #[test]
    fn document_cache_tracks_open_and_change() {
        let mut m = empty_manager();
        let path = PathBuf::from("/tmp/foo.rs");
        m.did_open(&path, "fn a(){}");
        let entry = m.open_docs.get(&path).expect("did_open should cache");
        assert_eq!(entry.language_id, "rust");
        assert_eq!(entry.text, "fn a(){}");
        assert_eq!(entry.version, 0);

        m.did_change(&path, 4, "fn a(){ b(); }");
        let entry = m
            .open_docs
            .get(&path)
            .expect("did_change should keep cache");
        assert_eq!(entry.text, "fn a(){ b(); }");
        assert_eq!(entry.version, 4);
    }
}
