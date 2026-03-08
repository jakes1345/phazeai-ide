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
    ChangeFile {
        path: PathBuf,
        text: String,
        version: i32,
    },
    /// Request completions at a cursor position — triggers Completions event.
    RequestCompletions { path: PathBuf, line: u32, col: u32 },
    /// Request go-to-definition at cursor position.
    RequestDefinition { path: PathBuf, line: u32, col: u32 },
    /// Request hover documentation at cursor position.
    RequestHover { path: PathBuf, line: u32, col: u32 },
    /// Request signature help (textDocument/signatureHelp) at cursor position.
    RequestSignatureHelp { path: PathBuf, line: u32, col: u32 },
    /// Request all references at cursor position (Shift+F12).
    RequestReferences { path: PathBuf, line: u32, col: u32 },
    /// Request code actions / quick-fixes at cursor position (Ctrl+.).
    RequestCodeActions { path: PathBuf, line: u32, col: u32 },
    /// Rename the symbol under cursor across the workspace (F2).
    RequestRename {
        path: PathBuf,
        line: u32,
        col: u32,
        new_name: String,
        workspace_root: PathBuf,
    },
    /// Request all symbols in the current document (outline, Ctrl+Shift+O).
    RequestDocumentSymbols { path: PathBuf },
    /// File was saved — send textDocument/didSave notification to LSP server.
    SaveFile { path: PathBuf },
    /// Request workspace-wide symbol search (Ctrl+T). Query is the filter string.
    RequestWorkspaceSymbols { query: String },
    /// Request peek definition (Alt+F12) — like go-to-def but returns source lines.
    RequestPeekDefinition { path: PathBuf, line: u32, col: u32 },
    /// Request code lens for the current file (textDocument/codeLens).
    RequestCodeLens { path: PathBuf },
    /// Request go-to-implementation at cursor position (Ctrl+F12).
    RequestImplementation { path: PathBuf, line: u32, col: u32 },
    /// Request LSP folding ranges for the current file (textDocument/foldingRange).
    RequestFoldingRanges { path: PathBuf },
    /// Organize imports on save using LSP textDocument/codeAction source.organizeImports.
    OrganizeImports { path: PathBuf },
    /// Request inlay hints for a visible range of a file (textDocument/inlayHint).
    RequestInlayHints { path: PathBuf, start_line: u32, end_line: u32 },
    /// Graceful shutdown.
    Shutdown,
}

/// An inlay hint (type annotation, parameter name, etc.) for inline display.
#[derive(Debug, Clone)]
pub struct InlayHintEntry {
    /// 0-based line the hint appears on.
    pub line: u32,
    /// 0-based column (byte offset within the line) after which the hint is shown.
    pub col: u32,
    /// Text to display, e.g. ": i32" or "name: ".
    pub label: String,
}

/// A code lens entry attached to a specific line.
#[derive(Debug, Clone)]
pub struct CodeLensEntry {
    /// 1-based line number the lens appears on.
    pub line: u32,
    /// Display label (e.g. "2 references", "Run test").
    pub label: String,
}

/// A symbol entry from the document symbol outline.
#[derive(Debug, Clone)]
pub struct SymbolEntry {
    pub name: String,
    pub kind: String, // "fn", "struct", "impl", "trait", "mod", etc.
    /// 1-based line number.
    pub line: u32,
    /// Nesting depth (0 = top-level).
    pub depth: u32,
}

/// Parsed signature help result returned by the LSP server.
#[derive(Debug, Clone)]
pub struct SignatureHelpResult {
    /// The full label of the active signature (e.g. `fn foo(a: i32, b: &str)`).
    pub label: String,
    /// Index of the currently-active parameter (0-based).
    pub active_param: usize,
    /// Labels of individual parameters extracted from the signature.
    pub params: Vec<String>,
}

/// A go-to-definition result (first location only; LSP may return multiple).
#[derive(Debug, Clone)]
pub struct DefinitionResult {
    pub path: PathBuf,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column.
    pub col: u32,
}

/// A single find-references result entry.
#[derive(Debug, Clone)]
pub struct ReferenceEntry {
    pub path: PathBuf,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column.
    pub col: u32,
}

