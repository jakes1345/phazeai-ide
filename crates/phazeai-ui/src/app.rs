use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use floem::{
    action::show_context_menu,
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, Modifiers, NamedKey},
    menu::{Menu, MenuItem},
    peniko::kurbo::Size,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{canvas, container, dyn_stack, empty, label, scroll, stack, text_input, Decorators},
    window::WindowConfig,
    Application, IntoView, Renderer,
};
use phazeai_core::config::LlmProvider;
use phazeai_core::constants::ui as ui_const;
use phazeai_core::{Agent, AgentEvent, Settings};
use phazeai_sidecar::{SidecarClient, SidecarManager};

use crate::lsp_bridge::{
    start_lsp_bridge, CodeAction, CodeLensEntry, CompletionEntry, DefinitionResult, DiagEntry,
    DiagSeverity, LspCommand, ReferenceEntry, SymbolEntry,
};

use crate::{
    commands::{match_global_shortcut, GlobalShortcut},
    components::icon::{icons, phaze_icon},
    panels::{
        chat::chat_panel, editor::editor_panel, explorer::explorer_panel,
        extensions::extensions_panel, git::git_panel, github_actions::github_actions_panel, search,
        settings::settings_panel, terminal::terminal_panel,
    },
    theme::{PhazeTheme, ThemeVariant},
    util::safe_get,
};

/// Vim normal-mode motions dispatched to the active editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimMotion {
    // Movement
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordBackward,
    LineStart,
    LineEnd,
    GotoFileTop,         // gg
    GotoFileBottom,      // G
    HalfPageDown,        // Ctrl+d
    HalfPageUp,          // Ctrl+u
    JumpMatchingBracket, // %
    // Edit
    DeleteLine,
    DeleteChar,
    DeleteToLineEnd,   // D
    ReplaceChar(char), // r<char>
    // Change (delete + enter insert mode)
    ChangeToLineEnd, // C
    ChangeWholeLine, // cc
    ChangeWord,      // cw
    // Yank / Paste (vim register)
    YankLine,
    Paste,
    PasteBefore,
    // Mode
    EnterInsert,
    EnterInsertAfter,
    EnterInsertNewlineBelow,
    InsertAtLineEnd,   // A
    InsertAtLineStart, // I
    // Visual mode
    VisualCharStart, // v — start char-wise visual selection
    VisualLineStart, // V — start line-wise visual selection
    // Repeat
    RepeatLastEdit, // .
    // Marks
    SetMark(char),  // m<char>
    GotoMark(char), // `<char>
    // Ex command
    EnterExMode, // :
    // Selection ops (editor operations)
    ExpandSelection, // Ctrl+Shift+→
    ShrinkSelection, // Ctrl+Shift+←
    /// Delete the current visual selection and exit visual mode.
    DeleteVisualSelection,
    /// Yank (copy) the current visual selection and exit visual mode.
    YankVisualSelection,
    /// Change (delete + enter insert) the current visual selection.
    ChangeVisualSelection,
}

/// Global IDE state shared across all panels via Floem reactive system.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub path: std::path::PathBuf,
    pub line: usize,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Explorer,
    Search,
    Git,
    AI,
    Composer,
    Settings,
    Terminal,
    Chat,
    Extensions,
    Account,
    Debug,
    Remote,
    Containers,
    Makefile,
    GitHub,
    Problems,
    Output,
    Ports,
    DebugConsole,
    References,
    Symbols,
    GitDiff,
}

#[derive(Clone)]
pub struct IdeState {
    pub theme: RwSignal<PhazeTheme>,
    pub left_panel_tab: RwSignal<Tab>,
    pub bottom_panel_tab: RwSignal<Tab>,
    pub show_left_panel: RwSignal<bool>,
    pub show_right_panel: RwSignal<bool>,
    pub show_bottom_panel: RwSignal<bool>,
    pub open_file: RwSignal<Option<PathBuf>>,
    pub workspace_root: RwSignal<PathBuf>,
    /// Set to `true` while the AI chat panel is processing a request.
    /// Shared with the editor's sentient gutter so it glows during inference.
    pub ai_thinking: RwSignal<bool>,
    /// Rendered width of the left sidebar (300.0 when open, 0.0 when closed).
    /// Note: This snaps immediately. In the future, this could use a smooth animation loop
    /// when Floem exposes a stable `create_animation` / `spring` API.
    pub left_panel_width: RwSignal<f64>,
    /// Real git branch — populated async from `git rev-parse --abbrev-ref HEAD`.
    pub git_branch: RwSignal<String>,
    /// Whether the command palette overlay is visible.
    pub command_palette_open: RwSignal<bool>,
    /// Search text typed in the command palette.
    pub command_palette_query: RwSignal<String>,
    /// Whether the Ctrl+P file picker overlay is visible.
    pub file_picker_open: RwSignal<bool>,
    /// Query text in the file picker.
    pub file_picker_query: RwSignal<String>,
    /// All workspace files, populated async when picker opens.
    pub file_picker_files: RwSignal<Vec<std::path::PathBuf>>,
    // Search
    pub search_query: RwSignal<String>,
    pub search_results: RwSignal<Vec<SearchResult>>,
    // LSP — populated async by start_lsp_bridge()
    pub diagnostics: RwSignal<Vec<DiagEntry>>,
    pub lsp_cmd: tokio::sync::mpsc::UnboundedSender<LspCommand>,
    /// Latest completion list from the LSP server (set after RequestCompletions).
    pub completions: RwSignal<Vec<CompletionEntry>>,
    /// Whether the completion popup is visible.
    pub completion_open: RwSignal<bool>,
    /// Index of the highlighted item in the completion popup.
    pub completion_selected: RwSignal<usize>,
    /// Active cursor position: (path, 0-based line, 0-based col).
    /// Written by the editor; read by Ctrl+Space handler to know where to request.
    pub active_cursor: RwSignal<Option<(PathBuf, u32, u32)>>,
    // Panel resize drag state (used by the divider + overlay)
    pub panel_drag_active: RwSignal<bool>,
    pub panel_drag_start_x: RwSignal<f64>,
    pub panel_drag_start_width: RwSignal<f64>,
    /// Completion to insert: (text, prefix_byte_len_to_delete).
    /// prefix_byte_len_to_delete is the number of bytes of already-typed prefix to
    /// replace — e.g. if the user typed "pri" and accepted "println!", delete 3 bytes.
    /// Set to 0 for ghost-text (FIM) insertions where no prefix exists.
    pub pending_completion: RwSignal<Option<(String, usize)>>,
    /// Whether the Ctrl+K inline AI-edit overlay is open.
    pub inline_edit_open: RwSignal<bool>,
    /// User instruction typed in the inline-edit overlay.
    pub inline_edit_query: RwSignal<String>,
    /// Prefix typed since last Ctrl+Space — used to filter the completion list.
    pub completion_filter_text: RwSignal<String>,
    /// Whether vim keybindings are enabled (persisted to settings).
    pub vim_mode: RwSignal<bool>,
    /// Editor font size (persisted to config.toml).
    pub font_size: RwSignal<u32>,
    /// Editor tab size (persisted to config.toml).
    pub tab_size: RwSignal<u32>,
    /// Set by the LSP bridge when a go-to-definition result arrives.
    /// The IdeState effect watches this, opens the target file, and sets `goto_line`.
    pub goto_definition: RwSignal<Option<DefinitionResult>>,
    /// Hover documentation text from the LSP server (None when idle).
    pub hover_text: RwSignal<Option<String>>,
    /// Non-zero when the editor should jump to this 1-based line in the active file.
    /// Cleared to 0 by the editor after it performs the scroll/jump.
    pub goto_line: RwSignal<u32>,
    /// Incremented to trigger comment-toggle on the current line in the active editor.
    pub comment_toggle_nonce: RwSignal<u64>,
    /// All currently open editor tabs (written by editor_panel, read for session save).
    pub open_tabs: RwSignal<Vec<PathBuf>>,
    /// Tabs to restore on startup (passed once to editor_panel; not reactive after init).
    pub initial_tabs: Vec<PathBuf>,
    /// Active AI provider display name (e.g. "Claude (Anthropic)", "Ollama (Local)").
    pub ai_provider: RwSignal<String>,
    /// Active AI model identifier (e.g. "claude-sonnet-4-6", "llama3.2").
    pub ai_model: RwSignal<String>,
    /// True when vim mode is in Normal (command) mode; false = Insert mode.
    pub vim_normal_mode: RwSignal<bool>,
    /// Vim: pending first key of a two-key command (e.g. "d" before "d", "g" before "g").
    pub vim_pending_key: RwSignal<Option<char>>,
    /// Vim motion dispatched to the active editor when set.
    pub vim_motion: RwSignal<Option<VimMotion>>,
    /// Ghost text (FIM) suggestion — shown inline after cursor, Tab to accept.
    pub ghost_text: RwSignal<Option<String>>,
    /// Output panel log lines (build/run output, LSP messages, etc.)
    pub output_log: RwSignal<Vec<String>>,
    /// Find-all-references results (Shift+F12).
    pub references: RwSignal<Vec<ReferenceEntry>>,
    /// Whether the References tab in the bottom panel is the active view.
    pub references_visible: RwSignal<bool>,
    /// Code action list populated by Ctrl+. / RequestCodeActions.
    pub code_actions: RwSignal<Vec<CodeAction>>,
    /// Whether the code-action floating dropdown is open.
    pub code_actions_open: RwSignal<bool>,
    /// F2 rename-symbol overlay: true when open.
    pub rename_open: RwSignal<bool>,
    /// F2 rename-symbol overlay: the new name being typed.
    pub rename_query: RwSignal<String>,
    /// Original word the user is renaming (filled from cursor on F2).
    pub rename_target: RwSignal<String>,
    /// Signature help result from the LSP server (Ctrl+Shift+Space).
    pub sig_help: RwSignal<Option<crate::lsp_bridge::SignatureHelpResult>>,
    /// Document symbol outline for the active file (LSP or regex fallback).
    pub doc_symbols: RwSignal<Vec<SymbolEntry>>,
    /// Toast notification text — auto-cleared after 3 s.
    pub status_toast: RwSignal<Option<String>>,
    /// Zen mode — when true, hides all panels for distraction-free editing (Ctrl+Shift+Z).
    pub zen_mode: RwSignal<bool>,
    /// Line ending style of the active file ("LF", "CRLF", or "Mixed").
    pub line_ending: RwSignal<&'static str>,
    /// Whether the workspace symbols overlay (Ctrl+T) is visible.
    pub ws_syms_open: RwSignal<bool>,
    /// Filter query typed in the workspace symbols overlay.
    pub ws_syms_query: RwSignal<String>,
    /// Workspace symbol results — updated by the LSP bridge after RequestWorkspaceSymbols.
    pub workspace_symbols: RwSignal<Vec<SymbolEntry>>,
    /// Whether the branch picker overlay is open (click branch in status bar).
    pub branch_picker_open: RwSignal<bool>,
    /// List of local git branches for the branch picker overlay.
    pub branch_list: RwSignal<Vec<String>>,
    /// Auto-save: when true, saves the active file after 1.5 s of inactivity.
    pub auto_save: RwSignal<bool>,
    /// Word wrap toggle — when true the editor wraps long lines at the viewport edge.
    pub word_wrap: RwSignal<bool>,
    /// Ctrl+D nonce — incremented to trigger "select next occurrence" in active editor.
    pub ctrl_d_nonce: RwSignal<u64>,
    /// Fold nonce — Ctrl+Shift+[ collapses the block at the cursor line.
    pub fold_nonce: RwSignal<u64>,
    /// Unfold nonce — Ctrl+Shift+] expands the collapsed block at/around the cursor.
    pub unfold_nonce: RwSignal<u64>,
    /// Alt+Up — move the current line up one line.
    pub move_line_up_nonce: RwSignal<u64>,
    /// Alt+Down — move the current line down one line.
    pub move_line_down_nonce: RwSignal<u64>,
    /// Alt+Shift+Down — duplicate the current line below.
    pub duplicate_line_nonce: RwSignal<u64>,
    /// Ctrl+Shift+K — delete the entire current line.
    pub delete_line_nonce: RwSignal<u64>,
    /// Inline blame annotation for the current cursor line (shown in status bar).
    pub active_blame: RwSignal<String>,
    /// Whether the split editor pane is visible (Ctrl+Alt+\).
    pub split_editor: RwSignal<bool>,
    /// Active file in the split editor pane (independent of primary pane).
    pub split_open_file: RwSignal<Option<PathBuf>>,
    /// Open tabs in the split editor pane.
    pub split_open_tabs: RwSignal<Vec<PathBuf>>,
    /// Cursor position in the split editor pane.
    pub split_active_cursor: RwSignal<Option<(PathBuf, u32, u32)>>,
    /// Column cursor up nonce — Ctrl+Alt+Up adds cursor on line above at same column.
    pub col_cursor_up_nonce: RwSignal<u64>,
    /// Column cursor down nonce — Ctrl+Alt+Down adds cursor on line below at same column.
    pub col_cursor_down_nonce: RwSignal<u64>,
    /// Sticky scroll lines for the active tab — enclosing scope headers pinned above editor.
    pub sticky_lines: RwSignal<Vec<String>>,
    /// Transform to uppercase nonce — editor transforms current selection or word to UPPER CASE.
    pub transform_upper_nonce: RwSignal<u64>,
    /// Transform to lowercase nonce — editor transforms current selection or word to lower case.
    pub transform_lower_nonce: RwSignal<u64>,
    /// Join current line with the next line nonce.
    pub join_line_nonce: RwSignal<u64>,
    /// Sort selected lines alphabetically nonce.
    pub sort_lines_nonce: RwSignal<u64>,
    /// Vim visual mode active (v/V pressed).
    pub vim_visual_mode: RwSignal<bool>,
    /// Vim visual mode line-wise (V) vs char-wise (v).
    pub vim_visual_line: RwSignal<bool>,
    /// Vim marks: char → (file_path, byte_offset).
    pub vim_marks: RwSignal<std::collections::HashMap<char, (std::path::PathBuf, usize)>>,
    /// Vim last applied motion — used by `.` (repeat last change).
    pub vim_last_motion: RwSignal<Option<VimMotion>>,
    /// Vim ex command bar visible (`:` pressed in normal mode).
    pub vim_ex_open: RwSignal<bool>,
    /// Vim ex command text being typed.
    pub vim_ex_input: RwSignal<String>,
    /// Expand selection nonce (Ctrl+Shift+→).
    pub expand_selection_nonce: RwSignal<u64>,
    /// Shrink selection nonce (Ctrl+Shift+←).
    pub shrink_selection_nonce: RwSignal<u64>,
    /// Split editor down — horizontal split (second editor below first).
    pub split_editor_down: RwSignal<bool>,
    /// Open file in the horizontal split pane.
    pub split_down_file: RwSignal<Option<std::path::PathBuf>>,
    /// Open tabs in the horizontal split pane.
    pub split_down_tabs: RwSignal<Vec<std::path::PathBuf>>,
    /// Cursor in horizontal split pane.
    pub split_down_cursor: RwSignal<Option<(std::path::PathBuf, u32, u32)>>,
    /// Relative line numbers: show distance-from-cursor in gutter instead of absolute.
    pub relative_line_numbers: RwSignal<bool>,
    /// Scratch file counter — each Ctrl+N increments for unique untitled name.
    pub scratch_counter: RwSignal<u32>,
    /// Scratch file paths — virtual paths not backed by disk.
    pub scratch_paths: RwSignal<Vec<std::path::PathBuf>>,
    /// Yank ring: last 5 yanked strings (vim yy / Ctrl+C).
    pub yank_ring: RwSignal<Vec<String>>,
    /// Index into yank ring for Ctrl+Shift+V cycle.
    pub yank_ring_idx: RwSignal<usize>,
    /// Active file is read-only (permissions check on open).
    pub active_readonly: RwSignal<bool>,
    /// Goto line/col overlay open.
    pub goto_overlay_open: RwSignal<bool>,
    /// Goto overlay input text.
    pub goto_overlay_input: RwSignal<String>,
    /// Bottom panel maximized (double-click header to toggle).
    pub bottom_panel_maximized: RwSignal<bool>,
    /// LSP progress message (e.g. "indexing 45%") — None when idle.
    pub lsp_progress: RwSignal<Option<String>>,
    /// Peek definition source lines (Alt+F12) — set when a peek result arrives.
    pub peek_def_lines: RwSignal<Vec<String>>,
    /// Whether the peek definition popup is visible.
    pub peek_def_open: RwSignal<bool>,
    /// Code lens entries for the active file.
    pub code_lens: RwSignal<Vec<CodeLensEntry>>,
    /// LSP folding ranges for the active file: (start_line, end_line) pairs (0-based).
    pub folding_ranges: RwSignal<Vec<(u32, u32)>>,
    /// When true, automatically send OrganizeImports after saving the active file.
    pub organize_imports_on_save: RwSignal<bool>,
    /// Text to send to the active terminal PTY (Run in Terminal / Run File).
    /// Set by editor context menu; terminal_panel watches and resets to None after writing.
    pub run_in_terminal_text: RwSignal<Option<String>>,
    /// Incremented to title-case the current selection in the active editor.
    pub transform_title_nonce: RwSignal<u64>,
    /// Incremented to format only the current selection (rustfmt/prettier on selection).
    pub format_selection_nonce: RwSignal<u64>,
    /// Incremented to save the active file without running format-on-save.
    pub save_no_format_nonce: RwSignal<u64>,
    /// Incremented to fold all detected ranges in the active editor.
    pub fold_all_nonce: RwSignal<u64>,
    /// Incremented to unfold all ranges in the active editor.
    pub unfold_all_nonce: RwSignal<u64>,
    /// Code-lens entries for the active file (shown as inline gutter labels).
    pub code_lens_visible: RwSignal<bool>,
    /// Whether LSP inlay hints are shown in the editor.
    pub inlay_hints_toggle: RwSignal<bool>,
    /// Inlay hint entries from LSP or regex fallback for the active file.
    pub inlay_hints_sig: RwSignal<Vec<crate::lsp_bridge::InlayHintEntry>>,
    /// Shared handle to the sidecar client for explicit shutdown on IDE exit.
    pub sidecar_client: Arc<std::sync::Mutex<Option<Arc<SidecarClient>>>>,
    /// Whether the semantic search sidecar is running.
    pub sidecar_ready: RwSignal<bool>,
    /// Human-readable sidecar/indexing status shown in the UI.
    pub sidecar_status: RwSignal<String>,
    /// True while the semantic index is being built or rebuilt.
    pub sidecar_building: RwSignal<bool>,
    /// Semantic search results (file path + snippet pairs).
    pub sidecar_results: RwSignal<Vec<(String, String)>>,
    /// Semantic index rebuild nonce — increment to trigger a rebuild.
    pub sidecar_build_nonce: RwSignal<u64>,
    /// Semantic search query nonce — increment to trigger a search.
    pub sidecar_search_nonce: RwSignal<u64>,
    /// Current semantic search query text.
    pub sidecar_query: RwSignal<String>,

    /// Text to inject into the chat panel input and auto-send.
    /// Set by context menu "Explain Selection" / "Generate Tests" / "Fix with AI".
    pub pending_chat_inject: RwSignal<Option<String>>,

    // Extensions
    /// Native plugin manager
    pub ext_manager: Arc<std::sync::Mutex<phazeai_core::ext_host::ExtensionManager>>,
    /// Extensions currently loading or starting up
    pub ext_loading: RwSignal<bool>,
    /// Commands registered by extensions
    pub ext_commands: RwSignal<Vec<String>>,
    /// List of loaded extensions
    pub extensions: RwSignal<Vec<String>>,
}

impl std::fmt::Debug for IdeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdeState")
            .field("workspace_root", &self.workspace_root.get_untracked())
            .finish()
    }
}

/// Persisted layout state from ~/.config/phazeai/session.toml.
/// Uses serde + toml for reliable serialization.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SessionState {
    /// All open tab paths.
    open_tabs: Vec<PathBuf>,
    /// Index of the active (focused) tab, if any.
    active_tab_index: Option<usize>,
    left_panel_width: f64,
    show_left_panel: bool,
    show_right_panel: bool,
    show_bottom_panel: bool,
    split_editor: bool,
    split_editor_down: bool,
    vim_mode: bool,
    theme: String,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            open_tabs: Vec::new(),
            active_tab_index: None,
            left_panel_width: 260.0,
            show_left_panel: true,
            show_right_panel: true,
            show_bottom_panel: false,
            split_editor: false,
            split_editor_down: false,
            vim_mode: false,
            theme: "Midnight Blue".to_string(),
        }
    }
}

impl SessionState {
    /// Returns the active file path based on active_tab_index, filtered to existing files.
    fn active_file(&self) -> Option<PathBuf> {
        let idx = self.active_tab_index?;
        self.open_tabs.get(idx).cloned()
    }
}

/// Load session from `~/.config/phazeai/session.toml`.
fn session_load() -> SessionState {
    let Some(dir) = dirs_next_config() else {
        return SessionState::default();
    };
    let Ok(text) = std::fs::read_to_string(dir.join("session.toml")) else {
        return SessionState::default();
    };
    let mut state: SessionState = toml::from_str(&text).unwrap_or_default();
    // Filter out tabs for files that no longer exist on disk.
    state.open_tabs.retain(|p| p.exists());
    // Clamp active_tab_index to the surviving tab list.
    if let Some(idx) = state.active_tab_index {
        if state.open_tabs.is_empty() {
            state.active_tab_index = None;
        } else if idx >= state.open_tabs.len() {
            state.active_tab_index = Some(state.open_tabs.len().saturating_sub(1));
        }
    }
    state
}

/// Save session to `~/.config/phazeai/session.toml`.
fn session_save(state: &SessionState) {
    let Some(dir) = dirs_next_config() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(content) = toml::to_string_pretty(state) {
        let _ = std::fs::write(dir.join("session.toml"), content);
    }
}

fn session_update(mutate: impl FnOnce(&mut SessionState)) {
    let mut current = session_load();
    mutate(&mut current);
    session_save(&current);
}

fn dirs_next_config() -> Option<PathBuf> {
    let home = std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))?;
    Some(home.join(".config").join("phazeai"))
}

/// Convert a provider display name back to LlmProvider enum.
fn provider_name_to_llm_provider(name: &str) -> Option<LlmProvider> {
    match name {
        "Claude (Anthropic)" => Some(LlmProvider::Claude),
        "OpenAI" => Some(LlmProvider::OpenAI),
        "Google Gemini" => Some(LlmProvider::Gemini),
        "Groq" => Some(LlmProvider::Groq),
        "Together.ai" => Some(LlmProvider::Together),
        "OpenRouter" => Some(LlmProvider::OpenRouter),
        "LM Studio (Local)" => Some(LlmProvider::LmStudio),
        "Ollama (Local)" => Some(LlmProvider::Ollama),
        _ => None,
    }
}

/// Save a single editor setting by loading the full Settings, mutating, and writing back.
/// This preserves all other settings (LLM, sidecar, providers, etc.).
pub fn save_editor_settings(mutate: impl FnOnce(&mut phazeai_core::config::EditorSettings)) {
    let mut settings = Settings::load();
    mutate(&mut settings.editor);
    let _ = settings.save();
}

/// Show a toast notification that auto-dismisses after 3 seconds.
/// Safe to call from any code that has access to `IdeState`.
pub fn show_toast(toast: RwSignal<Option<String>>, msg: impl Into<String>) {
    use floem::ext_event::create_ext_action;
    use floem::reactive::Scope;
    toast.set(Some(msg.into()));
    // Use Scope::current() to reuse the caller's scope — no leak.
    let dismiss = create_ext_action(Scope::current(), move |_: ()| {
        toast.set(None);
    });
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        dismiss(());
    });
}

/// Load editor config from Settings (reads `~/.config/phazeai/config.toml` via toml crate).
fn load_editor_settings() -> phazeai_core::config::EditorSettings {
    Settings::load().editor
}

#[allow(clippy::too_many_arguments)]
fn spawn_sidecar_start(
    python_path: String,
    script: PathBuf,
    workspace_root: PathBuf,
    shared_client: Arc<std::sync::Mutex<Option<Arc<SidecarClient>>>>,
    ready_tx: std::sync::mpsc::SyncSender<bool>,
    status_tx: std::sync::mpsc::SyncSender<String>,
    build_tx: std::sync::mpsc::SyncSender<bool>,
    build_after_start: bool,
) {
    std::thread::spawn(move || {
        if shared_client.lock().ok().and_then(|g| g.clone()).is_some() {
            let _ = ready_tx.send(true);
            if build_after_start {
                let _ = build_tx.send(true);
            }
            return;
        }

        let _ = status_tx.send(format!(
            "Starting semantic search sidecar with {}...",
            python_path
        ));

        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = status_tx.send(format!("Semantic search runtime error: {e}"));
                let _ = ready_tx.send(false);
                return;
            }
        };

        rt.block_on(async move {
            let mut mgr = SidecarManager::new(python_path.clone(), script.clone());
            match mgr.start().await {
                Ok(()) => {
                    if let Some(process) = mgr.into_process() {
                        match SidecarClient::from_process(process) {
                            Ok(client) => {
                                if client.health_check().await {
                                    let client = Arc::new(client);
                                    if let Ok(mut slot) = shared_client.lock() {
                                        *slot = Some(client);
                                    }
                                    let _ = ready_tx.send(true);
                                    let _ = status_tx.send(format!(
                                        "Semantic search ready for {}",
                                        workspace_root.display()
                                    ));
                                    if build_after_start {
                                        let _ = build_tx.send(true);
                                    }
                                } else {
                                    let _ = ready_tx.send(false);
                                    let _ = status_tx
                                        .send("Semantic search sidecar failed health check".into());
                                }
                            }
                            Err(e) => {
                                let _ = ready_tx.send(false);
                                let _ = status_tx
                                    .send(format!("Semantic search connection error: {e}"));
                            }
                        }
                    } else {
                        let _ = ready_tx.send(false);
                        let _ = status_tx.send("Semantic search process handle missing".into());
                    }
                }
                Err(e) => {
                    let _ = ready_tx.send(false);
                    let _ = status_tx.send(format!("Semantic search failed to start: {e}"));
                }
            }
        });
    });
}

