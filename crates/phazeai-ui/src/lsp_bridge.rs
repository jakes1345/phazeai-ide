//! LSP bridge — runs LspManager in a background tokio thread and exposes
//! a command sender (UI → LSP) plus a reactive diagnostics signal (LSP → UI).
//!
//! **Must be started from within a Floem reactive scope** (i.e. inside the
//! window callback), because `create_signal_from_channel` and `create_effect`
//! are reactive primitives.

use std::collections::HashMap;
use std::path::PathBuf;

use floem::ext_event::create_signal_from_channel;
use floem::reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate};
use phazeai_core::{LspEvent, LspManager};
use tokio::sync::mpsc;

/// Commands sent from the UI (sync context) to the LSP background thread.
/// `UnboundedSender::send()` is safe from any thread without a tokio runtime.
#[derive(Debug)]
pub enum LspCommand {
    /// A file was opened or the active tab switched to it.
    OpenFile { path: PathBuf, text: String },
    /// File content changed — send textDocument/didChange.
    ChangeFile { path: PathBuf, text: String, version: i32 },
    /// Graceful shutdown — exits the background thread.
    Shutdown,
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A single flattened diagnostic entry ready for UI display.
#[derive(Debug, Clone)]
pub struct DiagEntry {
    pub path: PathBuf,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column.
    pub col: u32,
    pub message: String,
    pub severity: DiagSeverity,
}

/// Start the LSP bridge.
///
/// Returns `(cmd_tx, diag_signal)`:
/// - `cmd_tx` — send commands from the UI thread (sync-safe via `UnboundedSender::send`).
/// - `diag_signal` — reactive `Vec<DiagEntry>` updated on every `publishDiagnostics` event.
///
/// **Call from within a Floem reactive scope.**
pub fn start_lsp_bridge(
    workspace_root: PathBuf,
) -> (mpsc::UnboundedSender<LspCommand>, RwSignal<Vec<DiagEntry>>) {
    let (lsp_cmd_tx, mut lsp_cmd_rx) = mpsc::unbounded_channel::<LspCommand>();
    let (diag_tx, diag_rx) = std::sync::mpsc::sync_channel::<Vec<DiagEntry>>(16);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[LSP] Failed to build tokio runtime: {e}");
                return;
            }
        };

        rt.block_on(async move {
            let (event_tx, mut event_rx) =
                tokio::sync::mpsc::unbounded_channel::<LspEvent>();
            let mut manager = LspManager::new(workspace_root, event_tx);
            // uri_string → Vec<DiagEntry> so we can merge diagnostics from all files
            let mut all_diags: HashMap<String, Vec<DiagEntry>> = HashMap::new();

            loop {
                tokio::select! {
                    cmd = lsp_cmd_rx.recv() => {
                        match cmd {
                            Some(LspCommand::OpenFile { path, text }) => {
                                if let Err(e) = manager.ensure_server_for_file(&path).await {
                                    eprintln!("[LSP] no server for {}: {e}", path.display());
                                } else {
                                    manager.did_open(&path, &text);
                                }
                            }
                            Some(LspCommand::ChangeFile { path, text, version }) => {
                                manager.did_change(&path, version, &text);
                            }
                            Some(LspCommand::Shutdown) | None => break,
                        }
                    }
                    event = event_rx.recv() => {
                        match event {
                            Some(LspEvent::Diagnostics { uri, diagnostics }) => {
                                let uri_str = uri.to_string();
                                let path = uri_str
                                    .strip_prefix("file://")
                                    .map(PathBuf::from)
                                    .unwrap_or_else(|| PathBuf::from(&uri_str));

                                if diagnostics.is_empty() {
                                    all_diags.remove(&uri_str);
                                } else {
                                    let entries: Vec<DiagEntry> = diagnostics
                                        .iter()
                                        .map(|d| DiagEntry {
                                            path: path.clone(),
                                            line: d.range.start.line + 1,
                                            col: d.range.start.character + 1,
                                            message: d.message.clone(),
                                            severity: severity_from_lsp(d.severity),
                                        })
                                        .collect();
                                    all_diags.insert(uri_str, entries);
                                }

                                let flat: Vec<DiagEntry> =
                                    all_diags.values().flatten().cloned().collect();
                                let _ = diag_tx.try_send(flat);
                            }
                            Some(_) => {} // ignore completions, hover, etc.
                            None => break, // event channel closed
                        }
                    }
                }
            }
        });
    });

    // Wire std::sync receiver into Floem's reactive system.
    // create_signal_from_channel registers the receiver with Floem's event loop;
    // the signal becomes Some(Vec<DiagEntry>) each time a batch arrives.
    let chan_sig = create_signal_from_channel(diag_rx);
    let diag_sig: RwSignal<Vec<DiagEntry>> = create_rw_signal(vec![]);
    create_effect(move |_| {
        if let Some(entries) = chan_sig.get() {
            diag_sig.set(entries);
        }
    });

    (lsp_cmd_tx, diag_sig)
}

fn severity_from_lsp(s: Option<lsp_types::DiagnosticSeverity>) -> DiagSeverity {
    if s == Some(lsp_types::DiagnosticSeverity::WARNING) {
        DiagSeverity::Warning
    } else if s == Some(lsp_types::DiagnosticSeverity::INFORMATION) {
        DiagSeverity::Info
    } else if s == Some(lsp_types::DiagnosticSeverity::HINT) {
        DiagSeverity::Hint
    } else {
        DiagSeverity::Error
    }
}