/// A code action / quick-fix offered by the LSP server (or generated locally).
#[derive(Debug, Clone)]
pub struct CodeAction {
    pub title: String,
    pub kind: String,
    /// Edits to apply: list of `(file_path, new_full_content)`.
    /// Empty means the action is handled procedurally (e.g. "Format Document").
    pub edit: Option<Vec<(PathBuf, String)>>,
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
/// Returns a 14-tuple: `(cmd_tx, diag_sig, comp_sig, def_sig, hover_sig, refs_sig, actions_sig,
/// sig_help_sig, doc_syms_sig, ws_syms_sig, lsp_progress_sig, peek_def_lines_sig, code_lens_sig,
/// folding_ranges_sig)`.
///
/// **Call from within a Floem reactive scope.**
pub fn start_lsp_bridge(
    workspace_root: PathBuf,
) -> (
    mpsc::UnboundedSender<LspCommand>,
    RwSignal<Vec<DiagEntry>>,
    RwSignal<Vec<CompletionEntry>>,
    RwSignal<Option<DefinitionResult>>,
    RwSignal<Option<String>>,
    RwSignal<Vec<ReferenceEntry>>,
    RwSignal<Vec<CodeAction>>,
    RwSignal<Option<SignatureHelpResult>>,
    RwSignal<Vec<SymbolEntry>>,
    RwSignal<Vec<SymbolEntry>>,
    RwSignal<Option<String>>,
    RwSignal<Vec<String>>,
    RwSignal<Vec<CodeLensEntry>>,
    RwSignal<Vec<(u32, u32)>>,
    RwSignal<Vec<InlayHintEntry>>,
) {
    let (lsp_cmd_tx, mut lsp_cmd_rx) = mpsc::unbounded_channel::<LspCommand>();

    // Diagnostics: bridge → Floem (sync_channel consumed by create_signal_from_channel)
    let (diag_tx, diag_rx) = std::sync::mpsc::sync_channel::<Vec<DiagEntry>>(16);
    // Completions: bridge → Floem
    let (comp_tx, comp_rx) = std::sync::mpsc::sync_channel::<Vec<CompletionEntry>>(8);
    // Definition: bridge → Floem
    let (def_tx, def_rx) = std::sync::mpsc::sync_channel::<DefinitionResult>(4);
    // Hover: bridge → Floem
    let (hover_tx, hover_rx) = std::sync::mpsc::sync_channel::<String>(4);
    // References: bridge → Floem
    let (refs_tx, refs_rx) = std::sync::mpsc::sync_channel::<Vec<ReferenceEntry>>(4);
    // Code actions: bridge → Floem
    let (actions_tx, actions_rx) = std::sync::mpsc::sync_channel::<Vec<CodeAction>>(4);
    // Signature help: bridge → Floem
    let (sig_tx, sig_rx) = std::sync::mpsc::sync_channel::<SignatureHelpResult>(4);
    // Document symbols: bridge → Floem
    let (syms_tx, syms_rx) = std::sync::mpsc::sync_channel::<Vec<SymbolEntry>>(4);
    // Workspace symbols: bridge → Floem
    let (ws_syms_tx, ws_syms_rx) = std::sync::mpsc::sync_channel::<Vec<SymbolEntry>>(4);
    // LSP progress: bridge → Floem (None = idle, Some("msg") = in progress)
    let (progress_tx, progress_rx) = std::sync::mpsc::sync_channel::<Option<String>>(8);
    // Peek definition lines: bridge → Floem
    let (peek_tx, peek_rx) = std::sync::mpsc::sync_channel::<Vec<String>>(4);
    // Code lens entries: bridge → Floem
    let (code_lens_tx, code_lens_rx) = std::sync::mpsc::sync_channel::<Vec<CodeLensEntry>>(4);
    // Folding ranges: bridge → Floem (start_line, end_line pairs, 0-based)
    let (fold_ranges_tx, fold_ranges_rx) = std::sync::mpsc::sync_channel::<Vec<(u32, u32)>>(4);
    // Inlay hints: bridge → Floem
    let (inlay_tx, inlay_rx) = std::sync::mpsc::sync_channel::<Vec<InlayHintEntry>>(4);

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
            let ws_root_for_refs = workspace_root.clone();
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
                            Some(LspCommand::RequestDefinition { path, line, col }) => {
                                if let Some(client) = manager.client_for_file(&path).cloned() {
                                    let path2  = path.clone();
                                    let evt_tx = event_tx.clone();
                                    tokio::spawn(async move {
                                        match client.goto_definition(&path2, line, col).await {
                                            Ok(locs) => {
                                                let _ = evt_tx.send(LspEvent::Definition(locs));
                                            }
                                            Err(e) => eprintln!("[LSP] definition error: {e}"),
                                        }
                                    });
                                }
                            }
                            Some(LspCommand::RequestHover { path, line, col }) => {
                                if let Some(client) = manager.client_for_file(&path).cloned() {
                                    let path2  = path.clone();
                                    let evt_tx = event_tx.clone();
                                    tokio::spawn(async move {
                                        match client.hover(&path2, line, col).await {
                                            Ok(Some(hover)) => {
                                                let _ = evt_tx.send(LspEvent::Hover(Some(hover)));
                                            }
                                            Ok(None) => {}
                                            Err(e) => eprintln!("[LSP] hover error: {e}"),
                                        }
                                    });
                                }
                            }
                            Some(LspCommand::RequestSignatureHelp { path, line, col }) => {
                                if let Some(client) = manager.client_for_file(&path).cloned() {
                                    let path2   = path.clone();
                                    let sig_tx2 = sig_tx.clone();
                                    tokio::spawn(async move {
                                        match client.signature_help(&path2, line, col).await {
                                            Ok(Some(sh)) => {
                                                if let Some(result) = parse_signature_help(sh) {
                                                    let _ = sig_tx2.try_send(result);
                                                }
                                            }
                                            Ok(None) => {}
                                            Err(e) => eprintln!("[LSP] signature_help error: {e}"),
                                        }
                                    });
                                }
                            }
                            Some(LspCommand::RequestReferences { path, line, col }) => {
                                // Try LSP first, fall back to ripgrep word-search.
                                if let Some(client) = manager.client_for_file(&path).cloned() {
                                    let path2      = path.clone();
                                    let evt_tx     = event_tx.clone();
                                    let ws_root2   = ws_root_for_refs.clone();
                                    tokio::spawn(async move {
                                        match client.find_references(&path2, line, col).await {
                                            Ok(locs) if !locs.is_empty() => {
                                                let _ = evt_tx.send(LspEvent::References(locs));
                                            }
                                            _ => {
                                                // Fallback: ripgrep word at cursor
                                                let entries = ripgrep_references(&path2, line, col, &ws_root2);
                                                let _ = evt_tx.send(LspEvent::References(
                                                    entries.into_iter().map(|e| {
                                                        use lsp_types::{Location, Range, Position};
                                                        let uri_str = format!("file://{}", e.path.display());
                                                        Location {
                                                            uri: uri_str.parse().unwrap_or_else(|_| {
                                                                "file:///unknown".parse().unwrap()
                                                            }),
                                                            range: Range {
                                                                start: Position { line: e.line.saturating_sub(1), character: e.col.saturating_sub(1) },
                                                                end:   Position { line: e.line.saturating_sub(1), character: e.col.saturating_sub(1) },
                                                            },
                                                        }
                                                    }).collect()
                                                ));
                                            }
                                        }
                                    });
                                } else {
                                    // No LSP server — use ripgrep directly
                                    let path2    = path.clone();
                                    let refs_tx2 = refs_tx.clone();
                                    let ws_root2 = ws_root_for_refs.clone();
                                    tokio::spawn(async move {
                                        let entries = ripgrep_references(&path2, line, col, &ws_root2);
                                        let _ = refs_tx2.send(entries);
                                    });
                                }
                            }
                            Some(LspCommand::RequestRename { path, line, col, new_name, workspace_root: ws }) => {
                                // Try LSP workspace/rename; fall back to project-wide text replace.
                                let did_lsp = if let Some(client) = manager.client_for_file(&path).cloned() {
                                    let path2     = path.clone();
                                    let new_name2 = new_name.clone();
                                    let old_word2 = match word_at_position(&path, line, col) {
                                        Some(w) => w,
                                        None    => String::new(),
                                    };
                                    // Ask LSP for workspace edits; apply by rewriting files directly.
                                    match client.rename_symbol(&path2, line, col, new_name2).await {
                                        Ok(Some(workspace_edit)) => {
                                            apply_workspace_edit(workspace_edit, &old_word2, &new_name);
                                            true
                                        }
                                        _ => false,
                                    }
                                } else { false };

                                if !did_lsp {
                                    // Fallback: ripgrep-based whole-word replace across workspace
                                    let old_word = match word_at_position(&path, line, col) {
                                        Some(w) => w,
                                        None => continue,
                                    };
                                    let refs = ripgrep_references(&path, line, col, &ws);
                                    // Collect unique file paths
                                    let mut files: Vec<PathBuf> = refs.iter().map(|r| r.path.clone()).collect();
                                    files.sort(); files.dedup();
                                    for file_path in files {
                                        if let Ok(content) = std::fs::read_to_string(&file_path) {
                                            // Replace whole-word occurrences
                                            let new_content = replace_whole_word(&content, &old_word, &new_name);
                                            if new_content != content {
                                                let _ = std::fs::write(&file_path, new_content);
                                            }
                                        }
                                    }
                                }
                            }
                            Some(LspCommand::RequestCodeActions { path, line, col }) => {
                                // Generate quick-fix suggestions locally (no LSP codeAction yet).
                                let actions_tx2 = actions_tx.clone();
                                let path2 = path.clone();
                                tokio::spawn(async move {
                                    let actions = generate_code_actions(&path2, line, col);
                                    let _ = actions_tx2.send(actions);
                                });
                            }
                            Some(LspCommand::RequestDocumentSymbols { path }) => {
                                let syms_tx2  = syms_tx.clone();
                                let path2     = path.clone();
                                let client_opt = manager.client_for_file(&path).cloned();
                                tokio::spawn(async move {
                                    if let Some(client) = client_opt {
                                        match client.document_symbols(&path2).await {
                                            Ok(syms) if !syms.is_empty() => {
                                                let entries = flatten_symbols(&syms, 0);
                                                let _ = syms_tx2.send(entries);
                                                return;
                                            }
                                            _ => {}
                                        }
                                    }
                                    // Fallback: regex-based symbol scan
                                    let entries = parse_symbols_from_file(&path2);
                                    let _ = syms_tx2.send(entries);
                                });
                            }
                            Some(LspCommand::SaveFile { path }) => {
                                manager.did_save(&path);
                            }
                            Some(LspCommand::RequestPeekDefinition { path, line, col }) => {
                                if let Some(client) = manager.client_for_file(&path).cloned() {
                                    let path2  = path.clone();
                                    let evt_tx = event_tx.clone();
                                    let peek_tx2 = peek_tx.clone();
                                    tokio::spawn(async move {
                                        match client.goto_definition(&path2, line, col).await {
                                            Ok(locs) => {
                                                if let Some(loc) = locs.into_iter().next() {
                                                    let uri_str = loc.uri.to_string();
                                                    let target_path = uri_str
                                                        .strip_prefix("file://")
                                                        .map(std::path::PathBuf::from)
                                                        .unwrap_or_else(|| std::path::PathBuf::from(&uri_str));
                                                    let target_line = loc.range.start.line as usize; // 0-based
                                                    // Read 15 lines centered on the target
                                                    let lines_snapshot: Vec<String> =
                                                        std::fs::read_to_string(&target_path)
                                                            .map(|content| {
                                                                let all: Vec<&str> = content.lines().collect();
                                                                let total = all.len();
                                                                let start = target_line.saturating_sub(7);
                                                                let end = (target_line + 8).min(total);
                                                                all[start..end]
                                                                    .iter()
                                                                    .enumerate()
                                                                    .map(|(i, l)| {
                                                                        let ln = start + i + 1; // 1-based
                                                                        let marker = if start + i == target_line { ">" } else { " " };
                                                                        format!("{marker}{ln:>4}  {l}")
                                                                    })
                                                                    .collect()
                                                            })
                                                            .unwrap_or_default();
                                                    if !lines_snapshot.is_empty() {
                                                        let _ = peek_tx2.try_send(lines_snapshot);
                                                    }
                                                    // Also fire a Definition event so goto_definition signal updates
                                                    let _ = evt_tx.send(LspEvent::Definition(vec![loc]));
                                                }
                                            }
                                            Err(e) => eprintln!("[LSP] peek definition error: {e}"),
                                        }
                                    });
                                }
                            }
                            Some(LspCommand::RequestCodeLens { path }) => {
                                let code_lens_tx2 = code_lens_tx.clone();
                                let path2 = path.clone();
                                let client_opt = manager.client_for_file(&path).cloned();
                                tokio::spawn(async move {
                                    let entries = {
                                        // LspClient doesn't yet expose textDocument/codeLens,
                                        // so we use the file-scan fallback in all cases.
                                        // client_opt is available for future extension.
                                        let _client_opt = client_opt;
                                        code_lens_from_file(&path2)
                                    };
                                    let _ = code_lens_tx2.try_send(entries);
                                });
                            }
                            Some(LspCommand::RequestWorkspaceSymbols { query }) => {
                                // Try each active LSP client for workspace symbols.
                                // Fall back to a ripgrep-based symbol scan if no LSP client handles it.
                                let ws_syms_tx2 = ws_syms_tx.clone();
                                let query2      = query.clone();
                                let ws_root2    = ws_root_for_refs.clone();
                                // Collect any available client (prefer rust-analyzer or ts-server).
                                let client_opt: Option<std::sync::Arc<phazeai_core::LspClient>> =
                                    manager.client_for_language("rust")
                                    .or_else(|| manager.client_for_language("typescript"))
                                    .or_else(|| manager.client_for_language("python"))
                                    .cloned();
                                tokio::spawn(async move {
                                    if let Some(client) = client_opt {
                                        match client.workspace_symbol(&query2).await {
                                            Ok(syms) if !syms.is_empty() => {
                                                let entries = syms.into_iter().map(|si| {
                                                    let kind_str = symbol_kind_str(si.kind);
                                                    SymbolEntry {
                                                        name: si.name,
                                                        kind: kind_str,
                                                        line: si.location.range.start.line + 1,
                                                        depth: 0,
                                                    }
                                                }).collect::<Vec<_>>();
                                                let _ = ws_syms_tx2.try_send(entries);
                                                return;
                                            }
                                            _ => {}
                                        }
                                    }
                                    // Fallback: ripgrep-based workspace symbol scan
                                    let entries = ripgrep_workspace_symbols(&query2, &ws_root2);
                                    let _ = ws_syms_tx2.try_send(entries);
                                });
                            }
                            Some(LspCommand::RequestImplementation { path, line, col }) => {
                                if let Some(client) = manager.client_for_file(&path).cloned() {
                                    let path2  = path.clone();
                                    let evt_tx = event_tx.clone();
                                    tokio::spawn(async move {
                                        match client.goto_implementation(&path2, line, col).await {
                                            Ok(locs) => {
                                                let _ = evt_tx.send(LspEvent::Definition(locs));
                                            }
                                            Err(e) => eprintln!("[LSP] implementation error: {e}"),
                                        }
                                    });
                                }
                            }
                            Some(LspCommand::RequestFoldingRanges { path }) => {
                                let fold_tx2 = fold_ranges_tx.clone();
                                let path2 = path.clone();
                                let client_opt = manager.client_for_file(&path).cloned();
                                tokio::spawn(async move {
                                    if let Some(client) = client_opt {
                                        match client.folding_range(&path2).await {
                                            Ok(ranges) => {
                                                let pairs: Vec<(u32, u32)> = ranges
                                                    .into_iter()
                                                    .filter(|r| r.start_line != r.end_line)
                                                    .map(|r| (r.start_line, r.end_line))
                                                    .collect();
                                                let _ = fold_tx2.try_send(pairs);
                                            }
                                            Err(e) => eprintln!("[LSP] folding_range error: {e}"),
                                        }
                                    }
                                });
                            }
                            Some(LspCommand::OrganizeImports { path }) => {
                                if let Some(client) = manager.client_for_file(&path).cloned() {
                                    let path2 = path.clone();
                                    tokio::spawn(async move {
                                        // Build a codeAction request with only source.organizeImports
                                        let uri_str = format!("file://{}", path2.display());
                                        let uri: lsp_types::Uri = match uri_str.parse() {
                                            Ok(u) => u,
                                            Err(_) => return,
                                        };
                                        let params = lsp_types::CodeActionParams {
                                            text_document: lsp_types::TextDocumentIdentifier {
                                                uri: uri.clone(),
                                            },
                                            range: lsp_types::Range {
                                                start: lsp_types::Position { line: 0, character: 0 },
                                                end:   lsp_types::Position { line: 0, character: 0 },
                                            },
                                            context: lsp_types::CodeActionContext {
                                                diagnostics: vec![],
                                                only: Some(vec![
                                                    lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                                                ]),
                                                trigger_kind: None,
                                            },
                                            work_done_progress_params: Default::default(),
                                            partial_result_params: Default::default(),
                                        };
                                        match client.code_action(params).await {
                                            Ok(actions) => {
                                                for action in actions {
                                                    if let lsp_types::CodeActionOrCommand::CodeAction(ca) = action {
                                                        if let Some(edit) = ca.edit {
                                                            apply_workspace_edit(edit, "", "");
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => eprintln!("[LSP] organize imports error: {e}"),
                                        }
                                    });
                                } else {
                                    // Fallback: sort Rust imports locally
                                    let _ = organize_rust_imports(&path);
                                }
                            }
                            Some(LspCommand::RequestInlayHints { path, start_line, end_line }) => {
                                let inlay_tx2 = inlay_tx.clone();
                                let path2 = path.clone();
                                let client_opt = manager.client_for_file(&path).cloned();
                                tokio::spawn(async move {
                                    // Try LSP first (textDocument/inlayHint)
                                    if let Some(client) = client_opt {
                                        if let Ok(hints) = client.inlay_hints(&path2, start_line, end_line).await {
                                            let entries: Vec<InlayHintEntry> = hints.into_iter().map(|h| {
                                                let label = match &h.label {
                                                    lsp_types::InlayHintLabel::String(s) => s.clone(),
                                                    lsp_types::InlayHintLabel::LabelParts(parts) => {
                                                        parts.iter().map(|p| p.value.as_str()).collect::<String>()
                                                    }
                                                };
                                                InlayHintEntry {
                                                    line: h.position.line,
                                                    col: h.position.character,
                                                    label,
                                                }
                                            }).collect();
                                            if !entries.is_empty() {
                                                let _ = inlay_tx2.try_send(entries);
                                                return;
                                            }
                                        }
                                    }
                                    // Fallback: regex-based hints for common Rust patterns
                                    let hints = inlay_hints_from_file(&path2, start_line, end_line);
                                    let _ = inlay_tx2.try_send(hints);
                                });
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

                            Some(LspEvent::Definition(locs)) => {
                                if let Some(loc) = locs.into_iter().next() {
                                    let uri_str = loc.uri.to_string();
                                    let path = uri_str
                                        .strip_prefix("file://")
                                        .map(std::path::PathBuf::from)
                                        .unwrap_or_else(|| std::path::PathBuf::from(&uri_str));
                                    let result = DefinitionResult {
                                        path,
                                        line: loc.range.start.line + 1,
                                        col:  loc.range.start.character + 1,
                                    };
                                    let _ = def_tx.try_send(result);
                                }
                            }
                            Some(LspEvent::Hover(Some(hover))) => {
                                let text = hover_to_string(hover);
                                if !text.is_empty() {
                                    let _ = hover_tx.try_send(text);
                                }
                            }
                            Some(LspEvent::References(locs)) => {
                                let entries: Vec<ReferenceEntry> = locs.into_iter().map(|loc| {
                                    let uri_str = loc.uri.to_string();
                                    let path = uri_str
                                        .strip_prefix("file://")
                                        .map(PathBuf::from)
                                        .unwrap_or_else(|| PathBuf::from(&uri_str));
                                    ReferenceEntry {
                                        path,
                                        line: loc.range.start.line + 1,
                                        col:  loc.range.start.character + 1,
                                    }
                                }).collect();
                                let _ = refs_tx.try_send(entries);
                            }
                            Some(LspEvent::Log(msg)) => {
                                if msg == "__progress_end__" {
                                    let _ = progress_tx.try_send(None);
                                } else if let Some(text) = msg.strip_prefix("__progress__") {
                                    let label = if text.len() > 40 {
                                        format!("{}…", &text[..40])
                                    } else {
                                        text.to_string()
                                    };
                                    let _ = progress_tx.try_send(Some(label));
                                }
                                // __progress_create__ and other Log messages are silently ignored.
                            }
                            Some(_) => {} // other events ignored
                            None => break, // event channel closed
                        }
                    }
                }
            }
        });
    });

    // Wire all std::sync channels into Floem's reactive system.
    let diag_chan = create_signal_from_channel(diag_rx);
    let comp_chan = create_signal_from_channel(comp_rx);
    let def_chan = create_signal_from_channel(def_rx);
    let hover_chan = create_signal_from_channel(hover_rx);
    let refs_chan = create_signal_from_channel(refs_rx);
    let actions_chan = create_signal_from_channel(actions_rx);
    let sig_chan = create_signal_from_channel(sig_rx);
    let syms_chan = create_signal_from_channel(syms_rx);
    let ws_syms_chan = create_signal_from_channel(ws_syms_rx);
    let progress_chan = create_signal_from_channel(progress_rx);
    let peek_chan = create_signal_from_channel(peek_rx);
    let code_lens_chan = create_signal_from_channel(code_lens_rx);
    let fold_ranges_chan = create_signal_from_channel(fold_ranges_rx);
    let inlay_chan = create_signal_from_channel(inlay_rx);

    let diag_sig: RwSignal<Vec<DiagEntry>> = create_rw_signal(vec![]);
    let comp_sig: RwSignal<Vec<CompletionEntry>> = create_rw_signal(vec![]);
    let def_sig: RwSignal<Option<DefinitionResult>> = create_rw_signal(None);
    let hover_sig: RwSignal<Option<String>> = create_rw_signal(None);
    let refs_sig: RwSignal<Vec<ReferenceEntry>> = create_rw_signal(vec![]);
    let actions_sig: RwSignal<Vec<CodeAction>> = create_rw_signal(vec![]);
    let sig_help_sig: RwSignal<Option<SignatureHelpResult>> = create_rw_signal(None);
    let syms_sig: RwSignal<Vec<SymbolEntry>> = create_rw_signal(vec![]);
    let ws_syms_sig: RwSignal<Vec<SymbolEntry>> = create_rw_signal(vec![]);
    let lsp_progress_sig: RwSignal<Option<String>> = create_rw_signal(None);
    let peek_def_lines_sig: RwSignal<Vec<String>> = create_rw_signal(vec![]);
    let code_lens_sig: RwSignal<Vec<CodeLensEntry>> = create_rw_signal(vec![]);
    let folding_ranges_sig: RwSignal<Vec<(u32, u32)>> = create_rw_signal(vec![]);
    let inlay_hints_sig: RwSignal<Vec<InlayHintEntry>> = create_rw_signal(vec![]);

    create_effect(move |_| {
        if let Some(entries) = diag_chan.get() {
            diag_sig.set(entries);
        }
    });
    create_effect(move |_| {
        if let Some(entries) = comp_chan.get() {
            comp_sig.set(entries);
        }
    });
    create_effect(move |_| {
        if let Some(result) = def_chan.get() {
            def_sig.set(Some(result));
        }
    });
    create_effect(move |_| {
        if let Some(text) = hover_chan.get() {
            hover_sig.set(Some(text));
        }
    });
    create_effect(move |_| {
        if let Some(entries) = refs_chan.get() {
            refs_sig.set(entries);
        }
    });
    create_effect(move |_| {
        if let Some(actions) = actions_chan.get() {
            actions_sig.set(actions);
        }
    });
    create_effect(move |_| {
        if let Some(result) = sig_chan.get() {
            sig_help_sig.set(Some(result));
        }
    });
    create_effect(move |_| {
        if let Some(entries) = syms_chan.get() {
            syms_sig.set(entries);
        }
    });
    create_effect(move |_| {
        if let Some(entries) = ws_syms_chan.get() {
            ws_syms_sig.set(entries);
        }
    });
    create_effect(move |_| {
        if let Some(val) = progress_chan.get() {
            lsp_progress_sig.set(val);
        }
    });
    create_effect(move |_| {
        if let Some(lines) = peek_chan.get() {
            peek_def_lines_sig.set(lines);
        }
    });
    create_effect(move |_| {
        if let Some(entries) = code_lens_chan.get() {
            code_lens_sig.set(entries);
        }
    });
    create_effect(move |_| {
        if let Some(pairs) = fold_ranges_chan.get() {
            folding_ranges_sig.set(pairs);
        }
    });
    create_effect(move |_| {
        if let Some(hints) = inlay_chan.get() {
            inlay_hints_sig.set(hints);
        }
    });

    (
        lsp_cmd_tx,
        diag_sig,
        comp_sig,
        def_sig,
        hover_sig,
        refs_sig,
        actions_sig,
        sig_help_sig,
        syms_sig,
        ws_syms_sig,
        lsp_progress_sig,
        peek_def_lines_sig,
        code_lens_sig,
        folding_ranges_sig,
        inlay_hints_sig,
    )
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract plain text from an `lsp_types::Hover` value.
fn hover_to_string(hover: lsp_types::Hover) -> String {
    use lsp_types::HoverContents;
    match hover.contents {
        HoverContents::Scalar(ms) => marked_string_to_text(ms),
        HoverContents::Array(items) => items
            .into_iter()
            .map(marked_string_to_text)
            .collect::<Vec<_>>()
            .join("\n\n"),
        HoverContents::Markup(markup) => markup.value,
    }
}

