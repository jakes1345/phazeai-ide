//! LSP bridge — runs LspManager in a background tokio thread and exposes:
//! - a command sender (UI → LSP, sync-safe)
//! - a reactive diagnostics signal (LSP → UI, all open files merged)
//! - a reactive completions signal (LSP → UI, latest completion list)
//!
//! **Must be started from within a Floem reactive scope** (window callback),
//! because `create_signal_from_channel` and `create_effect` are reactive.

use std::collections::HashMap;
use std::path::PathBuf;

use floem::ext_event::create_signal_from_channel;
use floem::reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate};
use phazeai_core::{LspEvent, LspManager};
use tokio::sync::mpsc;

// ── Public types ──────────────────────────────────────────────────────────────

/// Commands sent from the UI (sync) to the LSP background thread.
/// `UnboundedSender::send()` is safe from any thread without a runtime.
#[derive(Debug)]
pub enum LspCommand {
    /// File was opened / active tab changed — send textDocument/didOpen.
    OpenFile { path: PathBuf, text: String },
    /// File content changed — debounced 300 ms before forwarding did_change.
    ChangeFile { path: PathBuf, text: String, version: i32 },
    /// Request completions at a cursor position — triggers Completions event.
    RequestCompletions { path: PathBuf, line: u32, col: u32 },
    /// Graceful shutdown.
    Shutdown,
}

/// Diagnostic severity (mirrors LSP spec without pulling in lsp-types at call sites).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A single diagnostic entry, flattened for UI display.
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

/// A single completion item, simplified from lsp_types::CompletionItem.
#[derive(Debug, Clone)]
pub struct CompletionEntry {
    /// The text shown in the popup (method/field/keyword name).
    pub label: String,
    /// The text to insert (may include snippets; falls back to label).
    pub insert_text: String,
    /// Optional short description shown next to the label.
    pub detail: Option<String>,
}

// ── Bridge entry point ────────────────────────────────────────────────────────

