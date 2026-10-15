//! Built-in local AI: PhazeAI downloads a coding model that fits the machine,
//! verifies it, and runs it with llama.cpp's `llama-server` — no Ollama, LM
//! Studio or API key needed.
//!
//! - [`hardware`] — RAM / CPU / disk probe.
//! - [`catalog`] — the curated, hash-pinned model list and the fit rules.
//! - [`download`] — resumable, SHA-256-verified downloads.
//! - [`install`] — finding or installing `llama-server`.
//! - [`server`] — running `llama-server` and the process-wide server handle the
//!   `BuiltIn` provider talks to.

pub mod catalog;
pub mod download;
pub mod hardware;
pub mod install;
pub mod server;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use catalog::{recommend, CatalogEntry, Fit, FitReport, CATALOG};
pub use download::{download_model, DownloadProgress};
pub use hardware::HardwareInfo;
pub use server::{readiness_problem, ServerStatus, LOCAL_AI};

/// The `[local_ai]` section of config.toml.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalAiSettings {
    /// Catalog id of the model to run. Unset: the largest downloaded model.
    pub model: Option<String>,
    /// Explicit `llama-server` binary; skips PATH and the managed install.
    pub server_path: Option<PathBuf>,
    /// Context size override (tokens); default is the catalog entry's.
    pub ctx_size: Option<u32>,
    /// Start the server when PhazeAI starts, if the built-in provider is selected.
    pub auto_start: bool,
}

impl LocalAiSettings {
    /// Context size the built-in server runs with: the override, else the
    /// selected model's catalog value, else the catalog default.
    pub fn effective_ctx_size(&self) -> u32 {
        if let Some(ctx) = self.ctx_size {
            return ctx;
        }
        server::selected_model(self, &models_dir())
            .map(|entry| entry.ctx_size)
            .unwrap_or(catalog::DEFAULT_CTX_SIZE)
    }
}

impl Default for LocalAiSettings {
    fn default() -> Self {
        Self {
            model: None,
            server_path: None,
            ctx_size: None,
            auto_start: true,
        }
    }
}

/// Errors from the local-AI subsystem. Messages are written for end users.
#[derive(Debug, thiserror::Error)]
pub enum LocalAiError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error(
        "download is corrupt: SHA-256 {actual} does not match the expected {expected}; \
         the partial file was deleted"
    )]
    HashMismatch { expected: String, actual: String },
    #[error("download has the wrong size: got {actual} bytes, expected {expected}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("cancelled")]
    Cancelled,
    #[error(
        "not enough free disk space: need {} but only {} is free",
        human_bytes(*needed),
        human_bytes(*available)
    )]
    InsufficientDisk { needed: u64, available: u64 },
    #[error("{0}")]
    Other(String),
}

/// Root folder for everything the local-AI feature stores on disk.
///
/// Uses the *local* (non-roaming) data dir: on Windows that is
/// `%LOCALAPPDATA%\phazeai`, so multi-gigabyte models never end up in a roaming
/// profile. macOS: `~/Library/Application Support/phazeai`; Linux:
/// `$XDG_DATA_HOME/phazeai` (`~/.local/share/phazeai`).
pub fn data_root() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("phazeai")
}

/// Where downloaded GGUF models live.
pub fn models_dir() -> PathBuf {
    data_root().join("models")
}

/// Where PhazeAI installs its own copy of llama.cpp (`<root>/<release tag>/`).
pub fn llama_cpp_dir() -> PathBuf {
    data_root().join("llama.cpp")
}

/// `1.0 GB`, `512 MB`, ... (decimal units, as disk and download sizes are shown).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_uses_decimal_units() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1_117_320_768), "1.1 GB");
        assert_eq!(human_bytes(4_683_073_536), "4.7 GB");
    }

    #[test]
    fn dirs_are_under_the_data_root() {
        assert!(models_dir().starts_with(data_root()));
        assert!(llama_cpp_dir().starts_with(data_root()));
        assert!(data_root().ends_with("phazeai"));
    }
}