impl IdeState {
    pub fn new(settings: &Settings) -> Self {
        let _theme = PhazeTheme::from_name(&settings.editor.theme);
        // Use the git repository root as the workspace, so all git operations
        // are correctly scoped to the project root even when launched from a
        // subdirectory. Fall back to current_dir if not inside a git repo.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let workspace = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&cwd)
            .output()
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    String::from_utf8(out.stdout)
                        .ok()
                        .map(|s| PathBuf::from(s.trim()))
                } else {
                    None
                }
            })
            .unwrap_or(cwd);

        let git_branch = create_rw_signal("main".to_string());

        // Spawn a background thread to read the real git branch and push it to
        // the signal via a sync channel + create_signal_from_channel.
        let (branch_tx, branch_rx) = std::sync::mpsc::sync_channel::<String>(1);
        std::thread::spawn(move || {
            let branch = std::process::Command::new("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .output()
                .ok()
                .and_then(|out| {
                    if out.status.success() {
                        String::from_utf8(out.stdout)
                            .ok()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "main".to_string());
            let _ = branch_tx.send(branch);
        });

        // create_signal_from_channel hooks the mpsc receiver into Floem's reactive
        // system: whenever the channel receives a value the returned signal updates.
        let branch_signal = create_signal_from_channel(branch_rx);
        // Propagate the channel signal into the writable git_branch signal via effect.
        let git_branch_clone = git_branch;
        create_effect(move |_| {
            if let Some(b) = branch_signal.get() {
                git_branch_clone.set(b);
            }
        });

        // Restore last session.
        let session = session_load();

        // Load editor config from ~/.config/phazeai/config.toml via toml crate.
        let editor_cfg = load_editor_settings();

        let open_file: RwSignal<Option<PathBuf>> = create_rw_signal(session.active_file());
        let open_tabs_sig: RwSignal<Vec<PathBuf>> = create_rw_signal(Vec::new());
        let initial_tabs = session.open_tabs.clone();

        // Start LSP bridge — background tokio thread running LspManager.
        // Must be called in a Floem reactive scope (we're inside the window callback).
        let (
            lsp_cmd,
            diagnostics,
            completions,
            goto_definition,
            hover_text,
            references,
            code_actions,
            sig_help,
            doc_symbols,
            workspace_symbols,
            lsp_progress,
            peek_def_lines,
            code_lens,
            folding_ranges,
            inlay_hints_lsp,
        ) = start_lsp_bridge(workspace.clone());

        // Watch peek_def_lines: when it becomes non-empty, open the peek popup.
        let peek_def_open_sig: RwSignal<bool> = create_rw_signal(false);
        {
            let peek_lines = peek_def_lines;
            let peek_open = peek_def_open_sig;
            create_effect(move |_| {
                if !peek_lines.get().is_empty() {
                    peek_open.set(true);
                }
            });
        }

        // When a definition result arrives, navigate to the target file + line.
        let goto_line_sig: RwSignal<u32> = create_rw_signal(0u32);
        {
            let open_file2 = open_file;
            let goto_line2 = goto_line_sig;
            create_effect(move |_| {
                if let Some(result) = goto_definition.get() {
                    open_file2.set(Some(result.path.clone()));
                    goto_line2.set(result.line);
                    // Reset so the same definition won't re-trigger on the next
                    // reactive cycle that happens to read this signal.
                    goto_definition.set(None);
                }
            });
        }

        // Wire: whenever the active file changes, send did_open to the LSP server
        // and persist the session (including all open tabs).
        {
            let lsp_tx = lsp_cmd.clone();
            let tabs_for_save = open_tabs_sig;
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    // Read file + send LSP did_open + save session in background
                    // to avoid blocking the UI thread with synchronous I/O.
                    let lsp = lsp_tx.clone();
                    let all_tabs = tabs_for_save.get_untracked();
                    let p = path.clone();
                    std::thread::spawn(move || {
                        if let Ok(text) = std::fs::read_to_string(&p) {
                            let _ = lsp.send(LspCommand::OpenFile {
                                path: p.clone(),
                                text,
                            });
                        }
                        let _current = session_load();
                        session_update(|s| {
                            s.open_tabs = all_tabs;
                            s.active_tab_index = s.open_tabs.iter().position(|t| t == &p);
                        });
                    });
                }
            });
        }

        // Request document symbols whenever the active file changes.
        {
            let lsp_tx = lsp_cmd.clone();
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    let _ = lsp_tx.send(LspCommand::RequestDocumentSymbols { path });
                }
            });
        }

        // Request LSP folding ranges + code lens whenever the active file changes.
        {
            let lsp_tx = lsp_cmd.clone();
            let lsp_tx2 = lsp_cmd.clone();
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    let _ = lsp_tx.send(LspCommand::RequestFoldingRanges { path });
                }
            });
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    let _ = lsp_tx2.send(LspCommand::RequestCodeLens { path });
                }
            });
            let lsp_tx3 = lsp_cmd.clone();
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    let _ = lsp_tx3.send(LspCommand::RequestInlayHints {
                        path,
                        start_line: 0,
                        end_line: 2000,
                    });
                }
            });
        }

        // Detect read-only status + line-ending style in background thread.
        let active_readonly_sig: RwSignal<bool> = create_rw_signal(false);
        let line_ending_sig: RwSignal<&'static str> = create_rw_signal("LF");
        {
            use floem::ext_event::create_signal_from_channel;
            let (file_info_tx, file_info_rx) =
                std::sync::mpsc::sync_channel::<(bool, &'static str)>(1);
            let file_info_sig = create_signal_from_channel(file_info_rx);
            create_effect(move |_| {
                if let Some((readonly, ending)) = file_info_sig.get() {
                    active_readonly_sig.set(readonly);
                    line_ending_sig.set(ending);
                }
            });
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    let tx = file_info_tx.clone();
                    std::thread::spawn(move || {
                        let readonly = std::fs::metadata(&path)
                            .map(|m| m.permissions().readonly())
                            .unwrap_or(false);
                        let style = std::fs::read(&path)
                            .ok()
                            .map(|bytes| {
                                let crlf_count = bytes.windows(2).filter(|w| *w == b"\r\n").count();
                                let lf_count = bytes.iter().filter(|&&b| b == b'\n').count();
                                if crlf_count > 0 && lf_count > crlf_count {
                                    "Mixed"
                                } else if crlf_count > 0 {
                                    "CRLF"
                                } else {
                                    "LF"
                                }
                            })
                            .unwrap_or("LF");
                        let _ = tx.try_send((readonly, style));
                    });
                } else {
                    active_readonly_sig.set(false);
                    line_ending_sig.set("LF");
                }
            });
        }

        // Create persistent settings signals before Self so we can wire save effects.
        let theme_signal = create_rw_signal(PhazeTheme::from_name(&session.theme));
        let font_size_signal = create_rw_signal(editor_cfg.font_size as u32);
        let tab_size_signal = create_rw_signal(editor_cfg.tab_size);
        let auto_save_signal = create_rw_signal(editor_cfg.auto_save);
        let word_wrap_signal = create_rw_signal(editor_cfg.word_wrap);
        let relative_line_numbers_signal = create_rw_signal(editor_cfg.relative_line_numbers);
        let inlay_hints_toggle_signal = create_rw_signal(editor_cfg.inlay_hints);
        let code_lens_visible_signal = create_rw_signal(editor_cfg.code_lens);
        let organize_imports_signal = create_rw_signal(editor_cfg.organize_imports_on_save);

        // Whenever theme, font_size, or tab_size changes, persist to config.toml.
        // Done in a background thread to avoid blocking the UI.
        create_effect(move |_| {
            let theme_name = theme_signal.get().variant.name().to_string();
            let fs = font_size_signal.get();
            let ts = tab_size_signal.get();
            let auto_save = auto_save_signal.get();
            let word_wrap = word_wrap_signal.get();
            let rel_nums = relative_line_numbers_signal.get();
            let inlay = inlay_hints_toggle_signal.get();
            let code_lens = code_lens_visible_signal.get();
            let organize = organize_imports_signal.get();
            std::thread::spawn(move || {
                save_editor_settings(|e| {
                    e.theme = theme_name;
                    e.font_size = fs as f32;
                    e.tab_size = ts;
                    e.auto_save = auto_save;
                    e.word_wrap = word_wrap;
                    e.relative_line_numbers = rel_nums;
                    e.inlay_hints = inlay;
                    e.code_lens = code_lens;
                    e.organize_imports_on_save = organize;
                });
            });
        });

        // Also persist theme name to session.toml whenever it changes.
        create_effect(move |_| {
            let theme_name = theme_signal.get().variant.name().to_string();
            std::thread::spawn(move || {
                let _current = session_load();
                session_update(|s| s.theme = theme_name);
            });
        });

        // ── Sidecar startup ────────────────────────────────────────────────────
        // Locate server.py: first try <exe_dir>/sidecar/server.py, then
        // the repo-relative path, then ~/.config/phazeai/sidecar/server.py.
        let sidecar_ready_sig = create_rw_signal(false);
        let sidecar_status_sig = create_rw_signal(if !settings.sidecar.enabled {
            "Semantic search disabled in settings.".to_string()
        } else {
            "Semantic search not started.".to_string()
        });
        let sidecar_building_sig = create_rw_signal(false);
        let sidecar_results_sig: RwSignal<Vec<(String, String)>> = create_rw_signal(Vec::new());
        let sidecar_build_nonce_sig = create_rw_signal(0u64);
        let sidecar_search_nonce_sig = create_rw_signal(0u64);
        let sidecar_query_sig = create_rw_signal(String::new());

        let script_candidates: Vec<PathBuf> = {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()));
            let mut candidates = vec![
                PathBuf::from("sidecar/server.py"),
                PathBuf::from("../sidecar/server.py"),
            ];
            if let Some(dir) = exe_dir {
                candidates.push(dir.join("sidecar/server.py"));
                candidates.push(dir.join("../sidecar/server.py"));
                candidates.push(dir.join("../../sidecar/server.py"));
            }
            if let Some(home) = dirs_next_config() {
                candidates.push(home.join("sidecar/server.py"));
            }
            candidates
        };
        let sidecar_script = script_candidates.into_iter().find(|p| p.exists());

        // Shared sidecar client — always created so IdeState can reference it
        // for clean shutdown. Will be None when sidecar is disabled or unavailable.
        let shared_client: Arc<std::sync::Mutex<Option<Arc<SidecarClient>>>> =
            Arc::new(std::sync::Mutex::new(None));

        if !settings.sidecar.enabled {
            sidecar_ready_sig.set(false);
        } else if let Some(script) = sidecar_script {
            let sidecar_ready2 = sidecar_ready_sig;
            let sidecar_status2 = sidecar_status_sig;
            let sidecar_building2 = sidecar_building_sig;
            let sidecar_results2 = sidecar_results_sig;
            let sidecar_build_nonce = sidecar_build_nonce_sig;
            let sidecar_nonce = sidecar_search_nonce_sig;
            let sidecar_query2 = sidecar_query_sig;
            let (sc_tx, sc_rx) = std::sync::mpsc::sync_channel::<Vec<(String, String)>>(4);
            let sc_signal = create_signal_from_channel(sc_rx);
            create_effect(move |_| {
                if let Some(results) = sc_signal.get() {
                    sidecar_results2.set(results);
                }
            });
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<bool>(1);
            let ready_signal = create_signal_from_channel(ready_rx);
            create_effect(move |_| {
                if let Some(ok) = ready_signal.get() {
                    sidecar_ready2.set(ok);
                }
            });
            let (status_tx, status_rx) = std::sync::mpsc::sync_channel::<String>(8);
            let status_signal = create_signal_from_channel(status_rx);
            create_effect(move |_| {
                if let Some(status) = status_signal.get() {
                    sidecar_status2.set(status);
                }
            });
            let (building_tx, building_rx) = std::sync::mpsc::sync_channel::<bool>(8);
            let building_signal = create_signal_from_channel(building_rx);
            create_effect(move |_| {
                if let Some(is_building) = building_signal.get() {
                    sidecar_building2.set(is_building);
                }
            });
            let (build_tx, build_rx) = std::sync::mpsc::sync_channel::<bool>(8);
            let build_signal = create_signal_from_channel(build_rx);
            let workspace_root = workspace.clone();
            let python_path = settings.sidecar.python_path.clone();
            let script_for_build = script.clone();
            let shared_client_for_build = shared_client.clone();
            let ready_tx_for_build = ready_tx.clone();
            let status_tx_for_build = status_tx.clone();
            let build_tx_for_build = build_tx.clone();
            create_effect(move |_| {
                if build_signal.get().is_none() {
                    return;
                }

                let root = workspace_root.clone();
                let client = shared_client_for_build.lock().ok().and_then(|g| g.clone());
                if client.is_none() {
                    let _ = building_tx.send(true);
                    spawn_sidecar_start(
                        python_path.clone(),
                        script_for_build.clone(),
                        root,
                        shared_client_for_build.clone(),
                        ready_tx_for_build.clone(),
                        status_tx_for_build.clone(),
                        build_tx_for_build.clone(),
                        true,
                    );
                    return;
                }

                let client = client.unwrap();
                let tx = status_tx_for_build.clone();
                let building_tx2 = building_tx.clone();
                let root_str = root.display().to_string();
                std::thread::spawn(move || {
                    let _ = building_tx2.send(true);
                    let _ = tx.send(format!("Building semantic index for {root_str}..."));
                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            let _ = tx.send(format!("Semantic index runtime error: {e}"));
                            let _ = building_tx2.send(false);
                            return;
                        }
                    };

                    let result = rt.block_on(async move {
                        client
                            .build_index(std::slice::from_ref(&root_str))
                            .await
                            .map(|value| {
                                let indexed =
                                    value.get("indexed").and_then(|v| v.as_u64()).unwrap_or(0);
                                let total = value
                                    .get("total_files")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(indexed);
                                format!(
                                    "Semantic index ready: {indexed} files indexed ({total} total)"
                                )
                            })
                            .unwrap_or_else(|e| format!("Semantic index build failed: {e}"))
                    });

                    let _ = tx.send(result);
                    let _ = building_tx2.send(false);
                });
            });
            // Watch nonce: when incremented, send search request to sidecar
            let sc_tx2 = sc_tx.clone();
            let shared_client_for_search = shared_client.clone();
            let status_tx_for_search = status_tx.clone();
            create_effect(move |_| {
                let _nonce = sidecar_nonce.get();
                let query = sidecar_query2.get();
                if !query.is_empty() {
                    let tx = sc_tx2.clone();
                    let client_cell = shared_client_for_search.clone();
                    let status_tx3 = status_tx_for_search.clone();
                    std::thread::spawn(move || {
                        let client = client_cell.lock().ok().and_then(|g| g.clone());
                        let Some(client) = client else {
                            let _ = status_tx3.send(
                                "Semantic search unavailable. Build the index to start the sidecar."
                                    .to_string(),
                            );
                            let _ = tx.send(vec![(
                                "sidecar unavailable".to_string(),
                                "semantic search is not connected".to_string(),
                            )]);
                            return;
                        };

                        let rt = match tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                        {
                            Ok(rt) => rt,
                            Err(e) => {
                                let _ = tx.send(vec![(
                                    "sidecar error".to_string(),
                                    format!("failed to create runtime: {e}"),
                                )]);
                                return;
                            }
                        };

                        let results = rt.block_on(async move {
                            match client.search_embeddings(&query, 8).await {
                                Ok(value) => value
                                    .get("matches")
                                    .and_then(|v| v.as_array())
                                    .map(|matches| {
                                        matches
                                            .iter()
                                            .map(|m| {
                                                let file = m
                                                    .get("file")
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("unknown")
                                                    .to_string();
                                                let snippet = m
                                                    .get("snippet")
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                (file, snippet)
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default(),
                                Err(e) => vec![(
                                    "sidecar error".to_string(),
                                    if e.contains("Index not built") {
                                        "semantic index not built yet — click Reindex".to_string()
                                    } else {
                                        format!("semantic search failed: {e}")
                                    },
                                )],
                            }
                        });

                        let _ = tx.send(results);
                    });
                }
            });
            let build_tx_for_nonce = build_tx.clone();
            create_effect(move |_| {
                let nonce = sidecar_build_nonce.get();
                if nonce == 0 {
                    return;
                }
                let _ = build_tx_for_nonce.send(true);
            });

            if settings.sidecar.auto_start {
                sidecar_status_sig.set("Starting semantic search...".to_string());
                spawn_sidecar_start(
                    settings.sidecar.python_path.clone(),
                    script,
                    workspace.clone(),
                    shared_client.clone(),
                    ready_tx.clone(),
                    status_tx.clone(),
                    build_tx.clone(),
                    true,
                );
            } else {
                sidecar_status_sig.set(
                    "Semantic search idle. Click Reindex to start and build the index.".into(),
                );
            }
        } else {
            sidecar_status_sig.set("Semantic search sidecar script not found.".to_string());
        }

        // AI provider / model signals — initialized from current settings file.
        let ai_provider_sig =
            create_rw_signal(settings.llm.provider.to_provider_id().name().to_string());
        let ai_model_sig = create_rw_signal(settings.llm.model.clone());

        let status_toast_sig = create_rw_signal(None);

        // Extension Manager — native plugin system
        let ext_manager = Arc::new(std::sync::Mutex::new(
            phazeai_core::ext_host::ExtensionManager::new(),
        ));

        // Persist provider + model changes to settings.toml whenever they change.
        create_effect(move |_| {
            let provider_name = ai_provider_sig.get();
            let model = ai_model_sig.get();
            std::thread::spawn(move || {
                let mut s = Settings::load();
                let Some(provider) = provider_name_to_llm_provider(&provider_name) else {
                    return;
                };
                s.llm.provider = provider;
                s.llm.model = model;
                let _ = s.save();
            });
        });

        Self {
            theme: theme_signal,
            left_panel_tab: create_rw_signal(Tab::Explorer),
            bottom_panel_tab: create_rw_signal(Tab::Terminal),
            show_left_panel: create_rw_signal(session.show_left_panel),
            show_right_panel: create_rw_signal(session.show_right_panel),
            show_bottom_panel: create_rw_signal(session.show_bottom_panel),
            open_file,
            workspace_root: create_rw_signal(workspace),
            ai_thinking: create_rw_signal(false),
            left_panel_width: create_rw_signal(session.left_panel_width),
            git_branch,
            command_palette_open: create_rw_signal(false),
            command_palette_query: create_rw_signal(String::new()),
            file_picker_open: create_rw_signal(false),
            file_picker_query: create_rw_signal(String::new()),
            file_picker_files: create_rw_signal(Vec::new()),
            search_query: create_rw_signal("".to_string()),
            search_results: create_rw_signal(Vec::new()),
            diagnostics,
            lsp_cmd,
            completions,
            completion_open: create_rw_signal(false),
            completion_selected: create_rw_signal(0usize),
            active_cursor: create_rw_signal(None),
            panel_drag_active: create_rw_signal(false),
            panel_drag_start_x: create_rw_signal(0.0),
            panel_drag_start_width: create_rw_signal(session.left_panel_width),
            pending_completion: create_rw_signal(None::<(String, usize)>),
            inline_edit_open: create_rw_signal(false),
            inline_edit_query: create_rw_signal(String::new()),
            completion_filter_text: create_rw_signal(String::new()),
            vim_mode: create_rw_signal(session.vim_mode),
            font_size: font_size_signal,
            tab_size: tab_size_signal,
            goto_definition,
            hover_text,
            goto_line: goto_line_sig,
            comment_toggle_nonce: create_rw_signal(0u64),
            open_tabs: open_tabs_sig,
            initial_tabs,
            ai_provider: ai_provider_sig,
            ai_model: ai_model_sig,
            vim_normal_mode: create_rw_signal(false),
            vim_pending_key: create_rw_signal(None),
            vim_motion: create_rw_signal(None),
            ghost_text: create_rw_signal(None),
            output_log: create_rw_signal(vec!["[PhazeAI] Output panel ready.".to_string()]),
            references,
            references_visible: create_rw_signal(false),
            code_actions,
            code_actions_open: create_rw_signal(false),
            rename_open: create_rw_signal(false),
            rename_query: create_rw_signal(String::new()),
            rename_target: create_rw_signal(String::new()),
            sig_help,
            doc_symbols,
            status_toast: status_toast_sig,
            zen_mode: create_rw_signal(false),
            line_ending: line_ending_sig,
            ws_syms_open: create_rw_signal(false),
            ws_syms_query: create_rw_signal(String::new()),
            workspace_symbols,
            branch_picker_open: create_rw_signal(false),
            branch_list: create_rw_signal(Vec::new()),
            auto_save: auto_save_signal,
            word_wrap: word_wrap_signal,
            ctrl_d_nonce: create_rw_signal(0u64),
            fold_nonce: create_rw_signal(0u64),
            unfold_nonce: create_rw_signal(0u64),
            move_line_up_nonce: create_rw_signal(0u64),
            move_line_down_nonce: create_rw_signal(0u64),
            duplicate_line_nonce: create_rw_signal(0u64),
            delete_line_nonce: create_rw_signal(0u64),
            active_blame: create_rw_signal(String::new()),
            split_editor: create_rw_signal(false),
            split_open_file: create_rw_signal(None),
            split_open_tabs: create_rw_signal(Vec::new()),
            split_active_cursor: create_rw_signal(None),
            col_cursor_up_nonce: create_rw_signal(0u64),
            col_cursor_down_nonce: create_rw_signal(0u64),
            sticky_lines: create_rw_signal(Vec::new()),
            transform_upper_nonce: create_rw_signal(0u64),
            transform_lower_nonce: create_rw_signal(0u64),
            join_line_nonce: create_rw_signal(0u64),
            sort_lines_nonce: create_rw_signal(0u64),
            vim_visual_mode: create_rw_signal(false),
            vim_visual_line: create_rw_signal(false),
            vim_marks: create_rw_signal(std::collections::HashMap::new()),
            vim_last_motion: create_rw_signal(None),
            vim_ex_open: create_rw_signal(false),
            vim_ex_input: create_rw_signal(String::new()),
            expand_selection_nonce: create_rw_signal(0u64),
            shrink_selection_nonce: create_rw_signal(0u64),
            split_editor_down: create_rw_signal(false),
            split_down_file: create_rw_signal(None),
            split_down_tabs: create_rw_signal(Vec::new()),
            split_down_cursor: create_rw_signal(None),
            relative_line_numbers: relative_line_numbers_signal,
            scratch_counter: create_rw_signal(0u32),
            scratch_paths: create_rw_signal(Vec::new()),
            yank_ring: create_rw_signal(Vec::new()),
            yank_ring_idx: create_rw_signal(0usize),
            active_readonly: active_readonly_sig,
            goto_overlay_open: create_rw_signal(false),
            goto_overlay_input: create_rw_signal(String::new()),
            bottom_panel_maximized: create_rw_signal(false),
            lsp_progress,
            peek_def_lines,
            peek_def_open: peek_def_open_sig,
            code_lens,
            folding_ranges,
            organize_imports_on_save: organize_imports_signal,
            run_in_terminal_text: create_rw_signal(None),
            transform_title_nonce: create_rw_signal(0u64),
            format_selection_nonce: create_rw_signal(0u64),
            save_no_format_nonce: create_rw_signal(0u64),
            fold_all_nonce: create_rw_signal(0u64),
            unfold_all_nonce: create_rw_signal(0u64),
            code_lens_visible: code_lens_visible_signal,
            inlay_hints_toggle: inlay_hints_toggle_signal,
            inlay_hints_sig: inlay_hints_lsp,
            sidecar_client: shared_client.clone(),
            sidecar_ready: sidecar_ready_sig,
            sidecar_status: sidecar_status_sig,
            sidecar_building: sidecar_building_sig,
            sidecar_results: sidecar_results_sig,
            sidecar_build_nonce: sidecar_build_nonce_sig,
            sidecar_search_nonce: sidecar_search_nonce_sig,
            sidecar_query: sidecar_query_sig,
            pending_chat_inject: create_rw_signal(None),
            ext_manager,
            ext_loading: create_rw_signal(false),
            ext_commands: create_rw_signal(Vec::new()),
            extensions: create_rw_signal(Vec::new()),
        }
    }
}

// ── Command palette commands ──────────────────────────────────────────────────

#[derive(Clone)]
struct PaletteCommand {
    label: &'static str,
    action: fn(IdeState),
}

fn all_commands() -> Vec<PaletteCommand> {
    vec![
        PaletteCommand {
            label: "Open File…",
            action: |s| {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    s.open_file.set(Some(path));
                    s.show_left_panel.set(true);
                    s.left_panel_width.set(260.0);
                }
            },
        },
        PaletteCommand {
            label: "Open Folder…",
            action: |s| {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    s.workspace_root.set(folder);
                    // Clear file picker cache so it re-walks on next open
                    s.file_picker_files.set(Vec::new());
                    s.show_left_panel.set(true);
                    s.left_panel_width.set(300.0);
                    s.left_panel_tab.set(crate::app::Tab::Explorer);
                }
            },
        },
        PaletteCommand {
            label: "Toggle Terminal",
            action: |s| {
                s.show_bottom_panel.update(|v| *v = !*v);
            },
        },
        PaletteCommand {
            label: "Toggle Explorer",
            action: |s| {
                s.show_left_panel.update(|v| *v = !*v);
                let open = s.show_left_panel.get();
                s.left_panel_width.set(if open { 260.0 } else { 0.0 });
            },
        },
        PaletteCommand {
            label: "Toggle AI Chat",
            action: |s| {
                s.show_right_panel.update(|v| *v = !*v);
            },
        },
        PaletteCommand {
            label: "Show AI Panel",
            action: |s| {
                s.left_panel_tab.set(Tab::AI);
                s.show_left_panel.set(true);
                s.left_panel_width.set(260.0);
            },
        },
        // ── All 12 themes ────────────────────────────────────────────────────
        PaletteCommand {
            label: "Theme: Midnight Blue",
            action: |s| {
                s.theme
                    .set(PhazeTheme::from_variant(ThemeVariant::MidnightBlue));
            },
        },
        PaletteCommand {
            label: "Theme: Cyberpunk 2077",
            action: |s| {
                s.theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Cyberpunk));
            },
        },
        PaletteCommand {
            label: "Theme: Synthwave '84",
            action: |s| {
                s.theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Synthwave84));
            },
        },
        PaletteCommand {
            label: "Theme: Andromeda",
            action: |s| {
                s.theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Andromeda));
            },
        },
        PaletteCommand {
            label: "Theme: Dark",
            action: |s| {
                s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dark));
            },
        },
        PaletteCommand {
            label: "Theme: Dracula",
            action: |s| {
                s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dracula));
            },
        },
        PaletteCommand {
            label: "Theme: Tokyo Night",
            action: |s| {
                s.theme
                    .set(PhazeTheme::from_variant(ThemeVariant::TokyoNight));
            },
        },
        PaletteCommand {
            label: "Theme: Monokai",
            action: |s| {
                s.theme.set(PhazeTheme::from_variant(ThemeVariant::Monokai));
            },
        },
        PaletteCommand {
            label: "Theme: Nord Dark",
            action: |s| {
                s.theme
                    .set(PhazeTheme::from_variant(ThemeVariant::NordDark));
            },
        },
        PaletteCommand {
            label: "Theme: Matrix Green",
            action: |s| {
                s.theme
                    .set(PhazeTheme::from_variant(ThemeVariant::MatrixGreen));
            },
        },
        PaletteCommand {
            label: "Theme: Root Shell",
            action: |s| {
                s.theme
                    .set(PhazeTheme::from_variant(ThemeVariant::RootShell));
            },
        },
        PaletteCommand {
            label: "Theme: Light",
            action: |s| {
                s.theme.set(PhazeTheme::from_variant(ThemeVariant::Light));
            },
        },
        PaletteCommand {
            label: "Transform: To Uppercase",
            action: |s| s.transform_upper_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Transform: To Lowercase",
            action: |s| s.transform_lower_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Join Lines",
            action: |s| s.join_line_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Sort Lines (Ascending)",
            action: |s| s.sort_lines_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Toggle Relative Line Numbers",
            action: |s| s.relative_line_numbers.update(|v| *v = !*v),
        },
        PaletteCommand {
            label: "New Scratch File",
            action: |s| {
                let n = s.scratch_counter.get() + 1;
                s.scratch_counter.set(n);
                let p = std::path::PathBuf::from(format!("scratch://untitled-{n}"));
                s.scratch_paths.update(|v| v.push(p.clone()));
                s.open_file.set(Some(p));
            },
        },
        PaletteCommand {
            label: "Go to Line/Column",
            action: |s| {
                s.goto_overlay_open.set(true);
                s.goto_overlay_input.set(String::new());
            },
        },
        PaletteCommand {
            label: "Toggle Organize Imports on Save",
            action: |s| s.organize_imports_on_save.update(|v| *v = !*v),
        },
        PaletteCommand {
            label: "Transform: To Title Case",
            action: |s| s.transform_title_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Format Selection",
            action: |s| s.format_selection_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Save Without Formatting",
            action: |s| s.save_no_format_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Fold All",
            action: |s| s.fold_all_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Unfold All",
            action: |s| s.unfold_all_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Toggle Code Lens",
            action: |s| s.code_lens_visible.update(|v| *v = !*v),
        },
    ]
}