fn marked_string_to_text(ms: lsp_types::MarkedString) -> String {
    match ms {
        lsp_types::MarkedString::String(s) => s,
        lsp_types::MarkedString::LanguageString(ls) => ls.value,
    }
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

// ── Fallback helpers ──────────────────────────────────────────────────────────

/// Extract the word at the given 0-based (line, col) from file content.
fn word_at_position(path: &PathBuf, line: u32, col: u32) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let target_line = text.lines().nth(line as usize)?;
    let col = (col as usize).min(target_line.len());
    // Walk backward to start of word
    let start = target_line[..col]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map(|(i, _)| i)
        .unwrap_or(col);
    // Walk forward to end of word
    let end = target_line[col..]
        .char_indices()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map(|(i, _)| col + i + 1)
        .unwrap_or(col);
    let word = target_line[start..end].to_string();
    if word.is_empty() {
        None
    } else {
        Some(word)
    }
}

/// Ripgrep-based fallback for find-references.
/// Runs `rg --json <word> <workspace>` and parses the output into ReferenceEntry.
fn ripgrep_references(
    path: &PathBuf,
    line: u32,
    col: u32,
    workspace: &PathBuf,
) -> Vec<ReferenceEntry> {
    let word = match word_at_position(path, line, col) {
        Some(w) if !w.is_empty() => w,
        _ => return vec![],
    };

    let output = std::process::Command::new("rg")
        .args(["--json", "-w", &word, workspace.to_string_lossy().as_ref()])
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();

    for line_str in stdout.lines() {
        // Parse each JSON line; skip anything that isn't a "match" event.
        let val: serde_json::Value = match serde_json::from_str(line_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if val.get("type").and_then(|t| t.as_str()) != Some("match") {
            continue;
        }

        // Use a closure to allow `?`-style early returns without polluting the outer fn.
        let parsed: Option<Vec<ReferenceEntry>> = (|| {
            let data = val.get("data")?;
            let file_path = data.get("path")?.get("text")?.as_str()?;
            let line_num = data.get("line_number")?.as_u64()? as u32;
            let submatches = data.get("submatches")?.as_array()?;
            let mut local = Vec::new();
            for sm in submatches {
                let col_start = sm.get("start")?.as_u64()? as u32 + 1;
                local.push(ReferenceEntry {
                    path: PathBuf::from(file_path),
                    line: line_num,
                    col: col_start,
                });
            }
            Some(local)
        })();

        if let Some(mut new_entries) = parsed {
            entries.append(&mut new_entries);
        }
    }

    entries
}

fn generate_code_actions(path: &PathBuf, line: u32, col: u32) -> Vec<CodeAction> {
    let mut actions = Vec::new();

    // Always offer "Format Document"
    actions.push(CodeAction {
        title: "Format Document".to_string(),
        kind: "source.formatDocument".to_string(),
        edit: None, // handled procedurally by the UI
    });

    // If file is Rust, offer "Organize Imports" (sort/dedup `use` lines)
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext == "rs" {
        actions.push(CodeAction {
            title: "Organize Imports (sort use declarations)".to_string(),
            kind: "source.organizeImports".to_string(),
            edit: organize_rust_imports(path),
        });
    }

    // Context-specific: if word under cursor looks like a variable, offer "Rename Symbol"
    if let Some(word) = word_at_position(path, line, col) {
        if !word.is_empty() {
            actions.push(CodeAction {
                title: format!("Find All References to '{word}'"),
                kind: "refactor.findReferences".to_string(),
                edit: None,
            });
        }
    }

    actions
}

/// Sort and deduplicate top-level `use` declarations in a Rust file.
/// Returns `Some(vec![(path, new_content)])` if the file changed, else `None`.
fn organize_rust_imports(path: &PathBuf) -> Option<Vec<(PathBuf, String)>> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().collect();

    // Find the contiguous block of `use` statements at the top (after attributes / comments)
    let mut use_start: Option<usize> = None;
    let mut use_end = 0usize;
    for (i, l) in lines.iter().enumerate() {
        let trimmed = l.trim();
        if trimmed.starts_with("use ") {
            if use_start.is_none() {
                use_start = Some(i);
            }
            use_end = i;
        } else if use_start.is_some()
            && !trimmed.is_empty()
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("/*")
            && !trimmed.starts_with('#')
        {
            break;
        }
    }

    let start = use_start?;
    let mut use_lines: Vec<String> = lines[start..=use_end]
        .iter()
        .map(|l| l.to_string())
        .collect();

    let original = use_lines.clone();
    use_lines.sort();
    use_lines.dedup();

    if use_lines == original {
        return None; // already sorted
    }

    let mut new_lines = lines[..start]
        .to_vec()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    new_lines.extend(use_lines);
    new_lines.extend(lines[use_end + 1..].iter().map(|s| s.to_string()));
    let new_content = new_lines.join("\n");

    Some(vec![(path.clone(), new_content)])
}