/// Start the LSP bridge.
///
/// Returns `(cmd_tx, diag_signal, completions_signal)`.
///
/// - `cmd_tx` — sync-safe sender from the UI thread.
/// - `diag_signal` — updated on every `publishDiagnostics` (all files merged).
/// - `completions_signal` — updated after each `RequestCompletions` response.
///
/// **Call from within a Floem reactive scope.**
pub fn start_lsp_bridge(
    workspace_root: PathBuf,
) -> (
    mpsc::UnboundedSender<LspCommand>,
    RwSignal<Vec<DiagEntry>>,
    RwSignal<Vec<CompletionEntry>>,
) {
    let (lsp_cmd_tx, mut lsp_cmd_rx) = mpsc::unbounded_channel::<LspCommand>();

    // Diagnostics: bridge → Floem (sync_channel consumed by create_signal_from_channel)
    let (diag_tx, diag_rx) = std::sync::mpsc::sync_channel::<Vec<DiagEntry>>(16);
    // Completions: bridge → Floem
    let (comp_tx, comp_rx) = std::sync::mpsc::sync_channel::<Vec<CompletionEntry>>(8);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[LSP] Failed to build runtime: {e}");
                return;
            }
        };

        rt.block_on(async move {
            let (event_tx, mut event_rx) =
                tokio::sync::mpsc::unbounded_channel::<LspEvent>();
            let mut manager = LspManager::new(workspace_root, event_tx.clone());

            // uri → diagnostics (merged across all open files)
            let mut all_diags: HashMap<String, Vec<DiagEntry>> = HashMap::new();

            // Debounce state for ChangeFile: latest pending change + deadline.
            // The `sleep_until` arm only fires when `pending_change.is_some()`.
            let debounce_ms = tokio::time::Duration::from_millis(300);
            let far_future   = tokio::time::Instant::now() + tokio::time::Duration::from_secs(86400);
            let mut pending_change: Option<(PathBuf, String, i32)> = None;
            let mut change_deadline = far_future;

            loop {
                tokio::select! {
                    // ── Incoming command from the UI ─────────────────────────
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
                                // Buffer and debounce — reset deadline on every keystroke.
                                pending_change = Some((path, text, version));
                                change_deadline = tokio::time::Instant::now() + debounce_ms;
                            }
                            Some(LspCommand::RequestCompletions { path, line, col }) => {
                                if let Some(client) = manager.client_for_file(&path).cloned() {
                                    let path2    = path.clone();
                                    let evt_tx   = event_tx.clone();
                                    tokio::spawn(async move {
                                        match client.completion(&path2, line, col).await {
                                            Ok(items) => {
                                                let _ = evt_tx.send(LspEvent::Completions(items));
                                            }
                                            Err(e) => eprintln!("[LSP] completion error: {e}"),
                                        }
                                    });
                                }
                            }
                            Some(LspCommand::Shutdown) | None => break,
                        }
                    }

                    // ── Debounce flush: forward buffered ChangeFile ──────────
                    _ = tokio::time::sleep_until(change_deadline), if pending_change.is_some() => {
                        if let Some((path, text, version)) = pending_change.take() {
                            manager.did_change(&path, version, &text);
                        }
                        change_deadline = far_future; // reset timer to idle
                    }

                    // ── LSP server event ─────────────────────────────────────
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
                                    let entries = diagnostics.iter().map(|d| DiagEntry {
                                        path: path.clone(),
                                        line: d.range.start.line + 1,
                                        col:  d.range.start.character + 1,
                                        message:  d.message.clone(),
                                        severity: severity_from_lsp(d.severity),
                                    }).collect();
                                    all_diags.insert(uri_str, entries);
                                }

                                let flat: Vec<DiagEntry> =
                                    all_diags.values().flatten().cloned().collect();
                                let _ = diag_tx.try_send(flat);
                            }

                            Some(LspEvent::Completions(items)) => {
                                let entries: Vec<CompletionEntry> = items.iter().map(|item| {
                                    // Prefer TextEdit text, then insert_text, then label.
                                    let insert_text = item.insert_text.clone()
                                        .or_else(|| item.text_edit.as_ref().and_then(|te| {
                                            use lsp_types::CompletionTextEdit;
                                            match te {
                                                CompletionTextEdit::Edit(e) =>
                                                    Some(e.new_text.clone()),
                                                CompletionTextEdit::InsertAndReplace(e) =>
                                                    Some(e.new_text.clone()),
                                            }
                                        }))
                                        .unwrap_or_else(|| item.label.clone());

                                    CompletionEntry {
                                        label:       item.label.clone(),
                                        insert_text,
                                        detail:      item.detail.clone(),
                                    }
                                }).collect();
                                let _ = comp_tx.try_send(entries);
                            }

                            Some(_) => {} // hover, definition, references — ignored for now
                            None => break, // event channel closed
                        }
                    }
                }
            }
        });
    });

    // Wire both std::sync channels into Floem's reactive system.
    let diag_chan  = create_signal_from_channel(diag_rx);
    let comp_chan  = create_signal_from_channel(comp_rx);

    let diag_sig: RwSignal<Vec<DiagEntry>>        = create_rw_signal(vec![]);
    let comp_sig: RwSignal<Vec<CompletionEntry>>  = create_rw_signal(vec![]);

    create_effect(move |_| {
        if let Some(entries) = diag_chan.get() { diag_sig.set(entries); }
    });
    create_effect(move |_| {
        if let Some(entries) = comp_chan.get() { comp_sig.set(entries); }
    });

    (lsp_cmd_tx, diag_sig, comp_sig)
}

// ── Helper ────────────────────────────────────────────────────────────────────

fn severity_from_lsp(s: Option<lsp_types::DiagnosticSeverity>) -> DiagSeverity {
    if      s == Some(lsp_types::DiagnosticSeverity::WARNING)     { DiagSeverity::Warning }
    else if s == Some(lsp_types::DiagnosticSeverity::INFORMATION) { DiagSeverity::Info    }
    else if s == Some(lsp_types::DiagnosticSeverity::HINT)        { DiagSeverity::Hint    }
    else                                                           { DiagSeverity::Error   }
}