// ── File picker overlay (Ctrl+P) ──────────────────────────────────────────────

fn file_picker(state: IdeState) -> impl IntoView {
    let query = state.file_picker_query;
    let all_files = state.file_picker_files;
    let hovered: RwSignal<Option<usize>> = create_rw_signal(None);

    // When picker opens, walk workspace asynchronously (re-walk when root changes)
    let last_root: RwSignal<Option<std::path::PathBuf>> = create_rw_signal(None);
    let (files_tx, files_rx) = std::sync::mpsc::sync_channel::<Vec<std::path::PathBuf>>(1);
    let files_sig = floem::ext_event::create_signal_from_channel(files_rx);
    create_effect(move |_| {
        if let Some(files) = files_sig.get() {
            all_files.set(files);
        }
    });
    create_effect(move |_| {
        if !state.file_picker_open.get() {
            return;
        }
        let root = state.workspace_root.get();
        if last_root.get().as_ref() == Some(&root) {
            return;
        }
        last_root.set(Some(root.clone()));
        let tx = files_tx.clone();
        std::thread::spawn(move || {
            let files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&root)
                .max_depth(10)
                .into_iter()
                .flatten()
                .filter(|e| e.file_type().is_file())
                .filter(|e| {
                    let p = e.path().to_string_lossy();
                    !p.contains("/target/")
                        && !p.contains("/.git/")
                        && !p.contains("/node_modules/")
                        && !p.contains("/.cache/")
                })
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| e.into_path())
                .take(2000)
                .collect();
            let _ = tx.send(files);
        });
    });

    let filtered = move || -> Vec<(usize, std::path::PathBuf)> {
        let q = query.get().to_lowercase();
        all_files
            .get()
            .into_iter()
            .filter(|p| {
                if q.is_empty() {
                    return true;
                }
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                name.contains(&q) || p.to_string_lossy().to_lowercase().contains(&q)
            })
            .take(50)
            .enumerate()
            .collect()
    };

    let search_box = text_input(query).style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.width_full()
            .padding(10.0)
            .font_size(14.0)
            .color(p.text_primary)
            .background(p.bg_elevated)
            .border(1.0)
            .border_color(p.border_focus)
            .border_radius(6.0)
            .margin_bottom(8.0)
    });

    let items_view = scroll(
        dyn_stack(filtered, |(idx, _)| *idx, {
            let state = state.clone();
            move |(idx, path)| {
                let path_clone = path.clone();
                let root = state.workspace_root.get();
                let display = path
                    .strip_prefix(&root)
                    .ok()
                    .map(|r| r.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());
                let display2 = display.clone();
                let hov = hovered;
                let state = state.clone();
                container(
                    stack((
                        label(move || {
                            path_clone
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default()
                        })
                        .style({
                            let state = state.clone();
                            move |s| {
                                s.font_size(13.0)
                                    .color(state.theme.get().palette.text_primary)
                            }
                        }),
                        label(move || format!("  {}", display2)).style({
                            let state = state.clone();
                            move |s| {
                                s.font_size(11.0)
                                    .color(state.theme.get().palette.text_muted)
                                    .flex_grow(1.0)
                            }
                        }),
                    ))
                    .style(|s| s.items_center()),
                )
                .style({
                    let state = state.clone();
                    move |s| {
                        let t = state.theme.get();
                        let p = &t.palette;
                        s.width_full()
                            .padding_horiz(12.0)
                            .padding_vert(7.0)
                            .border_radius(4.0)
                            .background(if hov.get() == Some(idx) {
                                p.bg_elevated
                            } else {
                                floem::peniko::Color::TRANSPARENT
                            })
                            .cursor(floem::style::CursorStyle::Pointer)
                    }
                })
                .on_click_stop({
                    let state = state.clone();
                    let path2 = path.clone();
                    move |_| {
                        state.open_file.set(Some(path2.clone()));
                        state.file_picker_open.set(false);
                        state.file_picker_query.set(String::new());
                    }
                })
                .on_event_stop(EventListener::PointerEnter, move |_| {
                    hov.set(Some(idx));
                })
                .on_event_stop(EventListener::PointerLeave, move |_| {
                    hov.set(None);
                })
            }
        })
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(360.0));

    let empty_hint = container(label(|| "Searching workspace files…").style(move |s| {
        s.font_size(12.0)
            .color(state.theme.get().palette.text_muted)
    }))
    .style(move |s| {
        let empty = all_files.get().is_empty() && state.file_picker_open.get();
        s.width_full()
            .padding_vert(12.0)
            .items_center()
            .justify_center()
            .apply_if(!empty, |s| s.display(floem::style::Display::None))
    });

    let picker_box = stack((search_box, empty_hint, items_view))
        .style({
            let state = state.clone();
            move |s| {
                let t = state.theme.get();
                let p = &t.palette;
                s.flex_col()
                    .width(560.0)
                    .padding(16.0)
                    .background(p.bg_panel)
                    .border(1.0)
                    .border_color(p.glass_border)
                    .border_radius(10.0)
                    .box_shadow_h_offset(0.0)
                    .box_shadow_v_offset(4.0)
                    .box_shadow_blur(40.0)
                    .box_shadow_color(p.glow)
                    .box_shadow_spread(0.0)
            }
        })
        .on_event_stop(EventListener::KeyDown, {
            let state = state.clone();
            move |event| {
                if let Event::KeyDown(e) = event {
                    if e.key.logical_key == Key::Named(NamedKey::Escape) {
                        state.file_picker_open.set(false);
                        state.file_picker_query.set(String::new());
                    }
                }
            }
        });

    container(picker_box)
        .style({
            let state = state.clone();
            move |s| {
                let shown = state.file_picker_open.get();
                s.absolute()
                    .inset(0)
                    .items_start()
                    .justify_center()
                    .padding_top(80.0)
                    .background(floem::peniko::Color::from_rgba8(0, 0, 0, 160))
                    .z_index(ui_const::Z_FILE_PICKER)
                    .apply_if(!shown, |s| s.display(floem::style::Display::None))
            }
        })
        .on_click_stop({
            let state = state.clone();
            move |_| {
                state.file_picker_open.set(false);
                state.file_picker_query.set(String::new());
            }
        })
}

// ── Command palette overlay ───────────────────────────────────────────────────

fn command_palette(state: IdeState) -> impl IntoView {
    let query = state.command_palette_query;

    // Build a filtered list of matching commands driven by the query signal.
    #[allow(clippy::type_complexity)]
    let commands_list = move || -> Vec<(usize, &'static str, fn(IdeState))> {
        let q = query.get().to_lowercase();
        all_commands()
            .into_iter()
            .enumerate()
            .filter(|(_, cmd)| q.is_empty() || cmd.label.to_lowercase().contains(&q))
            .map(|(idx, cmd)| (idx, cmd.label, cmd.action))
            .collect()
    };

    let row_hovered: RwSignal<Option<usize>> = create_rw_signal(None);

    let search_box = text_input(query).style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.width_full()
            .padding(10.0)
            .font_size(14.0)
            .color(p.text_primary)
            .background(p.bg_elevated)
            .border(1.0)
            .border_color(p.border_focus)
            .border_radius(6.0)
            .margin_bottom(8.0)
    });

    let items_view = scroll(
        dyn_stack(commands_list, |(idx, lbl, _action)| (*idx, *lbl), {
            let state = state.clone();
            move |(idx, cmd_label, cmd_action)| {
                let hovered = row_hovered;
                let state = state.clone();
                container(label(move || cmd_label).style({
                    let state = state.clone();
                    move |s| {
                        s.font_size(13.0)
                            .color(state.theme.get().palette.text_primary)
                    }
                }))
                .style({
                    let state = state.clone();
                    move |s| {
                        let t = state.theme.get();
                        let p = &t.palette;
                        let is_hov = hovered.get() == Some(idx);
                        s.width_full()
                            .padding_horiz(12.0)
                            .padding_vert(8.0)
                            .border_radius(4.0)
                            .background(if is_hov {
                                p.bg_elevated
                            } else {
                                floem::peniko::Color::TRANSPARENT
                            })
                            .cursor(floem::style::CursorStyle::Pointer)
                    }
                })
                .on_click_stop({
                    let state = state.clone();
                    move |_| {
                        cmd_action(state.clone());
                        state.command_palette_open.set(false);
                        state.command_palette_query.set(String::new());
                    }
                })
                .on_event_stop(EventListener::PointerEnter, move |_| {
                    hovered.set(Some(idx));
                })
                .on_event_stop(EventListener::PointerLeave, move |_| {
                    hovered.set(None);
                })
            }
        })
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(320.0));

    let palette_box = stack((search_box, items_view))
        .style({
            let state = state.clone();
            move |s| {
                let t = state.theme.get();
                let p = &t.palette;
                s.flex_col()
                    .width(500.0)
                    .padding(16.0)
                    .background(p.bg_panel)
                    .border(1.0)
                    .border_color(p.glass_border)
                    .border_radius(10.0)
                    .box_shadow_h_offset(0.0)
                    .box_shadow_v_offset(4.0)
                    .box_shadow_blur(40.0)
                    .box_shadow_color(p.glow)
                    .box_shadow_spread(0.0)
            }
        })
        // Escape key closes the palette when it has focus
        .on_event_stop(EventListener::KeyDown, {
            let state = state.clone();
            move |event| {
                if let Event::KeyDown(e) = event {
                    if e.key.logical_key == Key::Named(floem::keyboard::NamedKey::Escape) {
                        state.command_palette_open.set(false);
                        state.command_palette_query.set(String::new());
                    }
                }
            }
        });

    // Dimmed full-screen backdrop — centers the palette box.
    // Clicking the backdrop (outside the palette) dismisses it.
    container(palette_box)
        .style({
            let state = state.clone();
            move |s| {
                let shown = state.command_palette_open.get();
                s.absolute()
                    .inset(0)
                    .items_center()
                    .justify_center()
                    .background(floem::peniko::Color::from_rgba8(0, 0, 0, 180))
                    .z_index(ui_const::Z_COMMAND_PALETTE)
                    .apply_if(!shown, |s| s.display(floem::style::Display::None))
            }
        })
        .on_click_stop({
            let state = state.clone();
            move |_| {
                state.command_palette_open.set(false);
                state.command_palette_query.set(String::new());
            }
        })
}

/// Cosmic canvas — absolute-positioned behind all UI panels.
/// Dark, clean, technical glass aesthetic: deep blue-black base + subtle hex
/// grid + faint corner glows. No large nebula blobs.
fn cosmic_bg_canvas(theme: RwSignal<PhazeTheme>) -> impl IntoView {
    canvas(move |cx, size| {
        let t = theme.get();
        let p = &t.palette;
        let w = size.width;
        let h = size.height;

        // 1. Deep blue-black base fill — slight blue tint, not pure black
        cx.fill(
            &floem::kurbo::Rect::ZERO.with_size(size),
            floem::peniko::Color::from_rgb8(8, 8, 22),
            0.0,
        );

        if !t.is_cosmic() {
            return;
        }

        // 2. Subtle hex grid — faint accent dots for an "engineered" technical feel
        let hex_size = 40.0;
        let grid_color = p.accent.with_alpha(0.09);
        let horiz_dist = hex_size * 3.0f64.sqrt();
        let vert_dist = hex_size * 1.5;

        for row in 0..((h / vert_dist) as i32 + 2) {
            for col in 0..((w / horiz_dist) as i32 + 2) {
                let x_offset = if row % 2 == 1 { horiz_dist / 2.0 } else { 0.0 };
                let x = col as f64 * horiz_dist + x_offset;
                let y = row as f64 * vert_dist;
                cx.fill(
                    &floem::kurbo::Circle::new(floem::kurbo::Point::new(x, y), 1.0),
                    grid_color,
                    0.0,
                );
            }
        }

        // 3. No blobs, no glows — just the clean hex grid on dark base.
    })
    // Absolute-positioned so it doesn't participate in flex layout but
    // covers the full parent container — rendered below all siblings.
    .style(|s| s.absolute().inset(0))
}

fn activity_bar_btn(icon_svg: &'static str, tab: Tab, state: IdeState) -> impl IntoView {
    let is_hovered = create_rw_signal(false);
    let active = move || state.left_panel_tab.get() == tab && state.show_left_panel.get();

    let icon_color = move |p: &crate::theme::PhazePalette| {
        if active() {
            p.accent
        } else {
            p.text_secondary
        }
    };

    container(phaze_icon(icon_svg, 22.0, icon_color, state.theme))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            let is_active = active();
            let is_hov = is_hovered.get();

            s.width(42.0)
                .height(42.0)
                .border_radius(10.0)
                .items_center()
                .justify_center()
                .cursor(floem::style::CursorStyle::Pointer)
                .margin_bottom(6.0)
                .transition(
                    floem::style::Background,
                    floem::style::Transition::linear(Duration::from_millis(150)),
                )
                .apply_if(is_active, |s| {
                    s.background(p.accent_dim)
                        .box_shadow_blur(16.0)
                        .box_shadow_color(p.glow)
                        .box_shadow_spread(1.0)
                })
                .apply_if(is_hov && !is_active, |s| {
                    s.background(p.bg_surface.with_alpha(0.3))
                })
        })
        .on_click_stop(move |_| {
            if state.left_panel_tab.get() == tab && state.show_left_panel.get() {
                state.show_left_panel.set(false);
                state.left_panel_width.set(0.0);
            } else {
                state.left_panel_tab.set(tab);
                state.show_left_panel.set(true);
                state.left_panel_width.set(300.0); // Slightly wider sidebar for premium feel
            }
        })
        .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
            is_hovered.set(true);
        })
        .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
            is_hovered.set(false);
        })
}

fn activity_bar(state: IdeState) -> impl IntoView {
    stack((
        activity_bar_btn(icons::EXPLORER, Tab::Explorer, state.clone()),
        activity_bar_btn(icons::SEARCH, Tab::Search, state.clone()),
        activity_bar_btn(icons::SOURCE_CONTROL, Tab::Git, state.clone()),
        activity_bar_btn(icons::LIST_CHECKS, Tab::Symbols, state.clone()),
        activity_bar_btn(icons::AI, Tab::AI, state.clone()),
        activity_bar_btn(icons::COMPOSE, Tab::Composer, state.clone()),
        activity_bar_btn(icons::DEBUG, Tab::Debug, state.clone()),
        activity_bar_btn(icons::REMOTE, Tab::Remote, state.clone()),
        activity_bar_btn(icons::CONTAINER, Tab::Containers, state.clone()),
        activity_bar_btn(icons::LIST_CHECKS, Tab::Makefile, state.clone()),
        activity_bar_btn(icons::GITHUB, Tab::GitHub, state.clone()),
        stack((
            activity_bar_btn(icons::EXTENSIONS, Tab::Extensions, state.clone()),
            activity_bar_btn(icons::SETTINGS, Tab::Settings, state.clone()),
            activity_bar_btn(icons::ACCOUNT, Tab::Account, state.clone()),
        ))
        .style(|s| s.flex_col().margin_top(floem::unit::PxPctAuto::Auto)),
    ))
    .style(|s| s.flex_col().padding(8.0).gap(2.0))
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.flex_col()
            .width(48.0)
            .height_full()
            .background(p.glass_bg)
            .border_right(1.0)
            .border_color(p.glass_border)
            .justify_between()
            .box_shadow_h_offset(3.0)
            .box_shadow_v_offset(0.0)
            .box_shadow_blur(16.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    })
}

fn coming_soon_panel(
    name: &'static str,
    description: &'static str,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    let header = container(label(move || name.to_uppercase()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(11.0)
            .font_weight(floem::text::Weight::BOLD)
            .color(p.text_muted)
            .padding_horiz(12.0)
            .padding_vert(8.0)
    }))
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full()
            .border_bottom(1.0)
            .border_color(p.glass_border)
    });

    let icon = container(label(move || "◇".to_string()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(32.0).color(p.accent).margin_bottom(12.0)
    }));

    let title = label(move || name.to_string()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(14.0)
            .font_weight(floem::text::Weight::BOLD)
            .color(p.text_primary)
            .margin_bottom(6.0)
    });

    let desc = label(move || description.to_string()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(11.5)
            .color(p.text_secondary)
            .margin_bottom(16.0)
    });

    let badge = container(label(|| "Coming Soon".to_string()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(10.0)
            .font_weight(floem::text::Weight::BOLD)
            .color(p.accent)
            .padding_horiz(10.0)
            .padding_vert(4.0)
    }))
    .style(move |s| {
        let p = theme.get().palette;
        s.border(1.0).border_color(p.accent).border_radius(12.0)
    });

    let body = container(
        stack((icon, title, desc, badge)).style(|s| s.flex_col().items_center().gap(0.0)),
    )
    .style(|s| {
        s.flex_grow(1.0)
            .width_full()
            .items_center()
            .justify_center()
    });

    container(stack((header, body)).style(|s| s.flex_col().width_full().height_full())).style(
        move |s| {
            let t = theme.get();
            s.width_full().height_full().background(t.palette.glass_bg)
        },
    )
}

fn left_panel(state: IdeState) -> impl IntoView {
    let explorer = explorer_panel(
        state.workspace_root,
        state.open_file,
        state.theme,
        state.open_tabs,
    );

    let explorer_wrap = container(explorer).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Explorer, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let search_wrap = container(search::search_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Search, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let git_wrap = container(git_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Git, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let debug_wrap = container(coming_soon_panel(
        "Run and Debug",
        "Run, step, and inspect your code with integrated debugger support.",
        state.theme,
    ))
    .style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Debug, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let extensions_wrap = container(extensions_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Extensions, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let remote_wrap = container(coming_soon_panel(
        "Remote Explorer",
        "Connect to remote machines, containers, and cloud environments via SSH.",
        state.theme,
    ))
    .style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Remote, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let container_wrap = container(coming_soon_panel(
        "Containers",
        "Manage Docker containers, images, and compose services.",
        state.theme,
    ))
    .style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Containers, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let makefile_wrap = container(coming_soon_panel(
        "Makefile",
        "Browse and run Makefile targets with a single click.",
        state.theme,
    ))
    .style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Makefile, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let github_wrap = container(github_actions_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::GitHub, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let symbols_wrap = container(symbol_outline_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Symbols, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let ai_wrap = container(crate::panels::ai_panel::ai_panel(state.theme)).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::AI, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let composer_wrap = container(crate::panels::composer::composer_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Composer, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let settings_wrap = container(settings_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Settings, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let account_wrap = container(coming_soon_panel(
        "Account",
        "Sign in to sync settings, manage PhazeAI Cloud features, and collaborate with your team.",
        state.theme,
    ))
    .style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.left_panel_tab.get() != Tab::Account, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    container(
        stack((
            explorer_wrap,
            search_wrap,
            git_wrap,
            symbols_wrap,
            debug_wrap,
            extensions_wrap,
            remote_wrap,
            container_wrap,
            makefile_wrap,
            github_wrap,
            ai_wrap,
            composer_wrap,
            settings_wrap,
            account_wrap,
        ))
        .style(|s| s.width_full().height_full()),
    )
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        let show = state.show_left_panel.get();
        let width = if show {
            state.left_panel_width.get()
        } else {
            0.0
        };
        s.width(width)
            .height_full()
            .background(p.glass_bg)
            .border_right(1.0)
            .border_color(p.glass_border)
            .box_shadow_h_offset(6.0)
            .box_shadow_v_offset(0.0)
            .box_shadow_blur(12.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
            .apply_if(!show, |s| s.display(floem::style::Display::None))
    })
}

fn bottom_panel_tab(label_str: &'static str, tab: Tab, state: IdeState) -> impl IntoView {
    let is_hovered = create_rw_signal(false);
    container(label(move || label_str))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            let active = state.bottom_panel_tab.get() == tab;
            let hovered = is_hovered.get();
            s.padding_horiz(12.0)
                .padding_vert(6.0)
                .font_size(11.0)
                .color(if active { p.accent } else { p.text_muted })
                .background(if active {
                    p.bg_surface
                } else if hovered {
                    p.bg_elevated
                } else {
                    floem::peniko::Color::TRANSPARENT
                })
                .cursor(floem::style::CursorStyle::Pointer)
                .apply_if(active, |s| s.border_top(2.0).border_color(p.accent))
        })
        .on_click_stop(move |_| {
            state.bottom_panel_tab.set(tab);
            state.show_bottom_panel.set(true);
        })
        .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
            is_hovered.set(true);
        })
        .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
            is_hovered.set(false);
        })
}