/// Replace all whole-word occurrences of `old` with `new_name` in `text`.
/// Uses a simple byte-scan so we don't need a regex dependency.
fn replace_whole_word(text: &str, old: &str, new_name: &str) -> String {
    if old.is_empty() {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(pos) = remaining.find(old) {
        let before = &remaining[..pos];
        let after_start = pos + old.len();
        // Check word boundaries: char before and after must not be word chars.
        let before_ok = before
            .chars()
            .last()
            .map(|c| !c.is_alphanumeric() && c != '_')
            .unwrap_or(true);
        let after_ok = remaining[after_start..]
            .chars()
            .next()
            .map(|c| !c.is_alphanumeric() && c != '_')
            .unwrap_or(true);
        if before_ok && after_ok {
            result.push_str(before);
            result.push_str(new_name);
        } else {
            result.push_str(before);
            result.push_str(old);
        }
        remaining = &remaining[after_start..];
    }
    result.push_str(remaining);
    result
}

/// Apply an LSP `WorkspaceEdit` by rewriting files on disk.
/// Falls back to whole-word replace when workspace edit is empty.
fn apply_workspace_edit(edit: lsp_types::WorkspaceEdit, old_word: &str, new_name: &str) {
    // Prefer document_changes (newer LSP); fall back to changes map.
    if let Some(doc_changes) = edit.document_changes {
        use lsp_types::DocumentChanges;
        match doc_changes {
            DocumentChanges::Edits(edits) => {
                for te in edits {
                    let uri_str = te.text_document.uri.to_string();
                    let path = uri_str
                        .strip_prefix("file://")
                        .map(std::path::Path::new)
                        .unwrap_or_else(|| std::path::Path::new(&uri_str));
                    apply_text_edits(path, &te.edits);
                }
            }
            DocumentChanges::Operations(ops) => {
                use lsp_types::DocumentChangeOperation;
                for op in ops {
                    if let DocumentChangeOperation::Edit(te) = op {
                        let uri_str = te.text_document.uri.to_string();
                        let path = uri_str
                            .strip_prefix("file://")
                            .map(std::path::Path::new)
                            .unwrap_or_else(|| std::path::Path::new(&uri_str));
                        apply_text_edits(path, &te.edits);
                    }
                }
            }
        }
        return;
    }

    if let Some(changes) = edit.changes {
        for (uri, edits) in changes {
            let uri_str = uri.to_string();
            let path = uri_str
                .strip_prefix("file://")
                .map(std::path::Path::new)
                .unwrap_or_else(|| std::path::Path::new(&uri_str));
            let wrapped: Vec<lsp_types::OneOf<lsp_types::TextEdit, lsp_types::AnnotatedTextEdit>> =
                edits.into_iter().map(lsp_types::OneOf::Left).collect();
            apply_text_edits(path, &wrapped);
        }
        return;
    }

    // No edits from LSP — should not happen, but guard anyway
    eprintln!("[LSP] apply_workspace_edit: no edits (old={old_word} new={new_name})");
}

/// Apply a list of LSP TextEdits to a file on disk (sort descending by range so
/// offsets stay valid after each replacement).
fn apply_text_edits(
    path: &std::path::Path,
    edits: &[lsp_types::OneOf<lsp_types::TextEdit, lsp_types::AnnotatedTextEdit>],
) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = content.lines().collect();

    // Flatten into (start_line, start_char, end_line, end_char, new_text)
    let mut flat: Vec<(u32, u32, u32, u32, String)> = edits
        .iter()
        .filter_map(|e| {
            let te = match e {
                lsp_types::OneOf::Left(t) => t.clone(),
                lsp_types::OneOf::Right(a) => a.text_edit.clone(),
            };
            Some((
                te.range.start.line,
                te.range.start.character,
                te.range.end.line,
                te.range.end.character,
                te.new_text.clone(),
            ))
        })
        .collect();

    // Apply in reverse order so earlier positions aren't shifted
    flat.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

    let mut new_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for (sl, sc, el, ec, new_text) in flat {
        let sl = sl as usize;
        let el = el as usize;
        if sl >= new_lines.len() {
            continue;
        }
        if sl == el {
            let line = &new_lines[sl];
            let sc = sc as usize;
            let ec = ec as usize;
            if sc <= line.len() && ec <= line.len() {
                let mut l = new_lines[sl].clone();
                l.replace_range(sc..ec, &new_text);
                new_lines[sl] = l;
            }
        } else {
            // Multi-line edit: rebuild affected range
            let start_line = new_lines[sl].clone();
            let sc = (sc as usize).min(start_line.len());
            let prefix = start_line[..sc].to_string();
            let end_line = if el < new_lines.len() {
                new_lines[el].clone()
            } else {
                String::new()
            };
            let ec = (ec as usize).min(end_line.len());
            let suffix = end_line[ec..].to_string();
            let replacement = format!("{prefix}{new_text}{suffix}");
            new_lines.drain(sl..=el.min(new_lines.len() - 1));
            new_lines.insert(sl, replacement);
        }
    }

    let new_content = new_lines.join("\n");
    let _ = std::fs::write(path, new_content);
}

/// Parse an lsp_types::SignatureHelp into our simplified struct.
fn parse_signature_help(sh: lsp_types::SignatureHelp) -> Option<SignatureHelpResult> {
    let active_sig = sh.active_signature.unwrap_or(0) as usize;
    let sig = sh.signatures.into_iter().nth(active_sig)?;
    let label = sig.label.clone();
    let active_param = sh
        .active_parameter
        .or_else(|| sig.active_parameter)
        .unwrap_or(0) as usize;
    let params: Vec<String> = sig
        .parameters
        .unwrap_or_default()
        .into_iter()
        .map(|p| match p.label {
            lsp_types::ParameterLabel::Simple(s) => s,
            lsp_types::ParameterLabel::LabelOffsets([s, e]) => {
                label.get(s as usize..e as usize).unwrap_or("").to_string()
            }
        })
        .collect();
    Some(SignatureHelpResult {
        label,
        active_param,
        params,
    })
}

/// Flatten nested `lsp_types::DocumentSymbol` tree into a flat list with depth info.
fn flatten_symbols(syms: &[lsp_types::DocumentSymbol], depth: u32) -> Vec<SymbolEntry> {
    let mut out = Vec::new();
    for sym in syms {
        let kind = match sym.kind {
            lsp_types::SymbolKind::FUNCTION => "fn",
            lsp_types::SymbolKind::METHOD => "fn",
            lsp_types::SymbolKind::STRUCT => "struct",
            lsp_types::SymbolKind::ENUM => "enum",
            lsp_types::SymbolKind::INTERFACE => "trait",
            lsp_types::SymbolKind::VARIABLE => "let",
            lsp_types::SymbolKind::CONSTANT => "const",
            lsp_types::SymbolKind::TYPE_PARAMETER => "type",
            lsp_types::SymbolKind::MODULE => "mod",
            lsp_types::SymbolKind::NAMESPACE => "mod",
            _ => "item",
        };
        out.push(SymbolEntry {
            name: sym.name.clone(),
            kind: kind.to_string(),
            line: sym.selection_range.start.line + 1,
            depth,
        });
        if let Some(children) = &sym.children {
            out.extend(flatten_symbols(children, depth + 1));
        }
    }
    out
}