/// Like `bottom_panel_tab` but with a reactive label closure so the tab text can show counts.
fn bottom_panel_tab_dyn<F>(label_fn: F, tab: Tab, state: IdeState) -> impl IntoView
where
    F: Fn() -> String + 'static,
{
    let is_hovered = create_rw_signal(false);
    container(label(label_fn))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            let active = state.bottom_panel_tab.get() == tab;
            let hovered = is_hovered.get();
            s.padding_horiz(12.0)
                .padding_vert(6.0)
                .font_size(11.0)
                .color(if active { p.accent } else { p.text_muted })
                .background(if active {
                    p.bg_surface
                } else if hovered {
                    p.bg_elevated
                } else {
                    floem::peniko::Color::TRANSPARENT
                })
                .cursor(floem::style::CursorStyle::Pointer)
                .apply_if(active, |s| s.border_top(2.0).border_color(p.accent))
        })
        .on_click_stop(move |_| {
            state.bottom_panel_tab.set(tab);
            state.show_bottom_panel.set(true);
        })
        .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
            is_hovered.set(true);
        })
        .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
            is_hovered.set(false);
        })
}

fn status_bar(state: IdeState) -> impl IntoView {
    // Cloud sign-in indicator (left-most element)
    // let cloud_btn = container(label(|| "☁ Sign in"))
    //     .style(move |s| {
    //         let p = state.theme.get().palette;
    //         s.font_size(10.0)
    //             .padding_horiz(8.0)
    //             .padding_vert(2.0)
    //             .margin_right(8.0)
    //             .border_radius(3.0)
    //             .cursor(floem::style::CursorStyle::Pointer)
    //             .color(p.accent)
    //             .background(p.accent_dim)
    //     })
    //     .on_click_stop(|_| {
    //         // Open PhazeAI cloud sign-in in the system browser.
    //         let url = phazeai_cloud::auth::login_url();
    //         let opener = if cfg!(target_os = "macos") {
    //             "open"
    //         } else if cfg!(target_os = "windows") {
    //             "cmd"
    //         } else {
    //             "xdg-open"
    //         };
    //         let mut cmd = std::process::Command::new(opener);
    //         if cfg!(target_os = "windows") {
    //             cmd.args(["/C", "start", "", url]);
    //         } else {
    //             cmd.arg(url);
    //         }
    //         let _ = cmd.spawn();
    //     });

    // Branch clickable button — click to open branch picker overlay
    let branch_btn = {
        let s = state.clone();
        let s2 = state.clone();
        let is_hov = create_rw_signal(false);
        container(
            stack((
                phaze_icon(icons::BRANCH, 12.0, move |p| p.accent, state.theme),
                label(move || format!(" {} ", s.git_branch.get())).style(move |s2| {
                    s2.color(state.theme.get().palette.text_secondary)
                        .font_size(11.0)
                }),
            ))
            .style(|s| s.items_center()),
        )
        .style(move |s| {
            let p = s2.theme.get().palette;
            s.padding_horiz(6.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .background(if is_hov.get() {
                    p.bg_elevated
                } else {
                    floem::peniko::Color::TRANSPARENT
                })
        })
        .on_click_stop({
            let s3 = state.clone();
            let (branch_tx, branch_rx) = std::sync::mpsc::sync_channel::<Vec<String>>(1);
            let branch_sig = floem::ext_event::create_signal_from_channel(branch_rx);
            let picker_open_sig = state.branch_picker_open;
            let branch_list_sig = state.branch_list;
            create_effect(move |_| {
                if let Some(branches) = branch_sig.get() {
                    branch_list_sig.set(branches);
                    picker_open_sig.set(true);
                }
            });
            move |_| {
                let root = s3.workspace_root.get();
                let tx = branch_tx.clone();
                std::thread::spawn(move || {
                    let branches = std::process::Command::new("git")
                        .args(["branch", "--list"])
                        .current_dir(&root)
                        .output()
                        .ok()
                        .map(|out| {
                            String::from_utf8_lossy(&out.stdout)
                                .lines()
                                .map(|l| l.trim_start_matches(['*', ' ']).trim().to_string())
                                .filter(|l| !l.is_empty())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let _ = tx.send(branches);
                });
            }
        })
        .on_event_stop(EventListener::PointerEnter, move |_| is_hov.set(true))
        .on_event_stop(EventListener::PointerLeave, move |_| is_hov.set(false))
    };

    let left = stack((
        branch_btn,
        label(|| "   ").style(|s| s.font_size(11.0)),
        phaze_icon(icons::BRANCH, 12.0, move |p| p.accent, state.theme),
        label(move || format!(" {}", state.ai_model.get())).style(move |s| {
            s.color(state.theme.get().palette.text_secondary)
                .font_size(11.0)
        }),
    ))
    .style(|s| s.items_center().padding_horiz(8.0));

    // VIM mode toggle button — shows INSERT/NORMAL when active
    let vim_btn = {
        let s = state.clone();
        let s_label = state.clone();
        container(label(move || {
            if !s_label.vim_mode.get() {
                return "NORMAL".to_string();
            }
            if s_label.vim_normal_mode.get() {
                "-- NORMAL --".to_string()
            } else {
                "-- INSERT --".to_string()
            }
        }))
        .style(move |s2| {
            let p = state.theme.get().palette;
            let vim = state.vim_mode.get();
            let normal = state.vim_normal_mode.get();
            s2.font_size(10.0)
                .padding_horiz(6.0)
                .padding_vert(2.0)
                .margin_right(6.0)
                .border_radius(3.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .color(if vim { p.bg_base } else { p.text_muted })
                .background(if vim && normal {
                    p.warning
                } else if vim {
                    p.accent
                } else {
                    p.bg_elevated
                })
                .border(1.0)
                .border_color(if vim { p.accent } else { p.border })
        })
        .on_click_stop(move |_| {
            let s2 = s.clone();
            s2.vim_mode.update(|v| *v = !*v);
            // When enabling vim mode, start in Normal mode.
            if s2.vim_mode.get() {
                s2.vim_normal_mode.set(true);
            } else {
                s2.vim_normal_mode.set(false);
            }
            session_update(|s| {
                s.vim_mode = s2.vim_mode.get();
            });
        })
    };

    let right = stack((
        // Line / column indicator — reads from active_cursor (set by editor on every move).
        label(move || {
            if let Some((_, line, col)) = state.active_cursor.get() {
                format!("Ln {},  Col {}  ", line + 1, col + 1)
            } else {
                String::new()
            }
        })
        .style(move |s| {
            s.color(state.theme.get().palette.text_secondary)
                .font_size(11.0)
        }),
        // LSP diagnostic counts — live from the reactive diagnostics signal.
        label(move || {
            let diags = state.diagnostics.get();
            let errs = diags
                .iter()
                .filter(|d| d.severity == DiagSeverity::Error)
                .count();
            let warns = diags
                .iter()
                .filter(|d| d.severity == DiagSeverity::Warning)
                .count();
            if errs == 0 && warns == 0 {
                String::new()
            } else {
                format!("⊗ {errs}  ⚠ {warns}  ")
            }
        })
        .style(move |s| {
            let p = state.theme.get().palette;
            let has_errs = state
                .diagnostics
                .get()
                .iter()
                .any(|d| d.severity == DiagSeverity::Error);
            s.font_size(11.0)
                .color(if has_errs { p.error } else { p.warning })
        }),
        vim_btn,
        // Diagnostic message for current cursor line (from LSP).
        label(move || {
            if let Some((ref path, line, _col)) = state.active_cursor.get() {
                let diags = state.diagnostics.get();
                // Find first diagnostic on current line (1-based line = line+1).
                let cur_line_1 = line + 1;
                if let Some(d) = diags
                    .iter()
                    .find(|d| d.path == *path && d.line == cur_line_1)
                {
                    let prefix = match d.severity {
                        DiagSeverity::Error => "⊗ ",
                        DiagSeverity::Warning => "⚠ ",
                        DiagSeverity::Info => "ℹ ",
                        DiagSeverity::Hint => "💡 ",
                    };
                    let msg = if d.message.len() > 60 {
                        let end = d.message.floor_char_boundary(60);
                        format!("{}{}…  ", prefix, &d.message[..end])
                    } else {
                        format!("{}{}  ", prefix, d.message)
                    };
                    return msg;
                }
            }
            String::new()
        })
        .style(move |s| {
            let p = state.theme.get().palette;
            let has_err = state
                .active_cursor
                .get()
                .map(|(ref path, line, _)| {
                    state.diagnostics.get().iter().any(|d| {
                        d.path == *path && d.line == line + 1 && d.severity == DiagSeverity::Error
                    })
                })
                .unwrap_or(false);
            s.font_size(10.0)
                .color(if has_err { p.error } else { p.warning })
        }),
        // LSP progress indicator — shown while indexing, hidden when idle.
        label(move || {
            state
                .lsp_progress
                .get()
                .map(|msg| {
                    if msg.len() > 40 {
                        let end = msg.floor_char_boundary(40);
                        format!("{}…  ", &msg[..end])
                    } else {
                        format!("{msg}  ")
                    }
                })
                .unwrap_or_default()
        })
        .style(move |s| {
            s.color(state.theme.get().palette.text_muted)
                .font_size(10.0)
                .apply_if(state.lsp_progress.get().is_none(), |s| {
                    s.display(floem::style::Display::None)
                })
        }),
        label(|| "AI Ready  ")
            .style(move |s| s.color(state.theme.get().palette.success).font_size(11.0)),
        // Git blame for current cursor line
        label(move || {
            let blame = state.active_blame.get();
            if blame.is_empty() {
                String::new()
            } else {
                format!("  {blame}  ")
            }
        })
        .style(move |s| {
            let p = state.theme.get().palette;
            s.font_size(10.0)
                .color(p.text_muted)
                .apply_if(state.active_blame.get().is_empty(), |s| {
                    s.display(floem::style::Display::None)
                })
        }),
        // Dynamic encoding + line ending indicator — clickable to toggle CRLF/LF
        {
            let le_state = state.clone();
            let le_theme = state.theme;
            let le_hov = create_rw_signal(false);
            container(
                label(move || format!("UTF-8 {}  ", le_state.line_ending.get())).style(move |s| {
                    let p = le_theme.get().palette;
                    s.color(if le_hov.get() { p.accent } else { p.text_muted })
                        .font_size(11.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                }),
            )
            .on_click_stop(move |_| {
                // Toggle line ending and convert file bytes
                let current = state.line_ending.get();
                let new_le: &'static str = if current == "CRLF" { "LF" } else { "CRLF" };
                state.line_ending.set(new_le);
                // Convert open file bytes
                if let Some(path) = state.open_file.get_untracked() {
                    if path.exists() {
                        let toast = state.status_toast;
                        if let Ok(bytes) = std::fs::read(&path) {
                            let converted = if new_le == "LF" {
                                // Remove all \r
                                bytes
                                    .into_iter()
                                    .filter(|&b| b != b'\r')
                                    .collect::<Vec<_>>()
                            } else {
                                // Add \r before each \n that isn't already preceded by \r
                                let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 20);
                                let mut prev = 0u8;
                                for b in bytes {
                                    if b == b'\n' && prev != b'\r' {
                                        out.push(b'\r');
                                    }
                                    out.push(b);
                                    prev = b;
                                }
                                out
                            };
                            let _ = std::fs::write(&path, &converted);
                            show_toast(toast, format!("Converted to {new_le}"));
                        }
                    }
                }
            })
            .on_event_stop(EventListener::PointerEnter, move |_| le_hov.set(true))
            .on_event_stop(EventListener::PointerLeave, move |_| le_hov.set(false))
        },
        label(move || {
            state
                .open_file
                .get()
                .as_ref()
                .and_then(|p| p.extension())
                .map(|e| match e.to_str().unwrap_or("") {
                    "rs" => "Rust  ",
                    "py" => "Python  ",
                    "js" | "ts" => "TypeScript  ",
                    "toml" => "TOML  ",
                    "md" => "Markdown  ",
                    _ => "Text  ",
                })
                .unwrap_or("  ")
                .to_string()
        })
        .style(move |s| {
            s.color(state.theme.get().palette.text_muted)
                .font_size(11.0)
        }),
        // Read-only indicator
        {
            let ro_theme = state.theme;
            let ro_sig = state.active_readonly;
            label(move || if ro_sig.get() { "🔒 READ-ONLY  " } else { "" }).style(move |s| {
                let p = ro_theme.get().palette;
                s.color(p.error)
                    .font_size(11.0)
                    .apply_if(!ro_sig.get(), |s| s.display(floem::style::Display::None))
            })
        },
    ))
    .style(|s| s.items_center().padding_horiz(8.0));

    stack((left, right)).style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.height(24.0)
            .width_full()
            .background(p.glass_bg)
            .border_top(1.0)
            .border_color(p.glass_border)
            .items_center()
            .justify_between()
            // Neon top-edge glow on status bar
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(-2.0)
            .box_shadow_blur(14.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    })
}

fn problems_view(state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal as crws;
    let diags = state.diagnostics;
    let theme = state.theme;
    let open_file = state.open_file;
    let goto_line = state.goto_line;

    // Filter toggles
    let show_errors = crws(true);
    let show_warnings = crws(true);

    let err_btn = container(label(move || {
        let n = diags
            .get()
            .iter()
            .filter(|d| d.severity == DiagSeverity::Error)
            .count();
        format!("⊗ Errors ({n})")
    }))
    .style(move |s| {
        let p = theme.get().palette;
        let on = show_errors.get();
        s.font_size(11.0)
            .padding_horiz(8.0)
            .padding_vert(3.0)
            .border_radius(4.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .color(if on { p.bg_base } else { p.error })
            .background(if on { p.error } else { p.bg_elevated })
    })
    .on_click_stop(move |_| {
        show_errors.update(|v| *v = !*v);
    });

    let warn_btn = container(label(move || {
        let n = diags
            .get()
            .iter()
            .filter(|d| d.severity == DiagSeverity::Warning)
            .count();
        format!("⚠ Warnings ({n})")
    }))
    .style(move |s| {
        let p = theme.get().palette;
        let on = show_warnings.get();
        s.font_size(11.0)
            .padding_horiz(8.0)
            .padding_vert(3.0)
            .border_radius(4.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .color(if on { p.bg_base } else { p.warning })
            .background(if on { p.warning } else { p.bg_elevated })
    })
    .on_click_stop(move |_| {
        show_warnings.update(|v| *v = !*v);
    });

    let filter_bar = stack((err_btn, warn_btn)).style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .gap(6.0)
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(p.border)
            .width_full()
            .items_center()
    });

    let empty_msg = container(
        label(move || {
            if diags.get().is_empty() {
                "No problems detected ✓".to_string()
            } else {
                String::new()
            }
        })
        .style(move |s| s.font_size(12.0).color(theme.get().palette.success)),
    )
    .style(move |s| {
        s.width_full()
            .padding(16.0)
            .apply_if(!diags.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let list = scroll(
        dyn_stack(
            move || {
                safe_get(diags, Vec::new())
                    .into_iter()
                    .filter(|d| match d.severity {
                        DiagSeverity::Error => show_errors.get(),
                        DiagSeverity::Warning => show_warnings.get(),
                        _ => true,
                    })
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(idx, _)| *idx,
            {
                let theme = state.theme;
                move |(_, entry): (usize, DiagEntry)| {
                    let sev = entry.severity;
                    let icon = match sev {
                        DiagSeverity::Error => "⊗",
                        DiagSeverity::Warning => "⚠",
                        DiagSeverity::Info => "ℹ",
                        DiagSeverity::Hint => "○",
                    };
                    let filename = entry
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "?".to_string());
                    let loc = format!("{}:{}", entry.line, entry.col);
                    let msg = entry.message.clone();
                    let path = entry.path.clone();
                    let line_no = entry.line;
                    let hovered = crws(false);

                    container(
                        stack((
                            label(move || icon.to_string()).style(move |s| {
                                let p = theme.get().palette;
                                let c = match sev {
                                    DiagSeverity::Error => p.error,
                                    DiagSeverity::Warning => p.warning,
                                    DiagSeverity::Info => p.accent,
                                    _ => p.text_muted,
                                };
                                s.font_size(13.0).color(c).margin_right(8.0)
                            }),
                            label(move || msg.clone()).style(move |s| {
                                s.font_size(12.0)
                                    .color(theme.get().palette.text_primary)
                                    .flex_grow(1.0)
                            }),
                            label(move || filename.clone()).style(move |s| {
                                s.font_size(11.0)
                                    .color(theme.get().palette.accent)
                                    .margin_left(8.0)
                            }),
                            label(move || loc.clone()).style(move |s| {
                                s.font_size(10.0)
                                    .color(theme.get().palette.text_muted)
                                    .margin_left(6.0)
                            }),
                        ))
                        .style(|s| s.flex_row().items_center().width_full()),
                    )
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.width_full()
                            .padding_horiz(12.0)
                            .padding_vert(5.0)
                            .cursor(floem::style::CursorStyle::Pointer)
                            .background(if hovered.get() {
                                p.bg_elevated
                            } else {
                                floem::peniko::Color::TRANSPARENT
                            })
                    })
                    .on_click_stop(move |_| {
                        open_file.set(Some(path.clone()));
                        goto_line.set(line_no);
                    })
                    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                        hovered.set(true);
                    })
                    .on_event_stop(
                        floem::event::EventListener::PointerLeave,
                        move |_| {
                            hovered.set(false);
                        },
                    )
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        s.width_full()
            .flex_grow(1.0)
            .apply_if(diags.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    stack((filter_bar, empty_msg, list)).style(|s| s.flex_col().width_full().height_full())
}

fn references_view(state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal as crws;
    let refs = state.references;
    let theme = state.theme;
    let open_file = state.open_file;
    let goto_line = state.goto_line;

    let empty_msg = container(
        label(move || {
            if refs.get().is_empty() {
                "Press Shift+F12 on a symbol to find all references.".to_string()
            } else {
                String::new()
            }
        })
        .style(move |s| s.font_size(12.0).color(theme.get().palette.text_muted)),
    )
    .style(move |s| {
        s.width_full()
            .padding(16.0)
            .apply_if(!refs.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let count_label = label(move || {
        let n = refs.get().len();
        if n == 0 {
            String::new()
        } else {
            format!("{n} reference{}", if n == 1 { "" } else { "s" })
        }
    })
    .style(move |s| {
        s.font_size(11.0)
            .color(theme.get().palette.text_muted)
            .padding_horiz(12.0)
            .padding_vert(4.0)
            .width_full()
            .apply_if(refs.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let list = scroll(
        dyn_stack(
            move || {
                safe_get(refs, Vec::new())
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(idx, _)| *idx,
            {
                let theme = state.theme;
                move |(_, entry): (usize, ReferenceEntry)| {
                    let filename = entry
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "?".to_string());
                    let loc = format!(":{}", entry.line);
                    let path = entry.path.clone();
                    let line_no = entry.line;
                    let hovered = crws(false);

                    // Show a snippet of the line if possible
                    let snippet = std::fs::read_to_string(&entry.path)
                        .ok()
                        .and_then(|c| {
                            c.lines()
                                .nth(entry.line.saturating_sub(1) as usize)
                                .map(|l| l.trim().to_string())
                        })
                        .unwrap_or_default();

                    container(
                        stack((
                            label(move || filename.clone()).style(move |s| {
                                s.font_size(12.0)
                                    .color(theme.get().palette.accent)
                                    .font_weight(floem::text::Weight::SEMIBOLD)
                            }),
                            label(move || loc.clone()).style(move |s| {
                                s.font_size(11.0)
                                    .color(theme.get().palette.text_muted)
                                    .margin_right(8.0)
                            }),
                            label(move || snippet.clone()).style(move |s| {
                                s.font_size(11.5)
                                    .color(theme.get().palette.text_secondary)
                                    .flex_grow(1.0)
                                    .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                            }),
                        ))
                        .style(|s| s.flex_row().items_center().width_full()),
                    )
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.width_full()
                            .padding_horiz(12.0)
                            .padding_vert(5.0)
                            .cursor(floem::style::CursorStyle::Pointer)
                            .background(if hovered.get() {
                                p.bg_elevated
                            } else {
                                floem::peniko::Color::TRANSPARENT
                            })
                    })
                    .on_click_stop(move |_| {
                        open_file.set(Some(path.clone()));
                        goto_line.set(line_no);
                    })
                    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                        hovered.set(true);
                    })
                    .on_event_stop(
                        floem::event::EventListener::PointerLeave,
                        move |_| {
                            hovered.set(false);
                        },
                    )
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        s.width_full()
            .flex_grow(1.0)
            .apply_if(refs.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    stack((count_label, empty_msg, list)).style(|s| s.flex_col().width_full().height_full())
}

fn output_view(state: IdeState) -> impl IntoView {
    let log = state.output_log;
    let theme = state.theme;
    scroll(
        dyn_stack(
            move || {
                safe_get(log, Vec::new())
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(idx, _)| *idx,
            move |(_, line): (usize, String)| {
                let line2 = line.clone();
                label(move || line.clone()).style(move |s| {
                    let p = theme.get().palette;
                    let color = if line2.starts_with("[error]")
                        || line2.contains("error") && !line2.contains("0 errors")
                    {
                        p.error
                    } else if line2.starts_with("[warn]") || line2.contains("warning") {
                        p.warning
                    } else {
                        p.text_secondary
                    };
                    s.font_size(11.5)
                        .color(color)
                        .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                        .padding_horiz(12.0)
                        .padding_vert(1.0)
                        .width_full()
                })
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().height_full())
}

fn debug_console_view(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    container(
        stack((
            label(|| "▷  No active debug session").style(move |s| {
                let p = theme.get().palette;
                s.font_size(13.0).color(p.text_muted)
            }),
            label(|| "Run a debug configuration to start a session.").style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0).color(p.text_muted).margin_top(4.0)
            }),
        ))
        .style(|s| s.flex_col().gap(4.0).items_center()),
    )
    .style(|s| s.width_full().height_full().items_center().justify_center())
}

fn ports_view(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    container(
        stack((
            label(|| "No forwarded ports").style(move |s| {
                let p = theme.get().palette;
                s.font_size(13.0).color(p.text_muted)
            }),
            label(|| "Ports forwarded by running processes will appear here.").style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0).color(p.text_muted).margin_top(4.0)
            }),
        ))
        .style(|s| s.flex_col().gap(4.0).items_center()),
    )
    .style(|s| s.width_full().height_full().items_center().justify_center())
}

/// Symbol outline panel — displayed in the left sidebar under the "Symbols" tab.
fn symbol_outline_panel(state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal as crws;
    let symbols = state.doc_symbols;
    let theme = state.theme;
    let open_file = state.open_file;
    let goto_line = state.goto_line;
    let lsp_cmd = state.lsp_cmd.clone();

    // Refresh button
    let refresh_btn = container(label(|| " ↺ ".to_string()).style(move |s| {
        s.font_size(13.0)
            .color(theme.get().palette.text_muted)
            .cursor(floem::style::CursorStyle::Pointer)
    }))
    .on_click_stop(move |_| {
        if let Some(path) = open_file.get_untracked() {
            let _ = lsp_cmd.send(LspCommand::RequestDocumentSymbols { path });
        }
    });

    let header = stack((
        label(|| "OUTLINE".to_string()).style(move |s| {
            s.font_size(11.0)
                .font_weight(floem::text::Weight::BOLD)
                .color(theme.get().palette.text_muted)
                .flex_grow(1.0)
                .padding_left(12.0)
        }),
        refresh_btn,
    ))
    .style(|s| s.flex_row().items_center().padding_vert(6.0).width_full());

    let empty_msg = container(
        label(move || {
            if symbols.get().is_empty() {
                "No symbols found in file.".to_string()
            } else {
                String::new()
            }
        })
        .style(move |s| {
            s.font_size(12.0)
                .color(theme.get().palette.text_muted)
                .padding(12.0)
        }),
    )
    .style(move |s| {
        s.apply_if(!symbols.get().is_empty(), |s| {
            s.display(floem::style::Display::None)
        })
    });

    let list = scroll(
        dyn_stack(
            move || {
                safe_get(symbols, Vec::new())
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(i, _)| *i,
            {
                let theme = state.theme;
                move |(_, sym): (usize, SymbolEntry)| {
                    let hovered = crws(false);
                    let name = sym.name.clone();
                    let kind = sym.kind.clone();
                    let line_no = sym.line;
                    let indent = sym.depth * 12;

                    let kind_color = match kind.as_str() {
                        "fn" => floem::peniko::Color::from_rgb8(86, 156, 214),
                        "struct" => floem::peniko::Color::from_rgb8(78, 201, 176),
                        "enum" => floem::peniko::Color::from_rgb8(197, 134, 192),
                        "trait" => floem::peniko::Color::from_rgb8(220, 220, 170),
                        "impl" => floem::peniko::Color::from_rgb8(150, 200, 150),
                        "mod" => floem::peniko::Color::from_rgb8(200, 200, 100),
                        _ => floem::peniko::Color::from_rgb8(180, 180, 180),
                    };

                    container(
                        stack((
                            label(move || format!("{kind} ")).style(move |s| {
                                s.font_size(11.0)
                                    .color(kind_color)
                                    .font_family("JetBrains Mono, monospace".to_string())
                            }),
                            label(move || name.clone()).style(move |s| {
                                s.font_size(12.0)
                                    .color(theme.get().palette.text_primary)
                                    .font_family("JetBrains Mono, monospace".to_string())
                            }),
                        ))
                        .style(move |s| s.flex_row().items_center().padding_left(indent as f64)),
                    )
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.width_full()
                            .padding_horiz(8.0)
                            .padding_vert(3.0)
                            .cursor(floem::style::CursorStyle::Pointer)
                            .background(if hovered.get() {
                                p.bg_elevated
                            } else {
                                floem::peniko::Color::TRANSPARENT
                            })
                    })
                    .on_click_stop(move |_| {
                        goto_line.set(line_no);
                    })
                    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                        hovered.set(true);
                    })
                    .on_event_stop(
                        floem::event::EventListener::PointerLeave,
                        move |_| {
                            hovered.set(false);
                        },
                    )
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| s.flex_grow(1.0).width_full());

    stack((header, empty_msg, list)).style(|s| s.flex_col().width_full().height_full())
}

/// Git diff viewer — shown in the bottom panel "GIT DIFF" tab.
fn git_diff_view(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    let open_file = state.open_file;

    // Reactive signal holding the parsed diff lines (text + color-kind).
    // 0=context, 1=added (+), 2=removed (-), 3=header (@@/---/+++)
    let diff_lines: floem::reactive::RwSignal<Vec<(String, u8)>> =
        floem::reactive::create_rw_signal(vec![]);

    // Whenever the open file changes, run git diff in background.
    {
        let diff_sig = diff_lines;
        let (diff_tx, diff_rx) = std::sync::mpsc::sync_channel::<Vec<(String, u8)>>(1);
        let diff_result_sig = floem::ext_event::create_signal_from_channel(diff_rx);
        floem::reactive::create_effect(move |_| {
            if let Some(lines) = diff_result_sig.get() {
                diff_sig.set(lines);
            }
        });
        floem::reactive::create_effect(move |_| {
            if let Some(path) = open_file.get() {
                let tx = diff_tx.clone();
                std::thread::spawn(move || {
                    let lines = run_git_diff(&path);
                    let _ = tx.send(lines);
                });
            } else {
                diff_sig.set(vec![]);
            }
        });
    }

    let empty_msg = container(
        label(move || {
            if diff_lines.get().is_empty() {
                "No changes in file (git diff is clean).".to_string()
            } else {
                String::new()
            }
        })
        .style(move |s| {
            s.font_size(12.0)
                .color(theme.get().palette.text_muted)
                .padding(16.0)
        }),
    )
    .style(move |s| {
        s.apply_if(!diff_lines.get().is_empty(), |s| {
            s.display(floem::style::Display::None)
        })
    });

    let diff_scroll = scroll(
        dyn_stack(
            move || {
                safe_get(diff_lines, Vec::new())
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(i, _)| *i,
            move |(_, (text, kind)): (usize, (String, u8))| {
                let color = match kind {
                    1 => floem::peniko::Color::from_rgba8(80, 200, 80, 255), // added
                    2 => floem::peniko::Color::from_rgba8(220, 60, 60, 255), // removed
                    3 => floem::peniko::Color::from_rgba8(100, 160, 255, 255), // header
                    _ => theme.get().palette.text_secondary,                 // context
                };
                let bg = match kind {
                    1 => floem::peniko::Color::from_rgba8(40, 80, 40, 120),
                    2 => floem::peniko::Color::from_rgba8(80, 20, 20, 120),
                    3 => floem::peniko::Color::from_rgba8(20, 40, 80, 100),
                    _ => floem::peniko::Color::TRANSPARENT,
                };
                container(label(move || text.clone()).style(move |s| {
                    s.font_size(12.0)
                        .color(color)
                        .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                }))
                .style(move |s| {
                    s.width_full()
                        .padding_horiz(8.0)
                        .padding_vert(1.0)
                        .background(bg)
                })
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        s.flex_grow(1.0)
            .width_full()
            .apply_if(diff_lines.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    stack((empty_msg, diff_scroll)).style(|s| s.flex_col().width_full().height_full())
}

/// Run `git diff HEAD -- <path>` and return colored diff lines.
fn run_git_diff(path: &std::path::Path) -> Vec<(String, u8)> {
    let dir = path.parent().unwrap_or(path);
    let out = std::process::Command::new("git")
        .args(["diff", "HEAD", "--", path.to_str().unwrap_or("")])
        .current_dir(dir)
        .output();
    let output = match out {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        // Try diff against staged (index) as fallback
        let out2 = std::process::Command::new("git")
            .args(["diff", "--", path.to_str().unwrap_or("")])
            .current_dir(dir)
            .output();
        let text2 = match out2 {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(_) => return vec![],
        };
        if text2.trim().is_empty() {
            return vec![];
        }
        parse_diff_output(&text2)
    } else {
        parse_diff_output(&text)
    }
}

fn parse_diff_output(text: &str) -> Vec<(String, u8)> {
    text.lines()
        .map(|line| {
            let kind = if line.starts_with('+') && !line.starts_with("+++") {
                1
            } else if line.starts_with('-') && !line.starts_with("---") {
                2
            } else if line.starts_with("@@") || line.starts_with("---") || line.starts_with("+++") {
                3
            } else {
                0u8
            };
            (line.to_string(), kind)
        })
        .collect()
}

fn bottom_panel(state: IdeState) -> impl IntoView {
    let current_tab = state.bottom_panel_tab;
    let maximized = state.bottom_panel_maximized;

    container(
        stack((
            // Tab bar — double-click to maximize/restore
            stack((
                bottom_panel_tab("TERMINAL", Tab::Terminal, state.clone()),
                bottom_panel_tab_dyn(
                    {
                        let diags = state.diagnostics;
                        move || {
                            let n = diags.get().len();
                            if n == 0 {
                                "PROBLEMS".to_string()
                            } else {
                                format!("PROBLEMS ({})", n)
                            }
                        }
                    },
                    Tab::Problems,
                    state.clone(),
                ),
                bottom_panel_tab("REFERENCES", Tab::References, state.clone()),
                bottom_panel_tab("GIT DIFF", Tab::GitDiff, state.clone()),
                bottom_panel_tab("OUTPUT", Tab::Output, state.clone()),
                bottom_panel_tab("DEBUG CONSOLE", Tab::DebugConsole, state.clone()),
                bottom_panel_tab("PORTS", Tab::Ports, state.clone()),
                // Close button
                phaze_icon(icons::CLOSE, 12.0, move |p| p.text_muted, state.theme)
                    .style(move |s| {
                        s.margin_left(floem::unit::PxPctAuto::Auto)
                            .padding(4.0)
                            .cursor(floem::style::CursorStyle::Pointer)
                    })
                    .on_click_stop(move |_| {
                        state.show_bottom_panel.set(false);
                    }),
            ))
            .style(move |s| {
                let t = state.theme.get();
                s.width_full()
                    .height(32.0)
                    .background(t.palette.bg_elevated)
                    .border_bottom(1.0)
                    .border_color(t.palette.border)
                    .items_center()
                    .padding_horiz(12.0)
                    .gap(16.0)
            })
            .on_event_stop(EventListener::PointerDown, move |e| {
                // Double-click on tab bar → maximize/restore
                if let Event::PointerDown(pe) = e {
                    if pe.count == 2 {
                        maximized.update(|v| *v = !*v);
                    }
                }
            }),
            // Content
            stack((
                container(terminal_panel(
                    state.theme,
                    state.show_bottom_panel,
                    state.show_left_panel,
                    state.show_right_panel,
                    state.file_picker_open,
                    state.command_palette_open,
                    state.run_in_terminal_text,
                ))
                .style(move |s| {
                    s.width_full()
                        .height_full()
                        .apply_if(current_tab.get() != Tab::Terminal, |s| {
                            s.display(floem::style::Display::None)
                        })
                }),
                container(problems_view(state.clone())).style(move |s| {
                    s.width_full()
                        .height_full()
                        .apply_if(current_tab.get() != Tab::Problems, |s| {
                            s.display(floem::style::Display::None)
                        })
                }),
                container(references_view(state.clone())).style(move |s| {
                    s.width_full()
                        .height_full()
                        .apply_if(current_tab.get() != Tab::References, |s| {
                            s.display(floem::style::Display::None)
                        })
                }),
                container(git_diff_view(state.clone())).style(move |s| {
                    s.width_full()
                        .height_full()
                        .apply_if(current_tab.get() != Tab::GitDiff, |s| {
                            s.display(floem::style::Display::None)
                        })
                }),
                container(output_view(state.clone())).style(move |s| {
                    s.width_full()
                        .height_full()
                        .apply_if(current_tab.get() != Tab::Output, |s| {
                            s.display(floem::style::Display::None)
                        })
                }),
                container(debug_console_view(state.clone())).style(move |s| {
                    s.width_full()
                        .height_full()
                        .apply_if(current_tab.get() != Tab::DebugConsole, |s| {
                            s.display(floem::style::Display::None)
                        })
                }),
                container(ports_view(state.clone())).style(move |s| {
                    s.width_full()
                        .height_full()
                        .apply_if(current_tab.get() != Tab::Ports, |s| {
                            s.display(floem::style::Display::None)
                        })
                }),
            ))
            .style(|s| s.flex_grow(1.0).width_full()),
        ))
        .style(|s| s.flex_col().width_full().height_full()),
    )
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.flex_col()
            .height(240.0)
            .width_full()
            .background(p.glass_bg)
            .border_top(1.0)
            .border_color(p.glass_border)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(-4.0)
            .box_shadow_blur(10.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
            .apply_if(!state.show_bottom_panel.get(), |s| {
                s.display(floem::style::Display::None)
            })
    })
}

fn completion_popup(state: IdeState) -> impl IntoView {
    let items = state.completions;
    let selected = state.completion_selected;
    let filter = state.completion_filter_text;

    // Filtered + enumerated list — index preserves original position so
    // Enter/Tab handler can look up the right entry by `selected`.
    let filtered_items = move || -> Vec<(usize, CompletionEntry)> {
        let f = filter.get().to_lowercase();
        items
            .get()
            .into_iter()
            .enumerate()
            .filter(|(_, e)| f.is_empty() || e.label.to_lowercase().starts_with(&f))
            .collect()
    };

    let list = scroll(
        dyn_stack(filtered_items, |(idx, _)| *idx, {
            let state = state.clone();
            move |(idx, entry): (usize, CompletionEntry)| {
                let is_sel = move || selected.get() == idx;
                let item_detail = entry.detail.clone().unwrap_or_default();
                let item_label = entry.label.clone();
                stack((
                    label(move || item_label.clone()).style(move |s: floem::style::Style| {
                        let p = state.theme.get().palette;
                        s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
                    }),
                    label(move || item_detail.clone()).style(move |s: floem::style::Style| {
                        let p = state.theme.get().palette;
                        s.font_size(11.0).color(p.text_muted).margin_left(8.0)
                    }),
                ))
                .style(move |s: floem::style::Style| {
                    let p = state.theme.get().palette;
                    s.items_center()
                        .width_full()
                        .padding_horiz(12.0)
                        .padding_vert(5.0)
                        .border_radius(4.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                        .background(if is_sel() {
                            p.accent_dim
                        } else {
                            floem::peniko::Color::TRANSPARENT
                        })
                })
                .on_click_stop({
                    let state = state.clone();
                    move |_| {
                        selected.set(idx);
                        state.completion_open.set(false);
                    }
                })
                .on_event_stop(EventListener::PointerEnter, move |_| selected.set(idx))
            }
        })
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| {
        s.width_full()
            .max_height(ui_const::COMPLETION_POPUP_MAX_HEIGHT)
    });

    let header = stack((
        label(|| "Completions").style(move |s| {
            s.font_size(11.0)
                .color(state.theme.get().palette.text_muted)
                .flex_grow(1.0)
        }),
        container(label(|| "Esc"))
            .style(move |s| {
                let p = state.theme.get().palette;
                s.font_size(10.0)
                    .color(p.text_muted)
                    .background(p.bg_elevated)
                    .padding_horiz(5.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| state.completion_open.set(false)),
    ))
    .style(move |s| {
        s.items_center()
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(state.theme.get().palette.border)
            .margin_bottom(4.0)
    });

    let empty_hint = label(move || {
        let f = filter.get().to_lowercase();
        let count = items
            .get()
            .into_iter()
            .filter(|e| f.is_empty() || e.label.to_lowercase().starts_with(&f))
            .count();
        if count == 0 {
            format!(
                "No completions{}",
                if f.is_empty() {
                    String::new()
                } else {
                    format!(" for \"{f}\"")
                }
            )
        } else {
            String::new()
        }
    })
    .style(move |s| {
        let f = filter.get().to_lowercase();
        let count = items
            .get()
            .into_iter()
            .filter(|e| f.is_empty() || e.label.to_lowercase().starts_with(&f))
            .count();
        s.font_size(12.0)
            .color(state.theme.get().palette.text_muted)
            .padding(12.0)
            .apply_if(count > 0, |s| s.display(floem::style::Display::None))
    });

    let popup_box = stack((header, empty_hint, list))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            s.flex_col()
                .width(ui_const::COMPLETION_POPUP_WIDTH)
                .background(p.bg_panel)
                .border(1.0)
                .border_color(p.glass_border)
                .border_radius(8.0)
                .box_shadow_h_offset(0.0)
                .box_shadow_v_offset(4.0)
                .box_shadow_blur(32.0)
                .box_shadow_color(p.glow)
                .box_shadow_spread(0.0)
        })
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                match &ke.key.logical_key {
                    Key::Named(floem::keyboard::NamedKey::Escape) => {
                        state.completion_open.set(false);
                    }
                    Key::Named(floem::keyboard::NamedKey::ArrowDown) => {
                        let f = filter.get().to_lowercase();
                        let max = items
                            .get()
                            .into_iter()
                            .filter(|e| f.is_empty() || e.label.to_lowercase().starts_with(&f))
                            .count()
                            .saturating_sub(1);
                        selected.update(|v| *v = (*v + 1).min(max));
                    }
                    Key::Named(floem::keyboard::NamedKey::ArrowUp) => {
                        selected.update(|v| *v = v.saturating_sub(1));
                    }
                    _ => {}
                }
            }
        });

    container(popup_box)
        .style(move |s| {
            let shown = state.completion_open.get();
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(120.0)
                .z_index(ui_const::Z_COMPLETIONS)
                .background(floem::peniko::Color::TRANSPARENT)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| state.completion_open.set(false))
}

// ── Ctrl+K inline AI-edit overlay ────────────────────────────────────────────

#[derive(Clone, Debug)]
enum InlineEditUpdate {
    Done(String),
    Err(String),
}

fn inline_edit_overlay(state: IdeState) -> impl IntoView {
    let open = state.inline_edit_open;
    let query = state.inline_edit_query;
    let ai_thinking = state.ai_thinking;

    // Channel created once at overlay-construction time (reactive scope).
    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<InlineEditUpdate>(64);
    let update_sig = create_signal_from_channel(update_rx);

    // React to incoming AI updates — runs on UI thread (safe to write signals).
    {
        let state2 = state.clone();
        create_effect(move |_| {
            let Some(upd) = update_sig.get() else { return };
            match upd {
                InlineEditUpdate::Done(text) => {
                    // Insert AI result at current cursor position via pending_completion.
                    state2.pending_completion.set(Some((text, 0)));
                    state2.ai_thinking.set(false);
                    state2.inline_edit_open.set(false);
                    state2.inline_edit_query.set(String::new());
                }
                InlineEditUpdate::Err(e) => {
                    eprintln!("[PhazeAI] Ctrl+K error: {e}");
                    state2.ai_thinking.set(false);
                    state2.inline_edit_open.set(false);
                    state2.inline_edit_query.set(String::new());
                }
            }
        });
    }

    let hint = label(|| "Describe the change (Enter to apply, Esc to cancel)").style(move |s| {
        s.font_size(11.0)
            .color(state.theme.get().palette.text_muted)
            .margin_bottom(6.0)
    });

    let input = text_input(query)
        .placeholder("e.g. \"add error handling\", \"convert to async\", \"add JSDoc\"")
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            s.width_full().padding_horiz(10.0).padding_vert(8.0)
             .font_size(14.0)
             .color(p.text_primary)
             .background(p.bg_elevated)
             .border(1.0)
             .border_color(p.border_focus)
             .border_radius(6.0)
        })
        .on_event_stop(EventListener::KeyDown, move |ev| {
            if let Event::KeyDown(e) = ev {
                match &e.key.logical_key {
                    Key::Named(floem::keyboard::NamedKey::Escape) => {
                        open.set(false);
                        query.set(String::new());
                    }
                    Key::Named(floem::keyboard::NamedKey::Enter) => {
                        let instruction = query.get();
                        if instruction.is_empty() { return; }
                        ai_thinking.set(true);
                        // Build context from open file (first 4 KB to stay within budget)
                        let file_ctx = state.open_file.get()
                            .and_then(|p| std::fs::read_to_string(&p).ok())
                            .unwrap_or_default();
                        let end = file_ctx.floor_char_boundary(4096);
                        let file_ctx = if file_ctx.len() > 4096 { &file_ctx[..end] } else { &file_ctx };
                        let prompt = format!(
                            "Apply the following edit to the code. \
                             Respond with ONLY the generated code fragment, no explanation, no markdown fences.\n\n\
                             Instruction: {instruction}\n\nCode context:\n{file_ctx}"
                        );
                        let settings = Settings::load();
                        let tx = update_tx.clone(); // SyncSender: Clone + Send
                        std::thread::spawn(move || {
                            let rt = match tokio::runtime::Builder::new_current_thread()
                                .enable_all().build()
                            {
                                Ok(rt) => rt,
                                Err(e) => { let _ = tx.send(InlineEditUpdate::Err(format!("Runtime: {e}"))); return; }
                            };
                            rt.block_on(async move {
                                let client = match settings.build_llm_client() {
                                    Ok(c) => c,
                                    Err(e) => { let _ = tx.send(InlineEditUpdate::Err(format!("LLM: {e}"))); return; }
                                };
                                let agent = Agent::new(client);
                                let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
                                let run_fut = agent.run_with_events(&prompt, agent_tx);
                                let drain_fut = async {
                                    let mut accumulated = String::new();
                                    while let Some(event) = agent_rx.recv().await {
                                        match event {
                                            AgentEvent::TextDelta(text) => { accumulated.push_str(&text); }
                                            AgentEvent::Complete { .. } => {
                                                let _ = tx.send(InlineEditUpdate::Done(accumulated.clone()));
                                                break;
                                            }
                                            AgentEvent::Error(e) => {
                                                let _ = tx.send(InlineEditUpdate::Err(e));
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                };
                                let _ = tokio::join!(run_fut, drain_fut);
                            });
                        });
                    }
                    _ => {}
                }
            }
        });

    let badge = label(|| "✦ AI Edit").style(move |s| {
        let p = state.theme.get().palette;
        s.font_size(11.0)
            .color(p.accent)
            .font_weight(floem::text::Weight::BOLD)
            .margin_bottom(8.0)
    });

    let box_view = stack((badge, hint, input)).style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.flex_col()
            .padding(20.0)
            .width(520.0)
            .background(p.bg_panel)
            .border(1.0)
            .border_color(p.glass_border)
            .border_radius(10.0)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(6.0)
            .box_shadow_blur(40.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    });

    container(box_view)
        .style(move |s| {
            let shown = open.get();
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(200.0)
                .z_index(ui_const::Z_INLINE_EDIT)
                .background(floem::peniko::Color::from_rgba8(0, 0, 0, 160))
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| {
            open.set(false);
            query.set(String::new());
        })
}

// ── LSP Hover tooltip overlay ─────────────────────────────────────────────────

fn hover_tooltip(state: IdeState) -> impl IntoView {
    let hover_text = state.hover_text;
    let theme = state.theme;

    // Wrap the text in a styled container that looks like a floating doc box.
    let tooltip_box = container(label(move || hover_text.get().unwrap_or_default()).style(
        move |s| {
            let p = theme.get().palette;
            s.font_size(12.0).color(p.text_primary).max_width(480.0)
        },
    ))
    .style(move |s| {
        let shown = hover_text.get().is_some();
        let p = theme.get().palette;
        s.padding_horiz(12.0)
            .padding_vert(8.0)
            .background(p.bg_elevated)
            .border(1.0)
            .border_color(p.border)
            .border_radius(6.0)
            .max_width(500.0)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(4.0)
            .box_shadow_blur(16.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
            .apply_if(!shown, |s| s.display(floem::style::Display::None))
    })
    .on_click_stop(move |_| {
        hover_text.set(None);
    });

    // Anchor bottom-right of the editor area — absolute positioned so it floats
    // above everything. The user dismisses it by clicking anywhere on it.
    container(tooltip_box).style(move |s| {
        let shown = hover_text.get().is_some();
        s.absolute()
            .inset_bottom(60.0)
            .inset_left(320.0)
            .z_index(ui_const::Z_HOVER_TIP)
            .apply_if(!shown, |s| s.display(floem::style::Display::None))
    })
}

// ── Code Actions dropdown overlay (Ctrl+.) ───────────────────────────────────

fn code_actions_overlay(state: IdeState) -> impl IntoView {
    let open = state.code_actions_open;
    let actions = state.code_actions;
    let theme = state.theme;
    let hovered: RwSignal<Option<usize>> = create_rw_signal(None);

    let header = stack((
        label(|| "Quick Fix").style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .color(p.accent)
                .font_weight(floem::text::Weight::SEMIBOLD)
                .flex_grow(1.0)
        }),
        container(label(|| "Esc"))
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(10.0)
                    .color(p.text_muted)
                    .background(p.bg_elevated)
                    .padding_horiz(5.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| open.set(false)),
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.items_center()
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .border_bottom(1.0)
            .border_color(p.border)
            .margin_bottom(4.0)
    });

    let empty_hint = label(move || {
        if actions.get().is_empty() {
            "No actions available at this position.".to_string()
        } else {
            String::new()
        }
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(12.0)
            .color(p.text_muted)
            .padding(12.0)
            .apply_if(!actions.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let list = scroll(
        dyn_stack(
            move || {
                safe_get(actions, Vec::new())
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(idx, _)| *idx,
            {
                let state2 = state.clone();
                move |(idx, action): (usize, CodeAction)| {
                    let title = action.title.clone();
                    let kind = action.kind.clone();
                    let edits = action.edit.clone();
                    let hov = hovered;
                    let state3 = state2.clone();

                    container(
                        stack((
                            label(|| "▶ ").style(move |s| {
                                let p = state3.theme.get().palette;
                                s.font_size(10.0).color(p.accent).margin_right(4.0)
                            }),
                            label(move || title.clone()).style(move |s: floem::style::Style| {
                                let p = state3.theme.get().palette;
                                s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
                            }),
                        ))
                        .style(|s| s.flex_row().items_center().width_full()),
                    )
                    .style({
                        let state4 = state2.clone();
                        move |s| {
                            let p = state4.theme.get().palette;
                            s.width_full()
                                .padding_horiz(12.0)
                                .padding_vert(8.0)
                                .border_radius(4.0)
                                .cursor(floem::style::CursorStyle::Pointer)
                                .background(if hov.get() == Some(idx) {
                                    p.bg_elevated
                                } else {
                                    floem::peniko::Color::TRANSPARENT
                                })
                        }
                    })
                    .on_click_stop({
                        let state5 = state2.clone();
                        let kind2 = kind.clone();
                        let edits2 = edits.clone();
                        move |_| {
                            state5.code_actions_open.set(false);
                            if kind2 == "source.formatDocument" {
                                // Trigger format via comment_toggle_nonce repurposed, or a dedicated nonce.
                                // For now, open the file refresh by toggling a state:
                                // The formatter (save-on-format) runs when a file is re-opened.
                                if let Some(path) = state5.open_file.get() {
                                    if let Ok(text) = std::fs::read_to_string(&path) {
                                        let _ = state5.lsp_cmd.send(LspCommand::OpenFile {
                                            path: path.clone(),
                                            text,
                                        });
                                    }
                                }
                            } else if kind2 == "refactor.findReferences" {
                                // Switch to References tab in bottom panel
                                state5.show_bottom_panel.set(true);
                                state5.bottom_panel_tab.set(Tab::References);
                            } else if let Some(file_edits) = edits2.as_ref() {
                                // Apply workspace edits (e.g. organize imports)
                                for (fpath, new_content) in file_edits {
                                    let _ = std::fs::write(fpath, new_content);
                                    // Re-open in editor to reflect changes
                                    if state5.open_file.get().as_ref() == Some(fpath) {
                                        state5.open_file.set(Some(fpath.clone()));
                                    }
                                }
                            }
                        }
                    })
                    .on_event_stop(EventListener::PointerEnter, move |_| {
                        hov.set(Some(idx));
                    })
                    .on_event_stop(EventListener::PointerLeave, move |_| {
                        hov.set(None);
                    })
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(320.0));

    let dropdown_box = stack((header, empty_hint, list))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            s.flex_col()
                .width(380.0)
                .background(p.bg_panel)
                .border(1.0)
                .border_color(p.glass_border)
                .border_radius(8.0)
                .box_shadow_h_offset(0.0)
                .box_shadow_v_offset(4.0)
                .box_shadow_blur(32.0)
                .box_shadow_color(p.glow)
                .box_shadow_spread(0.0)
        })
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                if ke.key.logical_key == Key::Named(floem::keyboard::NamedKey::Escape) {
                    open.set(false);
                }
            }
        });

    container(dropdown_box)
        .style(move |s| {
            let shown = state.code_actions_open.get();
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(150.0)
                .z_index(ui_const::Z_CODE_ACTIONS)
                .background(floem::peniko::Color::from_rgba8(0, 0, 0, 100))
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| state.code_actions_open.set(false))
}

/// Rename-symbol overlay (F2): a small text-input dialog centered on screen.
fn rename_overlay(state: IdeState) -> impl IntoView {
    use floem::reactive::{SignalGet, SignalUpdate};
    use floem::views::{container, label, stack, text_input, Decorators};

    let open = state.rename_open;
    let query = state.rename_query;
    let target = state.rename_target;
    let lsp_cmd = state.lsp_cmd.clone();
    let cursor = state.active_cursor;
    let ws = state.workspace_root.get_untracked();

    let input = text_input(query).style(|s| s.width(320.0).padding(8.0).font_size(14.0));

    let confirm = {
        let lsp_cmd2 = lsp_cmd.clone();
        label(|| "Rename".to_string())
            .style(|s| {
                s.padding_horiz(16.0)
                    .padding_vert(6.0)
                    .background(floem::peniko::Color::from_rgb8(80, 160, 255))
                    .color(floem::peniko::Color::WHITE)
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| {
                let new_name = query.get_untracked();
                let _old = target.get_untracked();
                if let Some((path, line, col)) = cursor.get_untracked() {
                    let _ = lsp_cmd2.send(LspCommand::RequestRename {
                        path,
                        line,
                        col,
                        new_name,
                        workspace_root: ws.clone(),
                    });
                }
                open.set(false);
            })
    };

    let cancel = label(|| "Cancel".to_string())
        .style(|s| {
            s.padding_horiz(16.0)
                .padding_vert(6.0)
                .background(floem::peniko::Color::from_rgba8(255, 255, 255, 30))
                .border_radius(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| {
            open.set(false);
        });

    let title = label(move || format!("Rename '{}'", target.get())).style(|s| {
        s.font_size(13.0)
            .color(floem::peniko::Color::from_rgb8(180, 200, 230))
            .margin_bottom(8.0)
    });

    let dialog = container(
        stack((
            title,
            input,
            stack((confirm, cancel))
                .style(|s| s.flex_row().gap(8.0).margin_top(10.0).justify_end()),
        ))
        .style(|s| s.flex_col().gap(4.0)),
    )
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.padding(20.0)
            .border_radius(10.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .min_width(360.0)
    });

    container(dialog)
        .style(move |s| {
            let shown = open.get();
            s.absolute()
                .inset(0)
                .items_center()
                .justify_center()
                .z_index(ui_const::Z_RENAME)
                .background(floem::peniko::Color::from_rgba8(0, 0, 0, 130))
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

/// Signature-help tooltip (Ctrl+Shift+Space): shows function signature at bottom of editor area.
fn sig_help_overlay(state: IdeState) -> impl IntoView {
    use floem::reactive::SignalGet;
    use floem::views::{container, label, Decorators};

    let sig_help = state.sig_help;

    let content = container(
        label(move || {
            if let Some(sh) = sig_help.get() {
                let mut text = sh.label.clone();
                if !sh.params.is_empty() {
                    let params_joined = sh.params.join(", ");
                    // Highlight active param inline
                    if let Some(param) = sh.params.get(sh.active_param) {
                        text = format!("{text}\nActive: {param}  |  {params_joined}");
                    } else {
                        text = format!("{text}\n{params_joined}");
                    }
                }
                text
            } else {
                String::new()
            }
        })
        .style(|s| {
            s.font_size(12.0)
                .color(floem::peniko::Color::from_rgb8(200, 220, 255))
        }),
    )
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.padding(10.0)
            .border_radius(6.0)
            .background(p.bg_panel)
            .border(1.0)
            .border_color(p.glass_border)
            .max_width(600.0)
    });

    container(content)
        .style(move |s| {
            let shown = sig_help.get().is_some();
            s.absolute()
                .inset(0)
                .items_end()
                .justify_center()
                .padding_bottom(80.0)
                .z_index(ui_const::Z_SIG_HELP)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| sig_help.set(None))
}

// ── Toast notification overlay ────────────────────────────────────────────────

fn toast_overlay(state: IdeState) -> impl IntoView {
    let toast = state.status_toast;
    let theme = state.theme;

    container(
        label(move || toast.get().unwrap_or_default()).style(move |s| {
            let p = theme.get().palette;
            s.font_size(13.0).color(p.text_primary)
        }),
    )
    .style(move |s| {
        let shown = toast.get().is_some();
        let p = theme.get().palette;
        s.absolute()
            .inset_bottom(40.0)
            .inset_left(320.0)
            .z_index(ui_const::Z_TOAST)
            .padding_horiz(18.0)
            .padding_vert(10.0)
            .background(p.bg_elevated)
            .border_radius(8.0)
            .border(1.0)
            .border_color(p.border)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(4.0)
            .box_shadow_blur(20.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
            .apply_if(!shown, |s| s.display(floem::style::Display::None))
    })
}

// ── Workspace Symbols overlay (Ctrl+T) ───────────────────────────────────────
// Shows a fuzzy-searchable list of symbols across the whole workspace,
// provided by the LSP workspace/symbol request or a ripgrep fallback.

fn workspace_symbols_overlay(state: IdeState) -> impl IntoView {
    use floem::reactive::{SignalGet, SignalUpdate};
    use floem::views::{container, dyn_stack, empty, label, scroll, text_input, Decorators};

    let open = state.ws_syms_open;
    let query = state.ws_syms_query;
    let symbols = state.workspace_symbols;
    let theme = state.theme;
    let lsp_cmd = state.lsp_cmd.clone();
    let goto_line = state.goto_line;

    // Derived: filter symbols by query (client-side for fast response)
    let filtered = move || {
        let q = query.get().to_lowercase();
        let syms = symbols.get();
        if q.is_empty() {
            syms.into_iter().take(50).collect::<Vec<_>>()
        } else {
            syms.into_iter()
                .filter(|s| s.name.to_lowercase().contains(&q))
                .take(50)
                .collect::<Vec<_>>()
        }
    };

    let rows = scroll(
        dyn_stack(
            filtered,
            |s| format!("{}:{}", s.name, s.line),
            move |sym| {
                let name = sym.name.clone();
                let kind = sym.kind.clone();
                let line = sym.line;
                let row_theme = theme;
                container(
                    stack((
                        label(move || kind.clone()).style(move |s| {
                            let p = row_theme.get().palette;
                            s.font_size(10.0).color(p.accent).width(44.0)
                        }),
                        label(move || name.clone()).style(move |s| {
                            let p = row_theme.get().palette;
                            s.font_size(13.0).color(p.text_primary)
                        }),
                        label(move || format!("  :{line}")).style(move |s| {
                            let p = row_theme.get().palette;
                            s.font_size(11.0).color(p.text_muted)
                        }),
                    ))
                    .style(|s| s.items_center().padding_vert(2.0)),
                )
                .style(move |s| {
                    let p = row_theme.get().palette;
                    s.padding_horiz(12.0)
                        .padding_vert(4.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.background(p.bg_elevated))
                })
                .on_click_stop(move |_| {
                    open.set(false);
                    // Jump to the symbol's file and line.
                    // For simplicity use active file; a full implementation would parse the path.
                    goto_line.set(line);
                })
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.max_height(320.0).width_full());

    let search_box = text_input(query)
        .style(move |s| {
            let p = theme.get().palette;
            s.width_full()
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_base)
                .border(0.0)
                .padding_horiz(12.0)
                .padding_vert(8.0)
        })
        .on_event_stop(floem::event::EventListener::KeyDown, move |e| {
            use floem::keyboard::{Key, NamedKey};
            if let floem::event::Event::KeyDown(ke) = e {
                match ke.key.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        open.set(false);
                    }
                    Key::Named(NamedKey::Enter) => {
                        open.set(false);
                    }
                    _ => {}
                }
            }
        });

    // When query changes, send a workspace symbol request.
    {
        let lsp_tx = lsp_cmd.clone();
        create_effect(move |_| {
            let q = query.get();
            if open.get_untracked() {
                let _ = lsp_tx.send(LspCommand::RequestWorkspaceSymbols { query: q });
            }
        });
    }

    let dialog = stack((
        stack((label(|| "Workspace Symbols").style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .color(p.text_muted)
                .padding_horiz(12.0)
                .padding_vert(6.0)
        }),))
        .style(|s| s.width_full()),
        search_box,
        container(empty()).style(move |s| {
            s.height(1.0)
                .width_full()
                .background(theme.get().palette.border)
        }),
        rows,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width(520.0)
            .max_height(400.0)
            .border_radius(10.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(8.0)
            .box_shadow_blur(32.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    });

    container(dialog)
        .style(move |s| {
            let shown = open.get();
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(80.0)
                .z_index(ui_const::Z_WS_SYMBOLS)
                .background(floem::peniko::Color::from_rgba8(0, 0, 0, 140))
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

// ── Branch picker overlay (click branch in status bar) ───────────────────────

fn branch_picker_overlay(state: IdeState) -> impl IntoView {
    let open = state.branch_picker_open;
    let branches = state.branch_list;
    let current = state.git_branch;
    let theme = state.theme;
    let workspace = state.workspace_root;
    let toast = state.status_toast;

    // Channel for checkout results — created once, shared across all dyn_stack rows.
    let (checkout_tx, checkout_rx) = std::sync::mpsc::sync_channel::<Result<String, String>>(1);
    let checkout_sig = floem::ext_event::create_signal_from_channel(checkout_rx);
    {
        let tst = toast;
        let cur = current;
        create_effect(move |_| {
            if let Some(result) = checkout_sig.get() {
                match result {
                    Ok(new_branch) => {
                        cur.set(new_branch.clone());
                        show_toast(tst, format!("Switched to {new_branch}"));
                    }
                    Err(e) => {
                        show_toast(tst, format!("Checkout failed: {e}"));
                    }
                }
            }
        });
    }

    let rows = scroll(
        dyn_stack(
            move || safe_get(branches, Vec::new()),
            |b| b.clone(),
            move |branch| {
                let b_label = branch.clone();
                let b_current = branch.clone();
                let b_click = branch.clone();
                let is_current = move || current.get() == b_current;
                let hov = create_rw_signal(false);
                let ws = workspace;
                // Clone sender once per row so the `Fn` closure can call it repeatedly.
                let row_tx = checkout_tx.clone();

                container(
                    stack((
                        label(move || if is_current() { "✓ " } else { "  " }.to_string()).style(
                            move |s| {
                                s.font_size(12.0)
                                    .color(theme.get().palette.success)
                                    .width(20.0)
                            },
                        ),
                        label(move || b_label.clone()).style(move |s| {
                            s.font_size(13.0).color(theme.get().palette.text_primary)
                        }),
                    ))
                    .style(|s| s.items_center()),
                )
                .style(move |s| {
                    let p = theme.get().palette;
                    s.padding_horiz(12.0)
                        .padding_vert(6.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                        .background(if hov.get() {
                            p.bg_elevated
                        } else {
                            floem::peniko::Color::TRANSPARENT
                        })
                })
                .on_click_stop(move |_| {
                    let branch_name = b_click.clone();
                    open.set(false);
                    let root = ws.get();
                    let tx = row_tx.clone();
                    std::thread::spawn(move || {
                        let out = std::process::Command::new("git")
                            .args(["checkout", &branch_name])
                            .current_dir(&root)
                            .output();
                        let result = match out {
                            Ok(o) if o.status.success() => Ok(branch_name),
                            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                            Err(e) => Err(e.to_string()),
                        };
                        let _ = tx.send(result);
                    });
                })
                .on_event_stop(EventListener::PointerEnter, move |_| hov.set(true))
                .on_event_stop(EventListener::PointerLeave, move |_| hov.set(false))
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.max_height(320.0).width_full());

    let dialog = stack((
        label(|| "Switch Branch").style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .color(p.text_muted)
                .padding_horiz(12.0)
                .padding_vert(8.0)
                .font_weight(floem::text::Weight::BOLD)
        }),
        container(empty()).style(move |s| {
            s.height(1.0)
                .width_full()
                .background(theme.get().palette.border)
        }),
        rows,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width(320.0)
            .max_height(400.0)
            .border_radius(10.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(8.0)
            .box_shadow_blur(32.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    });

    container(dialog)
        .style(move |s| {
            let shown = open.get();
            s.absolute()
                .inset(0)
                .items_end()
                .justify_start()
                .padding_bottom(30.0)
                .padding_left(8.0)
                .z_index(ui_const::Z_BRANCH_PICKER)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

// ── Vim ex command bar (:w, :q, :wq, :wqa, :e <file>, etc.) ─────────────────
fn vim_ex_overlay(state: IdeState) -> impl IntoView {
    let open = state.vim_ex_open;
    let input_sig = state.vim_ex_input;
    let theme = state.theme;
    let open_file = state.open_file;
    let toast = state.status_toast;
    let workspace = state.workspace_root;

    let input_view = text_input(input_sig)
        .style(move |s| {
            let p = theme.get().palette;
            s.width_full()
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_panel)
                .border(0.0)
                .padding_horiz(4.0)
        })
        .on_event_stop(EventListener::KeyDown, move |e| {
            use floem::keyboard::{Key, NamedKey};
            if let Event::KeyDown(ke) = e {
                match &ke.key.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        open.set(false);
                        input_sig.set(String::new());
                    }
                    Key::Named(NamedKey::Enter) => {
                        let cmd = input_sig.get_untracked();
                        let cmd = cmd.trim().to_string();
                        open.set(false);
                        input_sig.set(String::new());
                        // Handle ex commands
                        match cmd.as_str() {
                            "w" | "write" => {
                                // Trigger save via auto_save signal or toast
                                show_toast(toast, "Saved".to_string());
                            }
                            "q" | "quit" => {
                                std::process::exit(0);
                            }
                            "wq" | "x" => {
                                show_toast(toast, "Saved".to_string());
                                std::process::exit(0);
                            }
                            "wqa" | "qa" => {
                                std::process::exit(0);
                            }
                            _ if cmd.starts_with("e ") => {
                                let path = cmd[2..].trim();
                                let full = workspace.get_untracked().join(path);
                                if full.exists() {
                                    open_file.set(Some(full));
                                } else {
                                    show_toast(toast, format!("No such file: {path}"));
                                }
                            }
                            _ if cmd.starts_with("cd ") => {
                                let dir = cmd[3..].trim();
                                let _ = std::env::set_current_dir(dir);
                            }
                            _ => {
                                if !cmd.is_empty() {
                                    show_toast(toast, format!("Unknown command: {cmd}"));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

    let bar = stack((
        label(|| ":").style(move |s| {
            s.font_size(14.0)
                .color(theme.get().palette.accent)
                .margin_right(4.0)
        }),
        input_view,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .width_full()
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .background(p.bg_panel)
            .border_top(1.5)
            .border_color(p.glass_border)
    });

    container(bar).style(move |s| {
        s.absolute()
            .inset_bottom(32.0) // just above status bar
            .inset_left(0)
            .inset_right(0)
            .z_index(ui_const::Z_VIM_EX)
            .apply_if(!open.get(), |s| s.display(floem::style::Display::None))
    })
}

// ── Goto line/col overlay (Ctrl+G) ────────────────────────────────────────────
fn goto_overlay(state: IdeState) -> impl IntoView {
    let open = state.goto_overlay_open;
    let input_sig = state.goto_overlay_input;
    let goto_line = state.goto_line;
    let theme = state.theme;
    let toast = state.status_toast;

    let input_view = text_input(input_sig)
        .placeholder("Line or line:col")
        .style(move |s| {
            let p = theme.get().palette;
            s.width(220.0)
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_panel)
                .border(0.0)
                .padding_horiz(4.0)
        })
        .on_event_stop(EventListener::KeyDown, move |e| {
            use floem::keyboard::{Key, NamedKey};
            if let Event::KeyDown(ke) = e {
                match &ke.key.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        open.set(false);
                        input_sig.set(String::new());
                    }
                    Key::Named(NamedKey::Enter) => {
                        let text = input_sig.get_untracked();
                        open.set(false);
                        input_sig.set(String::new());
                        // Parse "line" or "line:col"
                        let parts: Vec<&str> = text.trim().splitn(2, ':').collect();
                        if let Ok(line) = parts[0].parse::<u32>() {
                            goto_line.set(line.saturating_sub(1));
                        } else {
                            show_toast(toast, format!("Invalid: {text}"));
                        }
                    }
                    _ => {}
                }
            }
        });

    let dialog = stack((
        label(|| "Go to Line  ").style(move |s| {
            s.font_size(11.0)
                .color(theme.get().palette.text_muted)
                .font_weight(floem::text::Weight::BOLD)
        }),
        input_view,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .padding_horiz(16.0)
            .padding_vert(10.0)
            .border_radius(8.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(8.0)
            .box_shadow_blur(24.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    });

    container(dialog)
        .style(move |s| {
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(80.0)
                .z_index(ui_const::Z_GOTO)
                .apply_if(!open.get(), |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

// ── Peek Definition overlay (Alt+F12) ────────────────────────────────────────

fn peek_def_overlay(state: IdeState) -> impl IntoView {
    let open = state.peek_def_open;
    let lines = state.peek_def_lines;
    let theme = state.theme;

    let close_btn = container(label(|| "  Close  "))
        .style(move |s| {
            let p = theme.get().palette;
            s.padding_horiz(10.0)
                .padding_vert(4.0)
                .border_radius(4.0)
                .background(p.bg_elevated)
                .color(p.text_secondary)
                .font_size(11.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .border(1.0)
                .border_color(p.glass_border)
        })
        .on_click_stop(move |_| {
            open.set(false);
            lines.set(vec![]);
        });

    let header = stack((
        label(|| "Peek Definition").style(move |s| {
            s.font_size(12.0)
                .color(theme.get().palette.text_primary)
                .flex_grow(1.0)
        }),
        close_btn,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(p.glass_border)
    });

    let content = scroll(
        dyn_stack(
            move || {
                safe_get(lines, Vec::new())
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(i, _)| *i,
            move |(_, line_text)| {
                let is_highlight = line_text.starts_with('>');
                let text_val = line_text.clone();
                container(label(move || text_val.clone())).style(move |s| {
                    let p = theme.get().palette;
                    s.font_family("monospace".to_string())
                        .font_size(12.0)
                        .color(if is_highlight {
                            p.text_primary
                        } else {
                            p.text_secondary
                        })
                        .background(if is_highlight {
                            p.accent_dim
                        } else {
                            floem::peniko::Color::TRANSPARENT
                        })
                        .padding_horiz(12.0)
                        .padding_vert(1.0)
                        .width_full()
                })
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.max_height(280.0).width_full());

    let popup = stack((header, content)).style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width(560.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .border_radius(8.0)
    });

    container(popup)
        .style(move |s| {
            let shown = open.get() && !lines.get().is_empty();
            s.absolute()
                .inset(0)
                .items_center()
                .justify_center()
                .z_index(ui_const::Z_PEEK_DEF)
                .background(floem::peniko::Color::from_rgba8(0, 0, 0, 120))
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

fn ide_root(state: IdeState) -> impl IntoView {
    let raw_editor = editor_panel(
        state.open_file,
        state.theme,
        state.ai_thinking,
        state.lsp_cmd.clone(),
        state.active_cursor,
        state.pending_completion,
        state.diagnostics,
        state.goto_line,
        state.comment_toggle_nonce,
        state.initial_tabs.clone(),
        state.open_tabs,
        state.vim_motion,
        state.ghost_text,
        state.auto_save,
        state.workspace_root.get_untracked(),
        state.font_size,
        state.word_wrap,
        state.ctrl_d_nonce,
        state.fold_nonce,
        state.unfold_nonce,
        state.move_line_up_nonce,
        state.move_line_down_nonce,
        state.duplicate_line_nonce,
        state.delete_line_nonce,
        state.active_blame,
        state.col_cursor_up_nonce,
        state.col_cursor_down_nonce,
        state.sticky_lines,
        state.transform_upper_nonce,
        state.transform_lower_nonce,
        state.join_line_nonce,
        state.sort_lines_nonce,
        state.vim_visual_mode,
        state.vim_marks,
        state.vim_last_motion,
        state.expand_selection_nonce,
        state.shrink_selection_nonce,
        state.relative_line_numbers,
        state.yank_ring,
        state.tab_size,
        state.line_ending,
        state.folding_ranges,
        state.transform_title_nonce,
        state.format_selection_nonce,
        state.save_no_format_nonce,
        state.fold_all_nonce,
        state.unfold_all_nonce,
        state.code_lens,
        state.code_lens_visible,
        state.organize_imports_on_save,
        state.inlay_hints_sig,
        state.inlay_hints_toggle,
    );

    // ── Split editor (Ctrl+Alt+\) — second independent editor pane ──────────
    let split_raw = editor_panel(
        state.split_open_file,
        state.theme,
        state.ai_thinking,
        state.lsp_cmd.clone(),
        state.split_active_cursor,
        state.pending_completion,
        state.diagnostics,
        create_rw_signal(0u32), // independent goto_line for split pane
        create_rw_signal(0u64), // independent comment nonce
        vec![],                 // no session restore for split pane
        state.split_open_tabs,
        state.vim_motion,
        state.ghost_text,
        state.auto_save,
        state.workspace_root.get_untracked(),
        state.font_size,
        state.word_wrap,
        create_rw_signal(0u64),          // ctrl_d
        create_rw_signal(0u64),          // fold
        create_rw_signal(0u64),          // unfold
        create_rw_signal(0u64),          // move_up
        create_rw_signal(0u64),          // move_down
        create_rw_signal(0u64),          // duplicate
        create_rw_signal(0u64),          // delete_line
        create_rw_signal(String::new()), // blame
        create_rw_signal(0u64),          // col_cursor_up
        create_rw_signal(0u64),          // col_cursor_down
        create_rw_signal(Vec::new()),    // sticky_lines
        create_rw_signal(0u64),          // transform_upper
        create_rw_signal(0u64),          // transform_lower
        create_rw_signal(0u64),          // join_line
        create_rw_signal(0u64),          // sort_lines
        state.vim_visual_mode,
        state.vim_marks,
        state.vim_last_motion,
        create_rw_signal(0u64),                     // expand_selection
        create_rw_signal(0u64),                     // shrink_selection
        create_rw_signal(false),                    // relative_line_numbers
        create_rw_signal(Vec::<String>::new()),     // yank_ring
        state.tab_size,                             // tab_size
        state.line_ending,                          // line_ending_out
        create_rw_signal(Vec::<(u32, u32)>::new()), // lsp_folding_ranges (split pane)
        create_rw_signal(0u64),                     // transform_title_nonce
        create_rw_signal(0u64),                     // format_selection_nonce
        create_rw_signal(0u64),                     // save_no_format_nonce
        create_rw_signal(0u64),                     // fold_all_nonce
        create_rw_signal(0u64),                     // unfold_all_nonce
        create_rw_signal(vec![]),                   // code_lens_sig
        create_rw_signal(true),                     // code_lens_visible
        create_rw_signal(false),                    // organize_imports_on_save
        create_rw_signal(vec![]),                   // inlay_hints_sig
        create_rw_signal(false),                    // inlay_hints_toggle
    );
    let split_pane = container(split_raw).style(move |s| {
        s.flex_grow(1.0)
            .min_width(0.0)
            .min_height(0.0)
            .apply_if(!state.split_editor.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });
    let split_divider = container(floem::views::empty()).style(move |s| {
        let t = state.theme.get();
        s.width(3.0)
            .height_full()
            .background(t.palette.glass_border)
            .apply_if(!state.split_editor.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    // ── Editor right-click context menu ──────────────────────────────────────
    let editor = {
        let s = state.clone();
        container(raw_editor)
            .style(|s| s.size_full().min_width(0.0))
            .on_event_cont(EventListener::PointerDown, move |event| {
                if let Event::PointerDown(pe) = event {
                    if pe.button.is_secondary() {
                        let s2 = s.clone();
                        let s3 = s.clone();
                        let s4 = s.clone();
                        let s5 = s.clone();
                        let s6 = s.clone();
                        let s7 = s.clone();
                        let menu = Menu::new("")
                            .entry(MenuItem::new("Copy").action(move || {
                                // Trigger system copy (editor handles it internally on Ctrl+C)
                                // Best effort: nothing to do here without editor handle
                            }))
                            .entry(MenuItem::new("Paste").action(move || {
                                // Paste from clipboard into editor
                                if let Ok(mut cb) = arboard::Clipboard::new() {
                                    if let Ok(text) = cb.get_text() {
                                        s2.pending_completion.set(Some((text, 0)));
                                    }
                                }
                            }))
                            .separator()
                            .entry(MenuItem::new("Go to Definition\tF12").action(move || {
                                if let Some((path, line, col)) = s3.active_cursor.get() {
                                    let _ = s3.lsp_cmd.send(LspCommand::RequestDefinition {
                                        path,
                                        line,
                                        col,
                                    });
                                }
                            }))
                            .entry(MenuItem::new("Find All References\tShift+F12").action(
                                move || {
                                    if let Some((path, line, col)) = s4.active_cursor.get() {
                                        let _ = s4.lsp_cmd.send(LspCommand::RequestReferences {
                                            path,
                                            line,
                                            col,
                                        });
                                        s4.show_bottom_panel.set(true);
                                        s4.bottom_panel_tab.set(Tab::References);
                                    }
                                },
                            ))
                            .entry(MenuItem::new("Rename Symbol\tF2").action(move || {
                                s5.rename_open.set(true);
                            }))
                            .entry(MenuItem::new("Code Actions\tCtrl+.").action(move || {
                                if let Some((path, line, col)) = s6.active_cursor.get() {
                                    let _ = s6.lsp_cmd.send(LspCommand::RequestCodeActions {
                                        path,
                                        line,
                                        col,
                                    });
                                }
                            }))
                            .separator()
                            .entry(MenuItem::new("Toggle Comment\tCtrl+/").action(move || {
                                s7.comment_toggle_nonce.update(|v| *v += 1);
                            }));
                        // AI-powered context menu items
                        let s_explain = s.clone();
                        let s_tests = s.clone();
                        let s_fix = s.clone();
                        let s_run = s.clone();
                        let s_run_file = s.clone();
                        let menu = menu
                            .separator()
                            .entry(MenuItem::new("🤖 Explain Selection").action(move || {
                                if let Some((ref path, line, _)) = s_explain.active_cursor.get() {
                                    let fname = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_else(|| "file".to_string());
                                    s_explain.pending_chat_inject.set(Some(format!(
                                        "Explain the code around line {} in {}",
                                        line + 1,
                                        fname
                                    )));
                                    s_explain.show_right_panel.set(true);
                                }
                            }))
                            .entry(MenuItem::new("🧪 Generate Tests").action(move || {
                                if let Some((ref path, line, _)) = s_tests.active_cursor.get() {
                                    let fname = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_else(|| "file".to_string());
                                    s_tests.pending_chat_inject.set(Some(format!(
                                        "Generate unit tests for the function at line {} in {}",
                                        line + 1,
                                        fname
                                    )));
                                    s_tests.show_right_panel.set(true);
                                }
                            }))
                            .entry(MenuItem::new("🔧 Fix with AI").action(move || {
                                if let Some((ref path, line, _)) = s_fix.active_cursor.get() {
                                    let diags = s_fix.diagnostics.get();
                                    let cur_diag = diags
                                        .iter()
                                        .find(|d| d.path == *path && d.line == (line + 1));
                                    if let Some(d) = cur_diag {
                                        s_fix
                                            .pending_chat_inject
                                            .set(Some(format!("Fix this error: {}", d.message)));
                                        s_fix.show_right_panel.set(true);
                                    } else {
                                        show_toast(
                                            s_fix.status_toast,
                                            "No diagnostic on this line",
                                        );
                                    }
                                }
                            }));
                        // Run in Terminal / Run File entries
                        let menu = menu
                            .separator()
                            .entry(MenuItem::new("Run in Terminal").action(move || {
                                // Send selected text (from clipboard) to the active terminal.
                                // If nothing is in the clipboard, send a placeholder.
                                let text = if let Ok(mut cb) = arboard::Clipboard::new() {
                                    cb.get_text().unwrap_or_default()
                                } else {
                                    String::new()
                                };
                                if !text.trim().is_empty() {
                                    s_run
                                        .run_in_terminal_text
                                        .set(Some(text.trim().to_string()));
                                    s_run.show_bottom_panel.set(true);
                                    s_run.bottom_panel_tab.set(Tab::Terminal);
                                }
                            }))
                            .entry(MenuItem::new("Run File").action(move || {
                                // Build a shell command based on the active file's extension.
                                if let Some(ref path) = s_run_file.open_file.get() {
                                    let ext =
                                        path.extension().and_then(|e| e.to_str()).unwrap_or("");
                                    let path_str = path.to_string_lossy().to_string();
                                    let cmd = match ext {
                                        "rs" => "cargo run".to_string(),
                                        "py" => format!("python3 {}", path_str),
                                        "js" => format!("node {}", path_str),
                                        "ts" => format!("npx ts-node {}", path_str),
                                        "sh" => format!("bash {}", path_str),
                                        "rb" => format!("ruby {}", path_str),
                                        "go" => format!("go run {}", path_str),
                                        _ => format!("./{}", path_str),
                                    };
                                    s_run_file.run_in_terminal_text.set(Some(cmd));
                                    s_run_file.show_bottom_panel.set(true);
                                    s_run_file.bottom_panel_tab.set(Tab::Terminal);
                                }
                            }));
                        show_context_menu(menu, None);
                    }
                }
            })
    };

    let chat = chat_panel(
        state.theme,
        state.ai_thinking,
        state.pending_chat_inject,
        state.workspace_root,
    );

    let chat_wrap = container(chat).style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.height_full()
            .background(p.glass_bg)
            .border_left(1.0)
            .border_color(p.glass_border)
            // Left-edge glow from chat panel
            .box_shadow_h_offset(-6.0)
            .box_shadow_v_offset(0.0)
            .box_shadow_blur(12.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
            .apply_if(!state.show_right_panel.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    // Drag handle between left panel and editor.
    let divider = {
        let style_s = state.clone();
        let down_s = state.clone();
        container(empty())
            .style(move |s| {
                let t = style_s.theme.get();
                let active = style_s.panel_drag_active.get();
                let shown = style_s.show_left_panel.get();
                s.width(4.0)
                    .height_full()
                    .cursor(floem::style::CursorStyle::ColResize)
                    .background(
                        t.palette
                            .glass_border
                            .with_alpha(if active { 0.8 } else { 0.0 }),
                    )
                    .apply_if(!shown, |s| s.display(floem::style::Display::None))
            })
            .on_event_stop(EventListener::PointerDown, move |e| {
                if let Event::PointerDown(pe) = e {
                    down_s.panel_drag_active.set(true);
                    down_s.panel_drag_start_x.set(pe.pos.x);
                    down_s
                        .panel_drag_start_width
                        .set(down_s.left_panel_width.get());
                }
            })
    };

    // Zen mode — hide sidebars / bottom / status bar for distraction-free editing
    let zen = state.zen_mode;

    let activity_wrap = container(activity_bar(state.clone()))
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));
    let left_wrap = container(left_panel(state.clone()))
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));
    let divider_wrap = container(divider)
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));
    let chat_zen_wrap = container(chat_wrap)
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));

    // ── Horizontal (down) split editor pane ──────────────────────────────────
    let down_raw = editor_panel(
        state.split_down_file,
        state.theme,
        state.ai_thinking,
        state.lsp_cmd.clone(),
        state.split_down_cursor,
        state.pending_completion,
        state.diagnostics,
        create_rw_signal(0u32),
        create_rw_signal(0u64),
        vec![],
        state.split_down_tabs,
        state.vim_motion,
        state.ghost_text,
        state.auto_save,
        state.workspace_root.get_untracked(),
        state.font_size,
        state.word_wrap,
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(String::new()),
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(Vec::new()),
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        state.vim_visual_mode,
        state.vim_marks,
        state.vim_last_motion,
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(false),                    // relative_line_numbers
        create_rw_signal(Vec::<String>::new()),     // yank_ring
        state.tab_size,                             // tab_size
        state.line_ending,                          // line_ending_out
        create_rw_signal(Vec::<(u32, u32)>::new()), // lsp_folding_ranges (down pane)
        create_rw_signal(0u64),                     // transform_title_nonce
        create_rw_signal(0u64),                     // format_selection_nonce
        create_rw_signal(0u64),                     // save_no_format_nonce
        create_rw_signal(0u64),                     // fold_all_nonce
        create_rw_signal(0u64),                     // unfold_all_nonce
        create_rw_signal(vec![]),                   // code_lens_sig
        create_rw_signal(true),                     // code_lens_visible
        create_rw_signal(false),                    // organize_imports_on_save
        create_rw_signal(vec![]),                   // inlay_hints_sig
        create_rw_signal(false),                    // inlay_hints_toggle
    );
    let down_pane = container(down_raw).style(move |s| {
        s.flex_grow(1.0)
            .min_width(0.0)
            .min_height(0.0)
            .apply_if(!state.split_editor_down.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });
    let down_divider = container(floem::views::empty()).style(move |s| {
        let t = state.theme.get();
        s.height(3.0)
            .width_full()
            .background(t.palette.glass_border)
            .apply_if(!state.split_editor_down.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    // Horizontal split: primary editor (+ side split) stacked with down pane
    let horiz_split_editors = stack((editor, split_divider, split_pane))
        .style(|s| s.flex_grow(1.0).min_width(0.0).min_height(0.0));
    let editor_area = stack((horiz_split_editors, down_divider, down_pane))
        .style(|s| s.flex_col().flex_grow(1.0).min_width(0.0).min_height(0.0));

    // Main content row: activity bar + left panel + resize handle + editor area + chat
    let content_row = stack((
        activity_wrap,
        left_wrap,
        divider_wrap,
        editor_area,
        chat_zen_wrap,
    ))
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    // Bottom panel (terminal etc.)
    let bottom_raw = bottom_panel(state.clone());
    let bottom_panel_max = state.bottom_panel_maximized;
    let bottom = container(bottom_raw).style(move |s| {
        let s = s.apply_if(zen.get(), |s| s.display(floem::style::Display::None));
        if bottom_panel_max.get() {
            s.flex_grow(10.0).min_height(0.0)
        } else {
            s
        }
    });

    let state_for_status = state.clone();
    let status_raw = status_bar(state_for_status);
    let status_wrap = container(status_raw)
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));

    stack((content_row, bottom, status_wrap)).style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.flex_col()
            .width_full()
            .height_full()
            .background(if t.is_cosmic() {
                floem::peniko::Color::TRANSPARENT
            } else {
                p.bg_base
            })
            .color(p.text_primary)
    })
}

// ─── Menu Bar ─────────────────────────────────────────────────────────────────

/// Custom in-app menu bar (Floem's native `.window_menu()` is Linux-unsupported).
/// Each label opens a context menu via `show_context_menu` on click.
fn menu_bar(state: IdeState) -> impl IntoView {
    // Helper: a hoverable menu-bar label.
    let make_item = |label_text: &'static str, theme: RwSignal<PhazeTheme>| {
        let hovered = create_rw_signal(false);
        container(label(move || label_text))
            .style(move |sty| {
                let t = theme.get();
                let p = &t.palette;
                sty.padding_horiz(16.0)
                    .height(28.0)
                    .items_center()
                    .justify_center()
                    .cursor(floem::style::CursorStyle::Pointer)
                    .font_size(13.0)
                    .color(p.text_secondary)
                    .apply_if(hovered.get(), |s| {
                        s.background(p.bg_elevated).color(p.text_primary)
                    })
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                hovered.set(true);
            })
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
                hovered.set(false);
            })
    };

    // ── File menu ────────────────────────────────────────────────────────────
    let file_item = {
        let s = state.clone();
        make_item("File", state.theme).on_click_stop(move |_| {
            let s2 = s.clone();
            let s3 = s.clone();
            let s4 = s.clone();
            let menu = Menu::new("File")
                .entry(MenuItem::new("Open File…\tCtrl+O").action(move || {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        s2.open_file.set(Some(path));
                    }
                }))
                .entry(MenuItem::new("Open Folder…").action(move || {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        s3.workspace_root.set(folder);
                        s3.file_picker_files.set(Vec::new());
                        s3.show_left_panel.set(true);
                        s3.left_panel_tab.set(Tab::Explorer);
                    }
                }))
                .separator()
                .entry(MenuItem::new("Exit").action(move || {
                    let _ = s4.clone();
                    std::process::exit(0);
                }));
            show_context_menu(menu, None);
        })
    };

    // ── Edit menu ────────────────────────────────────────────────────────────
    let edit_item = {
        let s = state.clone();
        make_item("Edit", state.theme).on_click_stop(move |_| {
            let s2 = s.clone();
            let s3 = s.clone();
            let s4 = s.clone();
            let menu = Menu::new("Edit")
                .entry(MenuItem::new("Toggle Comment\tCtrl+/").action(move || {
                    s2.comment_toggle_nonce.update(|v| *v += 1);
                }))
                .separator()
                .entry(MenuItem::new("Inline AI Edit\tCtrl+K").action(move || {
                    s3.inline_edit_open.set(true);
                    s3.inline_edit_query.set(String::new());
                }))
                .separator()
                .entry(
                    MenuItem::new("Command Palette\tCtrl+Shift+P").action(move || {
                        s4.command_palette_open.set(true);
                    }),
                );
            show_context_menu(menu, None);
        })
    };

    // ── View menu ────────────────────────────────────────────────────────────
    let view_item = {
        let s = state.clone();
        make_item("View", state.theme).on_click_stop(move |_| {
            let s_exp = s.clone();
            let s_term = s.clone();
            let s_chat = s.clone();
            let s_zen = s.clone();
            let s_zin = s.clone();
            let s_zout = s.clone();
            // Theme submenu
            let theme_menu = Menu::new("Theme")
                .entry(MenuItem::new("Midnight Blue").action({
                    let s = s.clone();
                    move || {
                        s.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::MidnightBlue));
                    }
                }))
                .entry(MenuItem::new("Cyberpunk 2077").action({
                    let s = s.clone();
                    move || {
                        s.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::Cyberpunk));
                    }
                }))
                .entry(MenuItem::new("Synthwave '84").action({
                    let s = s.clone();
                    move || {
                        s.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::Synthwave84));
                    }
                }))
                .entry(MenuItem::new("Andromeda").action({
                    let s = s.clone();
                    move || {
                        s.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::Andromeda));
                    }
                }))
                .entry(MenuItem::new("Dark").action({
                    let s = s.clone();
                    move || {
                        s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dark));
                    }
                }))
                .entry(MenuItem::new("Dracula").action({
                    let s = s.clone();
                    move || {
                        s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dracula));
                    }
                }))
                .entry(MenuItem::new("Tokyo Night").action({
                    let s = s.clone();
                    move || {
                        s.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::TokyoNight));
                    }
                }))
                .entry(MenuItem::new("Monokai").action({
                    let s = s.clone();
                    move || {
                        s.theme.set(PhazeTheme::from_variant(ThemeVariant::Monokai));
                    }
                }))
                .entry(MenuItem::new("Nord Dark").action({
                    let s = s.clone();
                    move || {
                        s.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::NordDark));
                    }
                }))
                .entry(MenuItem::new("Matrix Green").action({
                    let s = s.clone();
                    move || {
                        s.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::MatrixGreen));
                    }
                }))
                .entry(MenuItem::new("Root Shell").action({
                    let s = s.clone();
                    move || {
                        s.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::RootShell));
                    }
                }))
                .entry(MenuItem::new("Light").action({
                    let s = s.clone();
                    move || {
                        s.theme.set(PhazeTheme::from_variant(ThemeVariant::Light));
                    }
                }));

            let menu = Menu::new("View")
                .entry(MenuItem::new("Explorer\tCtrl+B").action(move || {
                    s_exp.show_left_panel.update(|v| *v = !*v);
                    let open = s_exp.show_left_panel.get();
                    s_exp.left_panel_width.set(if open { 260.0 } else { 0.0 });
                }))
                .entry(MenuItem::new("Terminal\tCtrl+J").action(move || {
                    s_term.show_bottom_panel.update(|v| *v = !*v);
                }))
                .entry(MenuItem::new("AI Chat\tCtrl+\\").action(move || {
                    s_chat.show_right_panel.update(|v| *v = !*v);
                }))
                .separator()
                .entry(MenuItem::new("Zoom In\tCtrl+=").action(move || {
                    s_zin.font_size.update(|v| *v = (*v + 1).min(32));
                }))
                .entry(MenuItem::new("Zoom Out\tCtrl+-").action(move || {
                    s_zout.font_size.update(|v| *v = v.saturating_sub(1).max(8));
                }))
                .separator()
                .entry(MenuItem::new("Zen Mode\tCtrl+Shift+Z").action(move || {
                    s_zen.zen_mode.update(|v| *v = !*v);
                }))
                .separator()
                .entry(theme_menu);
            show_context_menu(menu, None);
        })
    };

    // ── Go menu ──────────────────────────────────────────────────────────────
    let go_item = {
        let s = state.clone();
        make_item("Go", state.theme).on_click_stop(move |_| {
            let s_def = s.clone();
            let s_sym = s.clone();
            let s_fp = s.clone();
            let menu = Menu::new("Go")
                .entry(MenuItem::new("Go to Definition\tF12").action(move || {
                    if let Some((path, line, col)) = s_def.active_cursor.get() {
                        let _ =
                            s_def
                                .lsp_cmd
                                .send(LspCommand::RequestDefinition { path, line, col });
                    }
                }))
                .entry(
                    MenuItem::new("Find All References\tShift+F12").action(move || {
                        if let Some((path, line, col)) = s_sym.active_cursor.get() {
                            let _ = s_sym.lsp_cmd.send(LspCommand::RequestReferences {
                                path,
                                line,
                                col,
                            });
                            s_sym.references_visible.set(true);
                            s_sym.show_bottom_panel.set(true);
                            s_sym.bottom_panel_tab.set(Tab::References);
                        }
                    }),
                )
                .entry(MenuItem::new("Workspace Symbols\tCtrl+T").action(move || {
                    s_fp.ws_syms_open.set(true);
                    s_fp.ws_syms_query.set(String::new());
                    let _ = s_fp.lsp_cmd.send(LspCommand::RequestWorkspaceSymbols {
                        query: String::new(),
                    });
                }));
            show_context_menu(menu, None);
        })
    };

    // ── Run menu ─────────────────────────────────────────────────────────────
    let run_item = {
        let s = state.clone();
        make_item("Run", state.theme).on_click_stop(move |_| {
            let s_run = s.clone();
            let s_build = s.clone();
            let s_test = s.clone();
            let menu = Menu::new("Run")
                .entry(MenuItem::new("Open Terminal\tCtrl+J").action(move || {
                    s_run.show_bottom_panel.set(true);
                    s_run.bottom_panel_tab.set(Tab::Terminal);
                }))
                .separator()
                .entry(MenuItem::new("Show Build Output").action(move || {
                    s_build.show_bottom_panel.set(true);
                    s_build.bottom_panel_tab.set(Tab::Output);
                }))
                .entry(
                    MenuItem::new("Show Problems\tCtrl+Shift+M").action(move || {
                        s_test.show_bottom_panel.set(true);
                        s_test.bottom_panel_tab.set(Tab::Problems);
                    }),
                );
            show_context_menu(menu, None);
        })
    };

    // ── Help menu ────────────────────────────────────────────────────────────
    let help_item = {
        let s = state.clone();
        make_item("Help", state.theme).on_click_stop(move |_| {
            let s2 = s.clone();
            let menu = Menu::new("Help")
                .entry(
                    MenuItem::new("Command Palette\tCtrl+Shift+P").action(move || {
                        s2.command_palette_open.set(true);
                    }),
                )
                .separator()
                .entry(MenuItem::new("About PhazeAI IDE").action(|| {
                    rfd::MessageDialog::new()
                        .set_title("About PhazeAI IDE")
                        .set_description(
                            "PhazeAI IDE v0.1.0\n\n\
                            AI-native code editor built in Rust.\n\n\
                            MIT License\n\n\
                            https://github.com/jakes1345/phazeai-ide",
                        )
                        .set_buttons(rfd::MessageButtons::Ok)
                        .show();
                }));
            show_context_menu(menu, None);
        })
    };

    // ── Bar layout ───────────────────────────────────────────────────────────
    let bar_state = state.clone();
    stack((
        file_item, edit_item, view_item, go_item, run_item, help_item,
    ))
    .style(move |s| {
        let t = bar_state.theme.get();
        let p = &t.palette;
        s.flex_row()
            .width_full()
            .height(24.0)
            .min_height(24.0)
            .background(p.bg_deep)
            .border_bottom(1.0)
            .border_color(p.glass_border.with_alpha(0.25))
            .items_center()
            .padding_left(4.0)
            .z_index(10)
    })
}

/// Launch the PhazeAI IDE.
pub fn launch_phaze_ide() {
    let settings = Settings::load();

    Application::new()
        .window(
            move |_| {
                let state = IdeState::new(&settings);

                // Overlay layers — rendered after IDE content so they paint on top.
                let palette = command_palette(state.clone());
                let picker = file_picker(state.clone());
                let completions_popup = completion_popup(state.clone());
                let hover_tip = hover_tooltip(state.clone());
                let inline_edit = inline_edit_overlay(state.clone());
                let code_actions_popup = code_actions_overlay(state.clone());
                let rename_popup = rename_overlay(state.clone());
                let sig_help_popup = sig_help_overlay(state.clone());
                let toast_popup = toast_overlay(state.clone());
                let ws_syms_popup = workspace_symbols_overlay(state.clone());
                let branch_picker_popup = branch_picker_overlay(state.clone());
                let vim_ex_popup = vim_ex_overlay(state.clone());
                let goto_popup = goto_overlay(state.clone());
                let peek_def_popup = peek_def_overlay(state.clone());

                // Full-window drag capture overlay — only visible while a panel
                // resize is in progress (panel_drag_active == true).  By covering
                // the entire window it intercepts PointerMove/PointerUp even when
                // the cursor has moved past the divider into the editor area.
                let drag_overlay = {
                    let style_s = state.clone();
                    let move_s = state.clone();
                    let up_s = state.clone();
                    container(empty())
                        .style(move |s| {
                            let active = style_s.panel_drag_active.get();
                            s.absolute()
                                .inset(0)
                                .z_index(ui_const::Z_DRAG_OVERLAY)
                                .cursor(floem::style::CursorStyle::ColResize)
                                .apply_if(!active, |s| s.display(floem::style::Display::None))
                        })
                        .on_event_stop(EventListener::PointerMove, move |e| {
                            if let Event::PointerMove(pe) = e {
                                let delta = pe.pos.x - move_s.panel_drag_start_x.get();
                                let new_w = (move_s.panel_drag_start_width.get() + delta)
                                    .clamp(80.0, 700.0);
                                move_s.left_panel_width.set(new_w);
                                move_s.show_left_panel.set(true);
                            }
                        })
                        .on_event_stop(EventListener::PointerUp, move |_| {
                            up_s.panel_drag_active.set(false);
                        })
                };

                // Root: cosmic canvas + menu bar + IDE + overlays (overlays use z_index)
                let ide_with_menu = stack((menu_bar(state.clone()), ide_root(state.clone())))
                    .style(|s| s.flex_col().width_full().height_full().padding(16.0));

                // Floem stack() supports up to 16 children; nest into two groups.
                let overlays_b = stack((
                    peek_def_popup, // Z_PEEK_DEF(485) — peek definition (Alt+F12)
                    vim_ex_popup,   // Z_VIM_EX(490) — vim ex command bar
                    goto_popup,     // Z_GOTO(495) — goto line/col (Ctrl+G)
                    drag_overlay,   // Z_DRAG_OVERLAY(50) — only shown during resize
                ))
                .style(|s| {
                    s.absolute()
                        .width_full()
                        .height_full()
                        .pointer_events(floem::style::PointerEvents::None)
                });

                stack((
                    cosmic_bg_canvas(state.theme),
                    ide_with_menu,
                    palette,             // Z_COMMAND_PALETTE(100)
                    picker,              // Z_FILE_PICKER(200) — on top of palette
                    hover_tip,           // Z_HOVER_TIP(250) — LSP hover doc
                    completions_popup,   // Z_COMPLETIONS(300) — above palette/picker
                    code_actions_popup,  // Z_CODE_ACTIONS(350) — code actions / quick-fix
                    sig_help_popup,      // Z_SIG_HELP(380) — signature help tooltip
                    inline_edit,         // Z_INLINE_EDIT(400) — highest overlay
                    rename_popup,        // Z_RENAME(420) — rename dialog
                    toast_popup,         // Z_TOAST(450) — toast notifications
                    ws_syms_popup,       // Z_WS_SYMBOLS(460) — workspace symbols (Ctrl+T)
                    branch_picker_popup, // Z_BRANCH_PICKER(470) — branch switcher
                    overlays_b,
                ))
                .style(move |s| {
                    let t = state.theme.get();
                    let p = &t.palette;
                    s.width_full().height_full().background(p.bg_base)
                })
                .on_event_stop(EventListener::KeyDown, {
                    let state = state.clone();
                    move |event| {
                        if let Event::KeyDown(key_event) = event {
                            let ctrl = key_event.modifiers.contains(Modifiers::CONTROL);
                            let shift = key_event.modifiers.contains(Modifiers::SHIFT);
                            let alt = key_event.modifiers.contains(Modifiers::ALT);

                            if ctrl && !alt {
                                if let Some(cmd) = match_global_shortcut(key_event) {
                                    match cmd {
                                        GlobalShortcut::ToggleLeftPanel => {
                                            state.show_left_panel.update(|v| *v = !*v);
                                            let now_open = state.show_left_panel.get();
                                            let new_w = if now_open { 260.0 } else { 0.0 };
                                            state.left_panel_width.set(new_w);
                                            session_update(|s| {
                                                s.left_panel_width = new_w;
                                                s.show_left_panel = now_open;
                                            });
                                            return;
                                        }
                                        GlobalShortcut::ToggleBottomPanel => {
                                            state.show_bottom_panel.update(|v| *v = !*v);
                                            session_update(|s| {
                                                s.show_bottom_panel = state.show_bottom_panel.get();
                                            });
                                            return;
                                        }
                                        GlobalShortcut::ToggleRightPanel => {
                                            state.show_right_panel.update(|v| *v = !*v);
                                            return;
                                        }
                                        GlobalShortcut::ToggleFilePicker => {
                                            let open = state.file_picker_open.get();
                                            state.file_picker_open.set(!open);
                                            if !open {
                                                state.file_picker_query.set(String::new());
                                            }
                                            return;
                                        }
                                        GlobalShortcut::ToggleCommandPalette => {
                                            let open = state.command_palette_open.get();
                                            state.command_palette_open.set(!open);
                                            return;
                                        }
                                    }
                                }
                            }

                            // ── Named keys ───────────────────────────────────────
                            if let Key::Named(ref named) = key_event.key.logical_key {
                                match named {
                                    floem::keyboard::NamedKey::Escape => {
                                        if state.peek_def_open.get() {
                                            state.peek_def_open.set(false);
                                            state.peek_def_lines.set(vec![]);
                                            return;
                                        }
                                        if state.branch_picker_open.get() {
                                            state.branch_picker_open.set(false);
                                            return;
                                        }
                                        if state.rename_open.get() {
                                            state.rename_open.set(false);
                                            return;
                                        }
                                        if state.sig_help.get().is_some() {
                                            state.sig_help.set(None);
                                            return;
                                        }
                                        if state.code_actions_open.get() {
                                            state.code_actions_open.set(false);
                                            return;
                                        }
                                        if state.inline_edit_open.get() {
                                            state.inline_edit_open.set(false);
                                            state.inline_edit_query.set(String::new());
                                            return;
                                        }
                                        if state.completion_open.get() {
                                            state.completion_open.set(false);
                                            return;
                                        }
                                        if state.file_picker_open.get() {
                                            state.file_picker_open.set(false);
                                            state.file_picker_query.set(String::new());
                                            return;
                                        }
                                        if state.command_palette_open.get() {
                                            state.command_palette_open.set(false);
                                            state.command_palette_query.set(String::new());
                                            return;
                                        }
                                        // Vim: Escape enters Normal mode / exits ex/visual
                                        if state.vim_mode.get() {
                                            if state.vim_ex_open.get() {
                                                state.vim_ex_open.set(false);
                                                state.vim_ex_input.set(String::new());
                                                return;
                                            }
                                            if state.vim_visual_mode.get() {
                                                state.vim_visual_mode.set(false);
                                            }
                                            state.vim_normal_mode.set(true);
                                            state.vim_pending_key.set(None);
                                            return;
                                        }
                                    }
                                    floem::keyboard::NamedKey::Tab => {
                                        // Tab accepts ghost text (FIM) suggestion first.
                                        if let Some(suggestion) = state.ghost_text.get() {
                                            // Ghost text: insert at cursor, no prefix to delete.
                                            state.pending_completion.set(Some((suggestion, 0)));
                                            state.ghost_text.set(None);
                                            return;
                                        }
                                        // Tab also accepts LSP completion popup.
                                        if state.completion_open.get() {
                                            let items = state.completions.get();
                                            let sel = state.completion_selected.get();
                                            let prefix_b = state.completion_filter_text.get().len();
                                            if let Some(entry) = items.get(sel) {
                                                let text = if entry.insert_text.is_empty() {
                                                    entry.label.clone()
                                                } else {
                                                    entry.insert_text.clone()
                                                };
                                                state
                                                    .pending_completion
                                                    .set(Some((text, prefix_b)));
                                            }
                                            state.completion_open.set(false);
                                            state.completion_filter_text.set(String::new());
                                            return;
                                        }
                                    }
                                    floem::keyboard::NamedKey::Enter => {
                                        if state.completion_open.get() {
                                            let items = state.completions.get();
                                            let sel = state.completion_selected.get();
                                            let prefix_b = state.completion_filter_text.get().len();
                                            if let Some(entry) = items.get(sel) {
                                                let text = if entry.insert_text.is_empty() {
                                                    entry.label.clone()
                                                } else {
                                                    entry.insert_text.clone()
                                                };
                                                state
                                                    .pending_completion
                                                    .set(Some((text, prefix_b)));
                                            }
                                            state.completion_open.set(false);
                                            state.completion_filter_text.set(String::new());
                                            return;
                                        }
                                    }
                                    // F12 — go to definition; Shift+F12 — find all references; Alt+F12 — peek definition; Ctrl+F12 — go to implementation
                                    floem::keyboard::NamedKey::F12 => {
                                        if let Some((path, line, col)) = state.active_cursor.get() {
                                            if ctrl {
                                                // Ctrl+F12: go to implementation
                                                let _ = state.lsp_cmd.send(
                                                    LspCommand::RequestImplementation {
                                                        path,
                                                        line,
                                                        col,
                                                    },
                                                );
                                            } else if shift {
                                                // Shift+F12: find all references
                                                let _ = state.lsp_cmd.send(
                                                    LspCommand::RequestReferences {
                                                        path,
                                                        line,
                                                        col,
                                                    },
                                                );
                                                state.references_visible.set(true);
                                                state.show_bottom_panel.set(true);
                                                state.bottom_panel_tab.set(Tab::References);
                                            } else if alt {
                                                // Alt+F12: peek definition
                                                state.peek_def_lines.set(vec![]);
                                                state.peek_def_open.set(false);
                                                let _ = state.lsp_cmd.send(
                                                    LspCommand::RequestPeekDefinition {
                                                        path,
                                                        line,
                                                        col,
                                                    },
                                                );
                                            } else {
                                                // F12: go to definition
                                                let _ = state.lsp_cmd.send(
                                                    LspCommand::RequestDefinition {
                                                        path,
                                                        line,
                                                        col,
                                                    },
                                                );
                                            }
                                        }
                                        return;
                                    }
                                    // F1 with Ctrl — show hover documentation
                                    floem::keyboard::NamedKey::F1 => {
                                        if ctrl {
                                            if let Some((path, line, col)) =
                                                state.active_cursor.get()
                                            {
                                                let _ =
                                                    state.lsp_cmd.send(LspCommand::RequestHover {
                                                        path,
                                                        line,
                                                        col,
                                                    });
                                            }
                                            return;
                                        }
                                    }
                                    // F2 — rename symbol at cursor
                                    floem::keyboard::NamedKey::F2 => {
                                        if let Some((path, line, col)) = state.active_cursor.get() {
                                            // Prefill rename box with the word under cursor
                                            let word = std::fs::read_to_string(&path)
                                                .ok()
                                                .and_then(|content| {
                                                    let target_line = content
                                                        .lines()
                                                        .nth(line as usize)?
                                                        .to_string();
                                                    let col = (col as usize).min(target_line.len());
                                                    let start = target_line[..col]
                                                        .char_indices()
                                                        .rev()
                                                        .take_while(|(_, c)| {
                                                            c.is_alphanumeric() || *c == '_'
                                                        })
                                                        .last()
                                                        .map(|(i, _)| i)
                                                        .unwrap_or(col);
                                                    let end = target_line[col..]
                                                        .char_indices()
                                                        .take_while(|(_, c)| {
                                                            c.is_alphanumeric() || *c == '_'
                                                        })
                                                        .last()
                                                        .map(|(i, _)| col + i + 1)
                                                        .unwrap_or(col);
                                                    let w = target_line[start..end].to_string();
                                                    if w.is_empty() {
                                                        None
                                                    } else {
                                                        Some(w)
                                                    }
                                                })
                                                .unwrap_or_default();
                                            state.rename_target.set(word.clone());
                                            state.rename_query.set(word);
                                            state.rename_open.set(true);
                                        }
                                        return;
                                    }
                                    // Alt+Up/Down — move or duplicate line
                                    floem::keyboard::NamedKey::ArrowUp
                                        if alt && !ctrl && !shift =>
                                    {
                                        state.move_line_up_nonce.update(|n| *n += 1);
                                        return;
                                    }
                                    floem::keyboard::NamedKey::ArrowDown if alt && !ctrl => {
                                        if shift {
                                            state.duplicate_line_nonce.update(|n| *n += 1);
                                        } else {
                                            state.move_line_down_nonce.update(|n| *n += 1);
                                        }
                                        return;
                                    }
                                    // Ctrl+Alt+Up/Down — add column cursor on adjacent line
                                    floem::keyboard::NamedKey::ArrowUp if ctrl && alt && !shift => {
                                        state.col_cursor_up_nonce.update(|n| *n += 1);
                                        return;
                                    }
                                    floem::keyboard::NamedKey::ArrowDown
                                        if ctrl && alt && !shift =>
                                    {
                                        state.col_cursor_down_nonce.update(|n| *n += 1);
                                        return;
                                    }
                                    _ => {}
                                }
                            }

                            // Ctrl+Space → request LSP completions and open popup
                            if ctrl
                                && key_event.key.logical_key
                                    == Key::Named(floem::keyboard::NamedKey::Space)
                            {
                                if let Some((path, line, col)) = state.active_cursor.get() {
                                    // Compute word before cursor as the filter prefix.
                                    let prefix = std::fs::read_to_string(&path)
                                        .ok()
                                        .and_then(|content| {
                                            let lines: Vec<&str> = content.lines().collect();
                                            let line_str = lines.get(line as usize)?;
                                            let col = (col as usize).min(line_str.len());
                                            let prefix: String = line_str[..col]
                                                .chars()
                                                .rev()
                                                .take_while(|c| c.is_alphanumeric() || *c == '_')
                                                .collect::<String>()
                                                .chars()
                                                .rev()
                                                .collect();
                                            Some(prefix)
                                        })
                                        .unwrap_or_default();
                                    state.completion_filter_text.set(prefix);
                                    let _ = state.lsp_cmd.send(LspCommand::RequestCompletions {
                                        path,
                                        line,
                                        col,
                                    });
                                }
                                state.completion_selected.set(0);
                                state.completion_open.set(true);
                                return;
                            }

                            // Ctrl+G → goto line/col overlay
                            if ctrl
                                && !shift
                                && !alt
                                && key_event.key.logical_key == Key::Character("g".into())
                            {
                                state.goto_overlay_open.set(true);
                                state.goto_overlay_input.set(String::new());
                                return;
                            }

                            // Ctrl+Alt+S → save without formatting
                            if ctrl
                                && !shift
                                && alt
                                && key_event.key.logical_key == Key::Character("s".into())
                            {
                                state.save_no_format_nonce.update(|v| *v += 1);
                                return;
                            }

                            // Ctrl+Alt+I → toggle inlay hints
                            if ctrl
                                && !shift
                                && alt
                                && key_event.key.logical_key == Key::Character("i".into())
                            {
                                state.inlay_hints_toggle.update(|v| *v = !*v);
                                let msg = if state.inlay_hints_toggle.get() {
                                    "Inlay Hints: on"
                                } else {
                                    "Inlay Hints: off"
                                };
                                show_toast(state.status_toast, msg);
                                return;
                            }

                            // Ctrl+N → new scratch file (untitled buffer)
                            if ctrl
                                && !shift
                                && !alt
                                && key_event.key.logical_key == Key::Character("n".into())
                            {
                                let n = state.scratch_counter.get() + 1;
                                state.scratch_counter.set(n);
                                let scratch_path =
                                    std::path::PathBuf::from(format!("scratch://untitled-{n}"));
                                state.scratch_paths.update(|v| v.push(scratch_path.clone()));
                                state.open_file.set(Some(scratch_path));
                                return;
                            }

                            // Ctrl+T → workspace symbols overlay
                            if ctrl
                                && !shift
                                && key_event.key.logical_key == Key::Character("t".into())
                            {
                                let open = state.ws_syms_open.get();
                                state.ws_syms_open.set(!open);
                                if !open {
                                    state.ws_syms_query.set(String::new());
                                    // Kick off an empty-query search to pre-populate list.
                                    let _ =
                                        state.lsp_cmd.send(LspCommand::RequestWorkspaceSymbols {
                                            query: String::new(),
                                        });
                                }
                                return;
                            }

                            // Ctrl+. → code actions
                            if ctrl && key_event.key.logical_key == Key::Character(".".into()) {
                                if let Some((path, line, col)) = state.active_cursor.get() {
                                    let _ = state.lsp_cmd.send(LspCommand::RequestCodeActions {
                                        path,
                                        line,
                                        col,
                                    });
                                }
                                state.code_actions_open.set(true);
                                return;
                            }

                            // Ctrl+Shift+Space → signature help
                            if ctrl
                                && shift
                                && key_event.key.logical_key
                                    == Key::Named(floem::keyboard::NamedKey::Space)
                            {
                                if let Some((path, line, col)) = state.active_cursor.get() {
                                    let _ = state.lsp_cmd.send(LspCommand::RequestSignatureHelp {
                                        path,
                                        line,
                                        col,
                                    });
                                }
                                return;
                            }

                            if let Key::Character(ref ch) = key_event.key.logical_key {
                                let ch = ch.clone();

                                // Alt+Z — toggle word wrap
                                if alt && !ctrl && !shift && ch.as_str() == "z" {
                                    state.word_wrap.update(|v| *v = !*v);
                                    let msg = if state.word_wrap.get() {
                                        "Word wrap on"
                                    } else {
                                        "Word wrap off"
                                    };
                                    show_toast(state.status_toast, msg);
                                    return;
                                }

                                // Ctrl+Alt+\ — toggle split editor pane
                                if ctrl && alt && !shift && ch.as_str() == "\\" {
                                    let was_open = state.split_editor.get();
                                    state.split_editor.update(|v| *v = !*v);
                                    if !was_open && state.split_open_file.get().is_none() {
                                        state.split_open_file.set(state.open_file.get());
                                    }
                                    return;
                                }

                                if ctrl && !shift && !alt {
                                    match ch.as_str() {
                                        // Ctrl+= / Ctrl++ — zoom in editor font
                                        "=" | "+" => {
                                            state.font_size.update(|v| *v = (*v + 1).min(40));
                                            return;
                                        }
                                        // Ctrl+- — zoom out editor font
                                        "-" => {
                                            state
                                                .font_size
                                                .update(|v| *v = v.saturating_sub(1).max(8));
                                            return;
                                        }
                                        // Ctrl+0 — reset editor font to default
                                        "0" => {
                                            state.font_size.set(14);
                                            return;
                                        }
                                        // Ctrl+D — vim half-page down OR multi-cursor
                                        "d" => {
                                            if state.vim_mode.get() && state.vim_normal_mode.get() {
                                                state.vim_motion.set(Some(VimMotion::HalfPageDown));
                                            } else {
                                                state.ctrl_d_nonce.update(|v| *v += 1);
                                            }
                                            return;
                                        }
                                        // Ctrl+U — vim half-page up
                                        "u" => {
                                            if state.vim_mode.get() && state.vim_normal_mode.get() {
                                                state.vim_motion.set(Some(VimMotion::HalfPageUp));
                                                return;
                                            }
                                        }
                                        // Ctrl+K — open inline AI edit overlay
                                        "k" => {
                                            state.inline_edit_open.set(true);
                                            state.inline_edit_query.set(String::new());
                                        }
                                        // Ctrl+/ — toggle line comment
                                        "/" => {
                                            state.comment_toggle_nonce.update(|v| *v += 1);
                                        }
                                        _ => {}
                                    }
                                }

                                // Ctrl+Shift+Z → toggle zen mode
                                if ctrl && shift && !alt {
                                    if ch.as_str() == "z" {
                                        state.zen_mode.update(|v| *v = !*v);
                                        let toast_msg = if state.zen_mode.get() {
                                            "Zen mode on — Ctrl+Shift+Z to exit"
                                        } else {
                                            "Zen mode off"
                                        };
                                        show_toast(state.status_toast, toast_msg);
                                        return;
                                    }
                                    // Ctrl+Shift+[ → fold block at cursor
                                    if ch.as_str() == "[" {
                                        state.fold_nonce.update(|v| *v += 1);
                                        show_toast(state.status_toast, "Folded");
                                        return;
                                    }
                                    // Ctrl+Shift+] → unfold block at cursor
                                    if ch.as_str() == "]" {
                                        state.unfold_nonce.update(|v| *v += 1);
                                        show_toast(state.status_toast, "Unfolded");
                                        return;
                                    }
                                    // Ctrl+Shift+K → delete entire line
                                    if ch.as_str() == "k" {
                                        state.delete_line_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+U → transform uppercase
                                    if ch.as_str() == "u" {
                                        state.transform_upper_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+L → transform lowercase
                                    if ch.as_str() == "l" {
                                        state.transform_lower_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+T → transform title case
                                    if ch.as_str() == "t" {
                                        state.transform_title_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+J → join lines
                                    if ch.as_str() == "j" {
                                        state.join_line_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+V → cycle yank ring and paste
                                    if ch.as_str() == "v" {
                                        let ring = state.yank_ring.get();
                                        if !ring.is_empty() {
                                            let idx = (state.yank_ring_idx.get() + 1) % ring.len();
                                            state.yank_ring_idx.set(idx);
                                            let text = ring[idx].clone();
                                            state.pending_completion.set(Some((text, 0)));
                                        }
                                        return;
                                    }
                                }

                                // Ctrl+Alt+Shift+D → split editor down toggle
                                if ctrl && alt && shift && ch.as_str() == "d" {
                                    state.split_editor_down.update(|v| *v = !*v);
                                    return;
                                }

                                // ── Vim normal-mode keys (no Ctrl) ───────────────
                                if state.vim_mode.get()
                                    && state.vim_normal_mode.get()
                                    && !ctrl
                                    && !alt
                                {
                                    let pending = state.vim_pending_key.get();
                                    let ch_str = ch.as_str();

                                    // Two-key sequences
                                    if let Some(prev) = pending {
                                        state.vim_pending_key.set(None);
                                        match (prev, ch_str) {
                                            ('d', "d") => {
                                                state.vim_motion.set(Some(VimMotion::DeleteLine));
                                                state
                                                    .vim_last_motion
                                                    .set(Some(VimMotion::DeleteLine));
                                            }
                                            ('g', "g") => {
                                                state.vim_motion.set(Some(VimMotion::GotoFileTop));
                                            }
                                            ('y', "y") => {
                                                state.vim_motion.set(Some(VimMotion::YankLine));
                                            }
                                            ('c', "c") => {
                                                state.vim_normal_mode.set(false);
                                                state
                                                    .vim_motion
                                                    .set(Some(VimMotion::ChangeWholeLine));
                                                state
                                                    .vim_last_motion
                                                    .set(Some(VimMotion::ChangeWholeLine));
                                            }
                                            ('c', "w") => {
                                                state.vim_normal_mode.set(false);
                                                state.vim_motion.set(Some(VimMotion::ChangeWord));
                                                state
                                                    .vim_last_motion
                                                    .set(Some(VimMotion::ChangeWord));
                                            }
                                            ('r', _) => {
                                                if let Some(c) = ch_str.chars().next() {
                                                    state
                                                        .vim_motion
                                                        .set(Some(VimMotion::ReplaceChar(c)));
                                                    state
                                                        .vim_last_motion
                                                        .set(Some(VimMotion::ReplaceChar(c)));
                                                }
                                            }
                                            ('m', _) => {
                                                if let Some(c) = ch_str.chars().next() {
                                                    state
                                                        .vim_motion
                                                        .set(Some(VimMotion::SetMark(c)));
                                                }
                                            }
                                            ('`', _) => {
                                                if let Some(c) = ch_str.chars().next() {
                                                    state
                                                        .vim_motion
                                                        .set(Some(VimMotion::GotoMark(c)));
                                                }
                                            }
                                            _ => {}
                                        }
                                        return;
                                    }

                                    // Visual mode intercepts d/y/c to operate on selection
                                    if state.vim_visual_mode.get_untracked() {
                                        match ch_str {
                                            "d" | "x" => {
                                                state
                                                    .vim_motion
                                                    .set(Some(VimMotion::DeleteVisualSelection));
                                                state.vim_visual_mode.set(false);
                                                state
                                                    .vim_last_motion
                                                    .set(Some(VimMotion::DeleteVisualSelection));
                                                return;
                                            }
                                            "y" => {
                                                state
                                                    .vim_motion
                                                    .set(Some(VimMotion::YankVisualSelection));
                                                state.vim_visual_mode.set(false);
                                                return;
                                            }
                                            "c" => {
                                                state.vim_visual_mode.set(false);
                                                state.vim_normal_mode.set(false);
                                                state
                                                    .vim_motion
                                                    .set(Some(VimMotion::ChangeVisualSelection));
                                                state
                                                    .vim_last_motion
                                                    .set(Some(VimMotion::ChangeVisualSelection));
                                                return;
                                            }
                                            _ => {} // fall through to normal motion handling
                                        }
                                    }

                                    // Single-key normal mode commands
                                    match ch_str {
                                        "h" => {
                                            state.vim_motion.set(Some(VimMotion::Left));
                                        }
                                        "j" => {
                                            state.vim_motion.set(Some(VimMotion::Down));
                                        }
                                        "k" => {
                                            state.vim_motion.set(Some(VimMotion::Up));
                                        }
                                        "l" => {
                                            state.vim_motion.set(Some(VimMotion::Right));
                                        }
                                        "w" => {
                                            state.vim_motion.set(Some(VimMotion::WordForward));
                                        }
                                        "b" => {
                                            state.vim_motion.set(Some(VimMotion::WordBackward));
                                        }
                                        "0" => {
                                            state.vim_motion.set(Some(VimMotion::LineStart));
                                        }
                                        "$" => {
                                            state.vim_motion.set(Some(VimMotion::LineEnd));
                                        }
                                        "x" => {
                                            state.vim_motion.set(Some(VimMotion::DeleteChar));
                                        }
                                        "i" => {
                                            state.vim_normal_mode.set(false);
                                            state.vim_motion.set(Some(VimMotion::EnterInsert));
                                        }
                                        "a" => {
                                            state.vim_normal_mode.set(false);
                                            state.vim_motion.set(Some(VimMotion::EnterInsertAfter));
                                        }
                                        "o" => {
                                            state.vim_normal_mode.set(false);
                                            state
                                                .vim_motion
                                                .set(Some(VimMotion::EnterInsertNewlineBelow));
                                        }
                                        // p / P — paste from vim register
                                        "p" => {
                                            state.vim_motion.set(Some(VimMotion::Paste));
                                        }
                                        "P" => {
                                            state.vim_motion.set(Some(VimMotion::PasteBefore));
                                        }
                                        // G — go to end of file
                                        "G" => {
                                            state.vim_motion.set(Some(VimMotion::GotoFileBottom));
                                        }
                                        // A — insert at end of line
                                        "A" => {
                                            state.vim_normal_mode.set(false);
                                            state.vim_motion.set(Some(VimMotion::InsertAtLineEnd));
                                        }
                                        // I — insert at start of line
                                        "I" => {
                                            state.vim_normal_mode.set(false);
                                            state
                                                .vim_motion
                                                .set(Some(VimMotion::InsertAtLineStart));
                                        }
                                        // C — change to end of line (delete + insert)
                                        "C" => {
                                            state.vim_normal_mode.set(false);
                                            state.vim_motion.set(Some(VimMotion::ChangeToLineEnd));
                                        }
                                        // D — delete to end of line
                                        "D" => {
                                            state.vim_motion.set(Some(VimMotion::DeleteToLineEnd));
                                            state
                                                .vim_last_motion
                                                .set(Some(VimMotion::DeleteToLineEnd));
                                        }
                                        // % — jump to matching bracket
                                        "%" => {
                                            state
                                                .vim_motion
                                                .set(Some(VimMotion::JumpMatchingBracket));
                                        }
                                        // v — start char-wise visual mode
                                        "v" => {
                                            state.vim_visual_mode.set(true);
                                            state.vim_visual_line.set(false);
                                            state.vim_motion.set(Some(VimMotion::VisualCharStart));
                                        }
                                        // V — start line-wise visual mode
                                        "V" => {
                                            state.vim_visual_mode.set(true);
                                            state.vim_visual_line.set(true);
                                            state.vim_motion.set(Some(VimMotion::VisualLineStart));
                                        }
                                        // Escape in visual mode — return to normal
                                        // (handled in NamedKey::Escape section below)
                                        // . — repeat last change
                                        "." => {
                                            if let Some(last) = state.vim_last_motion.get() {
                                                state.vim_motion.set(Some(last));
                                            }
                                        }
                                        // : — open ex command bar
                                        ":" => {
                                            state.vim_ex_open.set(true);
                                            state.vim_ex_input.set(String::new());
                                        }
                                        // d, g, y, c, r, m, ` — pending keys for two-key sequences
                                        "d" | "g" | "y" | "c" | "r" | "m" | "`" => {
                                            state
                                                .vim_pending_key
                                                .set(Some(ch_str.chars().next().unwrap()));
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                })
                .on_event_stop(EventListener::WindowClosed, {
                    let state = state.clone();
                    move |_| {
                        // Kill sidecar process cleanly on IDE exit.
                        if let Ok(guard) = state.sidecar_client.lock() {
                            if let Some(client) = guard.as_ref() {
                                // Build a small runtime just for the shutdown call.
                                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                {
                                    let client = client.clone();
                                    let _ = rt.block_on(client.shutdown());
                                }
                            }
                        }
                        // Save session state on close.
                        let open = state.open_file.get();
                        let theme_name = state.theme.get().variant.name();
                        session_update(|s| {
                            if let Some(f) = open {
                                s.active_tab_index = s.open_tabs.iter().position(|t| t == &f);
                            }
                            s.left_panel_width = state.left_panel_width.get();
                            s.show_bottom_panel = state.show_bottom_panel.get();
                            s.vim_mode = state.vim_mode.get();
                            s.theme = theme_name.to_string();
                        });
                    }
                })
            },
            Some(
                WindowConfig::default()
                    .title("PhazeAI IDE")
                    .size(Size::new(1400.0, 900.0)),
            ),
        )
        .run();
}