/// Fallback: scan a source file for symbol definitions with a simple pattern match.
fn parse_symbols_from_file(path: &PathBuf) -> Vec<SymbolEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut symbols = Vec::new();

    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - trimmed.len();
        let depth = (indent / 4) as u32;

        let entry = if ext == "rs" {
            if let Some(rest) = trimmed
                .strip_prefix("pub async fn ")
                .or_else(|| trimmed.strip_prefix("async fn "))
                .or_else(|| trimmed.strip_prefix("pub fn "))
                .or_else(|| trimmed.strip_prefix("fn "))
            {
                let name = rest.split(['(', '<', ' ']).next().unwrap_or("").to_string();
                Some(("fn", name))
            } else if let Some(rest) = trimmed
                .strip_prefix("pub struct ")
                .or_else(|| trimmed.strip_prefix("struct "))
            {
                let name = rest.split([' ', '<', '{']).next().unwrap_or("").to_string();
                Some(("struct", name))
            } else if let Some(rest) = trimmed
                .strip_prefix("pub enum ")
                .or_else(|| trimmed.strip_prefix("enum "))
            {
                let name = rest.split([' ', '<', '{']).next().unwrap_or("").to_string();
                Some(("enum", name))
            } else if let Some(rest) = trimmed
                .strip_prefix("pub trait ")
                .or_else(|| trimmed.strip_prefix("trait "))
            {
                let name = rest.split([' ', '<', '{']).next().unwrap_or("").to_string();
                Some(("trait", name))
            } else if let Some(rest) = trimmed.strip_prefix("impl ") {
                let name = rest.split([' ', '<', '{']).next().unwrap_or("").to_string();
                Some(("impl", name))
            } else if let Some(rest) = trimmed
                .strip_prefix("pub mod ")
                .or_else(|| trimmed.strip_prefix("mod "))
            {
                let name = rest.split([' ', '{']).next().unwrap_or("").to_string();
                Some(("mod", name))
            } else {
                None
            }
        } else if matches!(ext, "js" | "ts" | "jsx" | "tsx") {
            if let Some(rest) = trimmed.strip_prefix("function ") {
                let name = rest.split(['(', ' ']).next().unwrap_or("").to_string();
                Some(("fn", name))
            } else if let Some(rest) = trimmed.strip_prefix("class ") {
                let name = rest.split([' ', '{']).next().unwrap_or("").to_string();
                Some(("class", name))
            } else {
                None
            }
        } else if ext == "py" {
            if let Some(rest) = trimmed.strip_prefix("def ") {
                let name = rest.split(['(', ':']).next().unwrap_or("").to_string();
                Some(("fn", name))
            } else if let Some(rest) = trimmed.strip_prefix("class ") {
                let name = rest.split(['(', ':']).next().unwrap_or("").to_string();
                Some(("class", name))
            } else {
                None
            }
        } else {
            None
        };

        if let Some((kind, name)) = entry {
            if !name.is_empty() {
                symbols.push(SymbolEntry {
                    name,
                    kind: kind.to_string(),
                    line: (i as u32) + 1,
                    depth,
                });
            }
        }
    }
    symbols
}

/// Convert an LSP `SymbolKind` to a short human-readable string.
fn symbol_kind_str(kind: lsp_types::SymbolKind) -> String {
    use lsp_types::SymbolKind;
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE => "mod",
        SymbolKind::NAMESPACE => "ns",
        SymbolKind::PACKAGE => "pkg",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "method",
        SymbolKind::PROPERTY => "prop",
        SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "ctor",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "trait",
        SymbolKind::FUNCTION => "fn",
        SymbolKind::VARIABLE => "var",
        SymbolKind::CONSTANT => "const",
        SymbolKind::STRING => "str",
        SymbolKind::NUMBER => "num",
        SymbolKind::BOOLEAN => "bool",
        SymbolKind::ARRAY => "array",
        SymbolKind::OBJECT => "obj",
        SymbolKind::KEY => "key",
        SymbolKind::NULL => "null",
        SymbolKind::ENUM_MEMBER => "variant",
        SymbolKind::STRUCT => "struct",
        SymbolKind::EVENT => "event",
        SymbolKind::OPERATOR => "op",
        SymbolKind::TYPE_PARAMETER => "type",
        _ => "sym",
    }
    .to_string()
}

/// Ripgrep-based fallback workspace symbol search.
/// Scans for function/struct/trait/impl/class/def declarations matching `query`.
fn ripgrep_workspace_symbols(query: &str, workspace: &std::path::Path) -> Vec<SymbolEntry> {
    if query.is_empty() {
        return vec![];
    }

    // Pattern: lines that look like declarations containing the query term.
    let pattern = format!(
        r"(?i)(fn|pub fn|async fn|struct|impl|trait|enum|class|def|function|interface|type)\s+{query}"
    );

    let output = std::process::Command::new("rg")
        .args([
            "--json",
            "--max-count=100",
            "--type-add=code:*.{rs,py,js,ts,go,rb,java,cs,cpp,c,kt}",
            "--type=code",
            "-e",
            &pattern,
            workspace.to_string_lossy().as_ref(),
        ])
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();

    for line_str in stdout.lines() {
        let val: serde_json::Value = match serde_json::from_str(line_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if val.get("type").and_then(|t| t.as_str()) != Some("match") {
            continue;
        }

        let parsed: Option<SymbolEntry> = (|| {
            let data = val.get("data")?;
            let _file_path = data.get("path")?.get("text")?.as_str()?;
            let line_num = data.get("line_number")?.as_u64()? as u32;
            let text = data.get("lines")?.get("text")?.as_str()?.trim();

            // Extract name from the matched line: word after the keyword
            let parts: Vec<&str> = text.split_whitespace().collect();
            let (kind_str, name) = if parts.len() >= 2 {
                let kw = parts[0];
                let nm = parts[1].trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');
                (kw, nm.to_string())
            } else {
                ("sym", text.to_string())
            };

            Some(SymbolEntry {
                name,
                kind: kind_str.to_string(),
                line: line_num,
                depth: 0,
            })
        })();

        if let Some(entry) = parsed {
            entries.push(entry);
        }
    }

    entries
}

/// Fallback code lens generation from a source file.
/// Scans for function/method definitions and attaches a "Run" or "Test" label.
fn code_lens_from_file(path: &PathBuf) -> Vec<CodeLensEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut lenses = Vec::new();

    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let line_num = (i as u32) + 1; // 1-based

        let label = if ext == "rs" {
            if trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test]") {
                Some("Run Test".to_string())
            } else if trimmed.starts_with("pub fn ")
                || trimmed.starts_with("fn ")
                || trimmed.starts_with("pub async fn ")
                || trimmed.starts_with("async fn ")
            {
                let rest = trimmed
                    .trim_start_matches("pub async fn ")
                    .trim_start_matches("async fn ")
                    .trim_start_matches("pub fn ")
                    .trim_start_matches("fn ");
                let name = rest.split(['(', '<', ' ']).next().unwrap_or("").to_string();
                if !name.is_empty() {
                    Some(format!("fn {name}"))
                } else {
                    None
                }
            } else {
                None
            }
        } else if matches!(ext, "js" | "ts" | "jsx" | "tsx") {
            if trimmed.starts_with("function ") || trimmed.starts_with("export function ") {
                Some("Run".to_string())
            } else {
                None
            }
        } else if ext == "py" {
            if trimmed.starts_with("def test_") {
                Some("Run Test".to_string())
            } else if trimmed.starts_with("def ") {
                Some("Run".to_string())
            } else {
                None
            }
        } else {
            None
        };

        if let Some(lbl) = label {
            lenses.push(CodeLensEntry {
                line: line_num,
                label: lbl,
            });
        }
    }

    lenses
}

/// Regex-based inlay hint generator for common patterns when no LSP server is available.
/// Produces type/parameter hints for Rust `let x =` and function calls.
fn inlay_hints_from_file(path: &PathBuf, start_line: u32, end_line: u32) -> Vec<InlayHintEntry> {
    let Ok(content) = std::fs::read_to_string(path) else { return vec![]; };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "rs" { return vec![]; }

    let mut hints = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx as u32;
        if line_num < start_line || line_num > end_line { continue; }

        let trimmed = line.trim();

        // `let x = <literal>` — show type after variable name
        if let Some(rest) = trimmed.strip_prefix("let ") {
            // Extract variable name (stop at whitespace, :, =)
            let var_end = rest.find(|c: char| c == ':' || c == '=' || c.is_whitespace())
                .unwrap_or(rest.len());
            let var_name = &rest[..var_end];
            if var_name.is_empty() || var_name == "_" || var_name == "mut" { continue; }

            // Determine type from rhs
            let rhs = if let Some(eq_pos) = rest.find('=') { rest[eq_pos + 1..].trim() } else { continue };
            let type_hint = if rhs.starts_with('"') { ": &str" }
                else if rhs.starts_with("vec![") || rhs.starts_with("Vec::") { ": Vec<_>" }
                else if rhs.parse::<i64>().is_ok() { ": i32" }
                else if rhs.parse::<f64>().is_ok() && rhs.contains('.') { ": f64" }
                else if rhs == "true" || rhs == "false" { ": bool" }
                else if rhs.starts_with("HashMap::") || rhs.starts_with("std::collections::HashMap") { ": HashMap<_, _>" }
                else { continue };

            // Column after the variable name in the original line
            if let Some(col) = line.find(var_name) {
                hints.push(InlayHintEntry {
                    line: line_num,
                    col: (col + var_name.len()) as u32,
                    label: type_hint.to_string(),
                });
            }
        }
    }
    hints
}
