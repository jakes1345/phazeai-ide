use eframe::egui;
use egui::{Color32, RichText};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use similar::{ChangeTag, DiffOp, TextDiff};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::sync::{mpsc, oneshot};

use phazeai_core::{AgentEvent, ConversationHistory, LspEvent, LspManager, Settings};
use crate::state::IdeState;

use crate::keybindings::{self, Action};
use crate::panels::browser::BrowserPanel;
use crate::panels::chat::{ChatPanel, AiMode};
use crate::panels::diff::DiffPanel;
use crate::panels::editor::EditorPanel;
use crate::panels::explorer::ExplorerPanel;
use crate::panels::outline::OutlinePanel;
use crate::panels::search::SearchPanel;
use crate::panels::settings::SettingsPanel;
use crate::panels::terminal::TerminalPanel;
use crate::themes::{ThemeColors, ThemePreset};

#[derive(Clone, PartialEq)]
enum LeftPanelTab {
    Explorer,
    Search,
    Git,
    Outline,
}

struct InlineChatState {
    visible: bool,
    query: String,
    is_streaming: bool,
    streaming_text: String,
    just_opened: bool,
}

/// A single diff hunk with per-hunk accept/reject state.
#[derive(Clone)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<(HunkLineKind, String)>,
    pub old_start: usize,
    pub old_end: usize,
    pub accepted: bool,
}

#[derive(Clone, PartialEq)]
pub enum HunkLineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone)]
pub struct PendingApproval {
    pub tool_name: String,
    pub params_summary: String,
    /// Pre-computed diff string for write_file / edit_file tools (fallback display)
    pub diff: Option<String>,
    /// Structured per-hunk data for file edit tools
    pub hunks: Vec<DiffHunk>,
    /// Original file content (for reconstruction)
    pub old_content: String,
    /// Full new content the agent wants to write (if all hunks accepted)
    pub new_content: String,
    /// File path for direct write when applying partial hunks
    pub file_path: Option<String>,
    /// Full params JSON for context
    pub full_params: serde_json::Value,
}

/// A record of a completed agent run shown in agent history.
#[derive(Clone)]
pub struct AgentRun {
    pub timestamp: String,
    pub mode_label: String,
    pub summary: String,
    pub tool_count: usize,
}

// Shared state for handling approval requests
struct ApprovalState {
    pending: Option<(String, String, serde_json::Value, oneshot::Sender<bool>)>,
    // (tool_name, params_summary, full_params, response_tx)
}

// Command palette item types
#[derive(Clone)]
enum PaletteAction {
    OpenFile(PathBuf),
    NewFile,
    OpenFileDialog,
    Save,
    ToggleExplorer,
    ToggleChat,
    ToggleTerminal,
    ToggleSearch,
    OpenSettings,
    SwitchMode(AiMode),
    ToggleInlineChat,
}

pub struct PhazeApp {
    // Panels
    pub editor: EditorPanel,
    pub explorer: ExplorerPanel,
    pub search: SearchPanel,
    pub chat: ChatPanel,
    pub browser: BrowserPanel,
    pub terminal: TerminalPanel,
    pub settings_panel: SettingsPanel,
    pub diff: DiffPanel,
    pub outline: OutlinePanel,

    // Panel visibility
    show_explorer: bool,
    show_chat: bool,
    show_browser: bool,
    show_terminal: bool,
    show_about: bool,
    left_tab: LeftPanelTab,

    // Command palette
    show_palette: bool,
    palette_query: String,
    palette_selected: usize,
    palette_files: Vec<PathBuf>,
    palette_just_opened: bool,

    // Theme
    theme_preset: ThemePreset,
    theme: ThemeColors,

    // Layout sizes
    explorer_width: f32,
    chat_width: f32,
    terminal_height: f32,

    // Inline chat (Ctrl+K)
    inline_chat: InlineChatState,
    inline_event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
    inline_event_tx: mpsc::UnboundedSender<AgentEvent>,

    // Agent communication
    event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Cancellation token for the currently running agent
    agent_cancel: Option<tokio::sync::oneshot::Sender<()>>,

    // Tool approval
    pending_approval: Option<PendingApproval>,
    approval_state: Arc<Mutex<ApprovalState>>,

    // Tokio runtime for async operations
    runtime: tokio::runtime::Runtime,

    // Settings
    settings: Settings,

    // File watcher (notify-based)
    file_mod_times: HashMap<PathBuf, SystemTime>,
    last_file_check: std::time::Instant,
    file_watcher: Option<RecommendedWatcher>,
    file_watch_rx: Option<std::sync::mpsc::Receiver<PathBuf>>,

    // Browser state
    last_browser_url: String,

    // Persistent conversation history shared across all agent invocations
    agent_conversation: Arc<tokio::sync::Mutex<ConversationHistory>>,

    // Status notification (timed, shown in status bar)
    status_notification: Option<(String, bool, std::time::Instant)>, // (message, success, time)

    // Last state save time (save every 30s)
    last_state_save: std::time::Instant,

    // LSP
    lsp_manager: Option<LspManager>,
    lsp_event_rx: Option<mpsc::UnboundedReceiver<LspEvent>>,

    // Agent history (last 20 runs)
    agent_history: Vec<AgentRun>,
    show_agent_history: bool,

    // Split editor view
    split_editor: Option<EditorPanel>,
    split_ratio: f32,
}

impl PhazeApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, settings: Settings) -> Self {
        Self::new_headless(settings)
    }

    pub fn new_headless(settings: Settings) -> Self {
        let ide_state = IdeState::load();
        let theme_preset = resolve_theme_preset(&settings.editor.theme);
        let theme = ThemeColors::from_preset(&theme_preset);

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (inline_tx, inline_rx) = mpsc::unbounded_channel();

        let runtime = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");

        // Use last workspace if available, otherwise cwd
        let cwd = ide_state.last_workspace.clone()
            .filter(|p| p.exists())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));

        let mut explorer = ExplorerPanel::new();
        explorer.set_root(cwd.clone());

        let mut search_panel = SearchPanel::new();
        search_panel.set_root(cwd.clone());

        let mut terminal = TerminalPanel::new();
        terminal.set_cwd(cwd.clone());

        let mut diff_panel = DiffPanel::new();
        diff_panel.set_git_root(cwd);

        let editor = EditorPanel::new(settings.editor.font_size);
        let chat = ChatPanel::new();
        let browser = BrowserPanel::new();
        let settings_panel = SettingsPanel::new(settings.clone());
        let outline = OutlinePanel::new();

        Self {
            editor,
            explorer,
            search: search_panel,
            chat,
            browser,
            terminal,
            settings_panel,
            diff: diff_panel,
            outline,

            inline_chat: InlineChatState {
                visible: false,
                query: String::new(),
                is_streaming: false,
                streaming_text: String::new(),
                just_opened: false,
            },
            inline_event_rx: Some(inline_rx),
            inline_event_tx: inline_tx,

            show_explorer: ide_state.show_explorer,
            show_chat: ide_state.show_chat,
            show_browser: false,
            show_terminal: ide_state.show_terminal,
            show_about: false,
            left_tab: LeftPanelTab::Explorer,

            show_palette: false,
            palette_query: String::new(),
            palette_selected: 0,
            palette_files: Vec::new(),
            palette_just_opened: false,

            theme_preset,
            theme,

            explorer_width: ide_state.explorer_width,
            chat_width: ide_state.chat_width,
            terminal_height: ide_state.terminal_height,

            event_rx: Some(event_rx),
            event_tx,
            agent_cancel: None,

            pending_approval: None,
            approval_state: Arc::new(Mutex::new(ApprovalState { pending: None  })),

            runtime,

            settings,

            file_mod_times: HashMap::new(),
            last_file_check: std::time::Instant::now(),
            file_watcher: None,
            file_watch_rx: None,
            last_browser_url: String::new(),
            agent_conversation: Arc::new(tokio::sync::Mutex::new({
                let mut conv = ConversationHistory::new();
                conv.set_system_prompt(
                    "You are PhazeAI, an AI coding assistant. Help the user with their code."
                );
                conv
            })),
            status_notification: None,
            last_state_save: std::time::Instant::now(),

            lsp_manager: None,
            lsp_event_rx: None,

            agent_history: Vec::new(),
            show_agent_history: false,

            split_editor: None,
            split_ratio: 0.5,
        }
    }

    /// Check open editor files for disk modifications (e.g., from agent edits)
    fn save_state_periodically(&mut self) {
        if self.last_state_save.elapsed().as_secs() < 30 {
            return;
        }
        self.last_state_save = std::time::Instant::now();
        let state = IdeState {
            last_workspace: self.explorer.root().cloned(),
            explorer_width: self.explorer_width,
            chat_width: self.chat_width,
            terminal_height: self.terminal_height,
            show_explorer: self.show_explorer,
            show_chat: self.show_chat,
            show_terminal: self.show_terminal,
            ai_mode: self.chat.mode.label().to_string(),
        };
        state.save();
    }

    fn check_file_changes(&mut self) {
        // Initialize notify watcher once
        if self.file_watcher.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
            let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
            });
            if let Ok(w) = watcher {
                self.file_watcher = Some(w);
                self.file_watch_rx = Some(rx);
                // Watch all currently open files
                let paths: Vec<PathBuf> = self.editor.tabs.iter()
                    .filter_map(|t| t.path.clone())
                    .collect();
                for path in paths {
                    self.watch_file(&path);
                }
            }
        }

        // Drain notify events
        let mut changed_paths: Vec<PathBuf> = Vec::new();
        if let Some(ref rx) = self.file_watch_rx {
            while let Ok(path) = rx.try_recv() {
                changed_paths.push(path);
            }
        }

        // Fallback: also check every 5s for files not caught by notify
        let should_poll = self.last_file_check.elapsed().as_secs() >= 5;
        if should_poll {
            self.last_file_check = std::time::Instant::now();
            for tab in &self.editor.tabs {
                if let Some(ref path) = tab.path {
                    if let Ok(metadata) = std::fs::metadata(path) {
                        if let Ok(modified) = metadata.modified() {
                            let prev = self.file_mod_times.get(path).cloned();
                            if let Some(prev_time) = prev {
                                if modified > prev_time {
                                    changed_paths.push(path.clone());
                                }
                            }
                            self.file_mod_times.insert(path.clone(), modified);
                        }
                    }
                }
            }
        }

        // Reload changed tabs
        for changed in &changed_paths {
            for tab in &mut self.editor.tabs {
                if tab.path.as_ref() == Some(changed) && !tab.modified {
                    tracing::info!("Reloading changed file: {}", changed.display());
                    tab.reload_from_disk();
                    // Invalidate git hunks so they refresh after external edit
                    tab.last_git_check = None;
                }
            }
        }
    }

    fn watch_file(&mut self, path: &PathBuf) {
        if let Some(ref mut watcher) = self.file_watcher {
            let _ = watcher.watch(path.as_path(), RecursiveMode::NonRecursive);
        }
    }

    // ── LSP Integration ────────────────────────────────────────────────────

    /// Initialize the LSP manager for a workspace root (called when folder is opened).
    fn init_lsp(&mut self, workspace_root: PathBuf) {
        let (lsp_tx, lsp_rx) = mpsc::unbounded_channel::<LspEvent>();
        let manager = LspManager::new(workspace_root, lsp_tx);
        self.lsp_manager = Some(manager);
        self.lsp_event_rx = Some(lsp_rx);
    }

    /// Notify LSP when a file is opened.
    fn lsp_did_open(&mut self, path: &PathBuf, text: &str) {
        if let Some(ref manager) = self.lsp_manager {
            manager.did_open(path, text);
        }
    }

    /// Notify LSP when a file changes.
    fn lsp_did_change(&mut self, path: &PathBuf, version: i32, text: &str) {
        if let Some(ref manager) = self.lsp_manager {
            manager.did_change(path, version, text);
        }
    }

    /// Process pending LSP events (diagnostics, etc.)
    fn process_lsp_events(&mut self) {
        if let Some(ref mut rx) = self.lsp_event_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    LspEvent::Diagnostics { uri, diagnostics } => {
                        // Convert file:// URI to local path
                        let uri_str = uri.to_string();
                        let path_opt = uri_str.strip_prefix("file://")
                            .map(|s| PathBuf::from(percent_decode_uri(s)));

                        if let Some(diag_path) = path_opt {
                            for tab in &mut self.editor.tabs {
                                if tab.path.as_deref() == Some(&diag_path) {
                                    tab.diagnostics.clear();
                                    for diag in &diagnostics {
                                        let line_idx = diag.range.start.line as usize;
                                        tab.diagnostics.entry(line_idx).or_default().push(diag.clone());
                                    }
                                }
                            }
                        }
                    }
                    LspEvent::Hover(hover_opt) => {
                        let text = hover_opt.and_then(|h| {
                            match h.contents {
                                lsp_types::HoverContents::Scalar(markup) => {
                                    Some(match markup {
                                        lsp_types::MarkedString::String(s) => s,
                                        lsp_types::MarkedString::LanguageString(ls) => ls.value,
                                    })
                                }
                                lsp_types::HoverContents::Array(arr) => {
                                    let parts: Vec<String> = arr.into_iter().map(|ms| match ms {
                                        lsp_types::MarkedString::String(s) => s,
                                        lsp_types::MarkedString::LanguageString(ls) => ls.value,
                                    }).collect();
                                    Some(parts.join("\n"))
                                }
                                lsp_types::HoverContents::Markup(mc) => Some(mc.value),
                            }
                        });
                        self.editor.set_hover_result(text);
                    }
                    LspEvent::Definition(locations) => {
                        if let Some(loc) = locations.into_iter().next() {
                            let uri_str = loc.uri.to_string();
                            let path_opt = uri_str.strip_prefix("file://")
                                .map(|s| PathBuf::from(percent_decode_uri(s)));
                            if let Some(path) = path_opt {
                                let line = loc.range.start.line as usize;
                                self.editor.open_file_at_line(path, line);
                            }
                        }
                    }
                    LspEvent::Completions(items) => {
                        self.editor.set_completion_result(items);
                    }
                    LspEvent::References(locations) => {
                        // Show references as a notification with count; future: show in search panel
                        let count = locations.len();
                        self.status_notification = Some((
                            format!("Found {count} references"),
                            true,
                            std::time::Instant::now(),
                        ));
                        // Open the first reference if any (same as goto-def for now)
                        if let Some(loc) = locations.into_iter().next() {
                            let uri_str = loc.uri.to_string();
                            let path_opt = uri_str.strip_prefix("file://")
                                .map(|s| PathBuf::from(percent_decode_uri(s)));
                            if let Some(path) = path_opt {
                                let line = loc.range.start.line as usize;
                                self.editor.open_file_at_line(path, line);
                            }
                        }
                    }
                    LspEvent::Formatting(edits) => {
                        if let Some(tab) = self.editor.tabs.get_mut(self.editor.active_tab) {
                            let mut sorted = edits;
                            // Reverse order so earlier edits don't shift later offsets
                            sorted.sort_by(|a, b| {
                                b.range.start.line.cmp(&a.range.start.line)
                                    .then(b.range.start.character.cmp(&a.range.start.character))
                            });
                            tab.apply_lsp_edits(&sorted);
                        }
                        self.status_notification = Some((
                            "Document formatted".into(),
                            true,
                            std::time::Instant::now(),
                        ));
                    }
                    LspEvent::Initialized(server) => {
                        tracing::info!("LSP server initialized: {}", server);
                        self.status_notification = Some((
                            format!("LSP: {server} ready"),
                            true,
                            std::time::Instant::now(),
                        ));
                    }
                    LspEvent::Shutdown => {
                        tracing::info!("LSP server shut down");
                    }
                    LspEvent::Log(msg) => {
                        tracing::debug!("LSP log: {}", msg);
                    }
                }
            }
        }
    }

    /// Check if the active tab needs a hover request and dispatch it.
    fn lsp_poll_hover(&mut self) {
        if let Some((path, line, col)) = self.editor.lsp_hover_request() {
            if let Some(ref manager) = self.lsp_manager {
                if let Some(client) = manager.client_for_path(&path).cloned() {
                    self.runtime.spawn(async move {
                        match client.hover(&path, line, col).await {
                            Ok(result) => {
                                if let Err(e) = client.send_hover_event(result) {
                                    tracing::debug!("Hover send error: {e}");
                                }
                            }
                            Err(e) => tracing::debug!("LSP hover error: {e}"),
                        }
                    });
                }
            }
        }
    }

    /// Dispatch an LSP go-to-definition request for the current cursor position.
    fn lsp_goto_definition(&mut self) {
        if let Some((path, line, col)) = self.editor.lsp_goto_def_request() {
            if let Some(ref manager) = self.lsp_manager {
                if let Some(client) = manager.client_for_path(&path).cloned() {
                    self.runtime.spawn(async move {
                        match client.goto_definition(&path, line, col).await {
                            Ok(locs) => {
                                if let Err(e) = client.send_definition_event(locs) {
                                    tracing::debug!("Definition send error: {e}");
                                }
                            }
                            Err(e) => tracing::debug!("LSP goto_def error: {e}"),
                        }
                    });
                }
            }
        }
    }

    /// Dispatch an LSP completion request.
    fn lsp_trigger_completion(&mut self) {
        if let Some((path, line, col)) = self.editor.lsp_completion_request() {
            if let Some(ref manager) = self.lsp_manager {
                if let Some(client) = manager.client_for_path(&path).cloned() {
                    self.runtime.spawn(async move {
                        match client.completion(&path, line, col).await {
                            Ok(items) => {
                                if let Err(e) = client.send_completions_event(items) {
                                    tracing::debug!("Completion send error: {e}");
                                }
                            }
                            Err(e) => tracing::debug!("LSP completion error: {e}"),
                        }
                    });
                }
            }
        }
    }

    /// Dispatch an LSP find-references request.
    fn lsp_find_references(&mut self) {
        if let Some((path, line, col)) = self.editor.lsp_goto_def_request() {
            if let Some(ref manager) = self.lsp_manager {
                if let Some(client) = manager.client_for_path(&path).cloned() {
                    self.runtime.spawn(async move {
                        match client.find_references(&path, line, col).await {
                            Ok(locs) => {
                                if let Err(e) = client.send_references_event(locs) {
                                    tracing::debug!("References send error: {e}");
                                }
                            }
                            Err(e) => tracing::debug!("LSP find_references error: {e}"),
                        }
                    });
                }
            }
        }
    }

    /// Dispatch an LSP document formatting request and apply the result.
    fn lsp_format_document(&mut self) {
        if let Some((path, _, _)) = self.editor.lsp_goto_def_request() {
            if let Some(ref manager) = self.lsp_manager {
                if let Some(client) = manager.client_for_path(&path).cloned() {
                    self.runtime.spawn(async move {
                        match client.formatting(&path, 4, true).await {
                            Ok(edits) => {
                                if let Err(e) = client.send_formatting_event(edits) {
                                    tracing::debug!("Formatting send error: {e}");
                                }
                            }
                            Err(e) => tracing::debug!("LSP formatting error: {e}"),
                        }
                    });
                }
            }
        }
    }

    fn handle_keybindings(&mut self, ctx: &egui::Context) {
        // Handle approval UI keyboard shortcuts first
        if self.pending_approval.is_some() {
            ctx.input(|i| {
                if i.key_pressed(egui::Key::Y) {
                    self.handle_approval_response(true);
                } else if i.key_pressed(egui::Key::N) {
                    self.handle_approval_response(false);
                }
            });
        }

        if let Some(action) = keybindings::check_keybindings(ctx) {
            match action {
                Action::Save => self.editor.save_active(),
                Action::Open => self.open_file_dialog(),
                Action::NewFile => self.editor.new_tab(),
                Action::CloseTab => {
                    let idx = self.editor.active_tab;
                    self.editor.close_tab(idx);
                }
                Action::ToggleExplorer => self.show_explorer = !self.show_explorer,
                Action::ToggleChat => self.show_chat = !self.show_chat,
                Action::ToggleTerminal => self.show_terminal = !self.show_terminal,
                Action::FocusChat => {
                    self.show_chat = true;
                    // Focus will be handled in chat panel
                }
                Action::CommandPalette => {
                    self.open_command_palette();
                }
                Action::Undo => self.editor.undo(),
                Action::Redo => self.editor.redo(),
                Action::Find => self.editor.toggle_find(),
                Action::ToggleBrowser => self.show_browser = !self.show_browser,
                Action::SelectAll => {
                    if let Some(tab) = self.editor.tabs.get_mut(self.editor.active_tab) {
                        tab.select_all();
                    }
                }
                Action::Copy => {
                    if let Some(tab) = self.editor.tabs.get_mut(self.editor.active_tab) {
                        tab.copy_to_clipboard();
                    }
                }
                Action::Cut => {
                    if let Some(tab) = self.editor.tabs.get_mut(self.editor.active_tab) {
                        tab.cut_to_clipboard();
                    }
                }
                Action::Paste => {
                    if let Some(tab) = self.editor.tabs.get_mut(self.editor.active_tab) {
                        tab.paste_from_clipboard();
                    }
                }
                Action::ToggleSearch => {
                    self.show_explorer = true;
                    self.left_tab = if self.left_tab == LeftPanelTab::Search {
                        LeftPanelTab::Explorer
                    } else {
                        LeftPanelTab::Search
                    };
                }
                Action::InlineChat => {
                    self.inline_chat.visible = !self.inline_chat.visible;
                    if self.inline_chat.visible {
                        self.inline_chat.query.clear();
                        self.inline_chat.streaming_text.clear();
                        self.inline_chat.just_opened = true;
                    }
                }
                Action::SelectNextOccurrence => {
                    if let Some(tab) = self.editor.tabs.get_mut(self.editor.active_tab) {
                        tab.select_next_occurrence();
                    }
                }
                Action::FindReplace => {
                    if !self.editor.find_replace.visible {
                        self.editor.find_replace.visible = true;
                    } else {
                        self.editor.find_replace.visible = false;
                        self.editor.find_replace.matches.clear();
                    }
                }
                Action::ToggleGit => {
                    self.show_explorer = true;
                    self.left_tab = if self.left_tab == LeftPanelTab::Git {
                        LeftPanelTab::Explorer
                    } else {
                        LeftPanelTab::Git
                    };
                    if self.left_tab == LeftPanelTab::Git {
                        self.diff.refresh();
                    }
                }
                Action::GotoDefinition => {
                    self.lsp_goto_definition();
                }
                Action::Completion => {
                    self.lsp_trigger_completion();
                }
                Action::FindReferences => {
                    self.lsp_find_references();
                }
                Action::FormatDocument => {
                    self.lsp_format_document();
                }
            }
        }
    }

    fn open_file_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            self.watch_file(&path.clone());
            self.editor.open_file(path);
        }
    }

    fn process_agent_events(&mut self) {
        if let Some(ref mut rx) = self.event_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    AgentEvent::Thinking { .. } => {
                        self.chat.start_streaming();
                    }
                    AgentEvent::TextDelta(text) => {
                        self.chat.append_streaming_text(&text);
                    }
                    AgentEvent::ToolApprovalRequest { .. } => {
                        // Approval handled via approval_state callback
                    }
                    AgentEvent::ToolStart { name } => {
                        self.chat.add_tool_call(&name, true, "Running...");
                        self.status_notification = Some((
                            format!("Running: {name}"),
                            true,
                            std::time::Instant::now(),
                        ));
                    }
                    AgentEvent::ToolResult { name, success, summary } => {
                        self.chat.add_tool_call(&name, success, &summary);
                        self.status_notification = Some((
                            format!("{}: {}", name, if success { "done" } else { "failed" }),
                            success,
                            std::time::Instant::now(),
                        ));

                        // Signal to force-reload on next frame when agent edits a file
                        if matches!(name.as_str(), "write_file" | "edit_file") && success {
                            self.last_file_check = std::time::Instant::now()
                                .checked_sub(std::time::Duration::from_secs(10))
                                .unwrap_or(std::time::Instant::now());
                        }

                        // Stream bash output into terminal panel
                        if name == "bash" && !summary.is_empty() {
                            self.terminal.inject_output(&summary);
                            self.show_terminal = true;
                            self.terminal.scroll_to_bottom = true;
                        }
                    }
                    AgentEvent::Complete { .. } => {
                        self.chat.finish_streaming();
                        self.status_notification = Some((
                            "Agent complete".to_string(),
                            true,
                            std::time::Instant::now(),
                        ));
                        // Record in agent history
                        let summary = self.chat.messages.iter().rev()
                            .find(|m| matches!(m.role, crate::panels::chat::ChatMessageRole::User))
                            .map(|m| {
                                let s = &m.content;
                                if s.len() > 80 { format!("{}…", &s[..80]) } else { s.clone() }
                            })
                            .unwrap_or_else(|| "Agent run".to_string());
                        let tool_count = self.chat.messages.iter()
                            .map(|m| m.tool_calls.len()).sum();
                        self.agent_history.push(AgentRun {
                            timestamp: chrono::Local::now().format("%H:%M").to_string(),
                            mode_label: self.chat.mode.label().to_string(),
                            summary,
                            tool_count,
                        });
                        if self.agent_history.len() > 20 {
                            self.agent_history.remove(0);
                        }
                    }
                    AgentEvent::Error(err) => {
                        self.chat.finish_streaming();
                        self.chat.add_assistant_message(&format!("Error: {}", err));
                        self.status_notification = Some((
                            format!("Error: {err}"),
                            false,
                            std::time::Instant::now(),
                        ));
                    }
                    AgentEvent::BrowserFetchStart { url } => {
                        if self.browser.url == url {
                            self.browser.loading = true;
                        }
                    }
                    AgentEvent::BrowserFetchComplete { url, title, content } => {
                        self.browser.set_content(url, title, content);
                    }
                    AgentEvent::BrowserFetchError { url, error } => {
                        self.browser.set_error(url, error);
                    }
                }
            }
        }
    }

    fn process_browser_requests(&mut self) {
        let url = self.browser.url.clone();
        if url == self.last_browser_url {
            return;
        }

        self.last_browser_url = url.clone();
        let tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            let _ = tx.send(AgentEvent::BrowserFetchStart { url: url.clone() });
            
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .user_agent("PhazeAI/0.1.0")
                .build()
                .unwrap_or_default();

            match client.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        match resp.text().await {
                            Ok(text) => {
                                let title = extract_title(&text).unwrap_or_else(|| url.clone());
                                let content = clean_html_to_markdown(&text);
                                let _ = tx.send(AgentEvent::BrowserFetchComplete { url, title, content });
                            }
                            Err(e) => {
                                let _ = tx.send(AgentEvent::BrowserFetchError { url, error: e.to_string() });
                            }
                        }
                    } else {
                        let _ = tx.send(AgentEvent::BrowserFetchError { url, error: format!("HTTP {}", status) });
                    }
                }
                Err(e) => {
                    let _ = tx.send(AgentEvent::BrowserFetchError { url, error: e.to_string() });
                }
            }
        });
    }

    fn process_chat_messages(&mut self) {
        if let Some((raw_msg, mode)) = self.chat.pending_send.take() {
            // ── Collect context based on AI mode ──────────────────────
            let augmented_msg = self.build_context_message(&raw_msg, &mode);
            let system_prompt = mode.system_prompt().to_string();

            // Update shared conversation system prompt to match current mode
            if let Ok(mut c) = self.agent_conversation.try_lock() {
                c.set_system_prompt(system_prompt.clone());
                // Smart context pruning: keep under ~80k tokens to avoid hitting limits
                const MAX_CONTEXT_TOKENS: usize = 80_000;
                if c.estimate_tokens() > MAX_CONTEXT_TOKENS {
                    c.trim_to_token_budget(MAX_CONTEXT_TOKENS);
                    tracing::info!("Context pruned to ~{} tokens", c.estimate_tokens());
                }
            }

            // ── Select tool registry based on AI mode ─────────────────
            let tool_registry = match mode {
                AiMode::Ask => phazeai_core::ToolRegistry::read_only(),
                AiMode::Debug | AiMode::Plan => phazeai_core::ToolRegistry::standard(),
                AiMode::Chat | AiMode::Edit => phazeai_core::ToolRegistry::default(),
            };

            let tx = self.event_tx.clone();
            let settings = self.settings.clone();
            let approval_state = self.approval_state.clone();
            let conversation = self.agent_conversation.clone();

            // Create cancellation channel
            let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
            self.agent_cancel = Some(cancel_tx);

            self.runtime.spawn(async move {
                match settings.build_llm_client() {
                    Ok(llm) => {
                        let approval_fn: phazeai_core::ApprovalFn = Box::new(move |tool_name, params| {
                            let approval_state = approval_state.clone();
                            Box::pin(async move {
                                let params_summary = if params.is_null() {
                                    String::new()
                                } else {
                                    match serde_json::to_string_pretty(&params) {
                                        Ok(s) => {
                                            if s.chars().count() > 300 {
                                                let t: String = s.chars().take(300).collect();
                                                format!("{t}...")
                                            } else { s }
                                        }
                                        Err(_) => format!("{:?}", params),
                                    }
                                };

                                let (response_tx, response_rx) = oneshot::channel();
                                {
                                    let mut state = approval_state.lock().unwrap();
                                    state.pending = Some((
                                        tool_name,
                                        params_summary,
                                        params,
                                        response_tx,
                                    ));
                                }

                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(300),
                                    response_rx
                                ).await {
                                    Ok(Ok(approved)) => approved,
                                    _ => false,
                                }
                            })
                        });

                        let agent = phazeai_core::Agent::new(llm)
                            .with_tools(tool_registry)
                            .with_shared_conversation(conversation)
                            .with_approval(approval_fn);

                        tokio::select! {
                            result = agent.run_with_events(&augmented_msg, tx.clone()) => {
                                if let Err(e) = result {
                                    let _ = tx.send(AgentEvent::Error(e.to_string()));
                                }
                            }
                            _ = &mut cancel_rx => {
                                let _ = tx.send(AgentEvent::Error("Cancelled by user.".into()));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(AgentEvent::Error(format!("LLM client error: {e}")));
                    }
                }
            });
        }
    }

    /// Build the context-augmented message for the given AI mode.
    /// Resolve `@filename` mentions in a message. When the user writes `@path/to/file`,
    /// the file's content is appended as context.
    fn resolve_at_mentions(&self, user_msg: &str) -> String {
        let workspace_root = self.explorer.root().cloned()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        let mut extras = Vec::new();
        // Simple manual scan: find `@` followed by non-whitespace chars
        let mut remaining = user_msg;
        while let Some(at_pos) = remaining.find('@') {
            let after = &remaining[at_pos + 1..];
            let end = after.find(|c: char| c.is_whitespace() || c == ',' || c == ';')
                .unwrap_or(after.len());
            if end == 0 {
                remaining = &remaining[at_pos + 1..];
                continue;
            }
            let mention = &after[..end];
            // Only try path-like mentions (contains . or /)
            if mention.contains('.') || mention.contains('/') {
                let path = if std::path::Path::new(mention).is_absolute() {
                    PathBuf::from(mention)
                } else {
                    workspace_root.join(mention)
                };
                if path.is_file() && !extras.iter().any(|(m, _): &(String, String)| m == mention) {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let lang: String = path.extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("")
                            .to_string();
                        extras.push((mention.to_string(), format!(
                            "**@{mention}:**\n```{lang}\n{content}\n```"
                        )));
                    }
                }
            }
            remaining = &remaining[at_pos + 1..];
        }

        if extras.is_empty() {
            return user_msg.to_string();
        }
        let attachment_block = extras.into_iter().map(|(_, b)| b).collect::<Vec<_>>().join("\n\n");
        format!("{}\n\n---\n{}", user_msg, attachment_block)
    }

    fn build_context_message(&self, user_msg: &str, mode: &AiMode) -> String {
        // Resolve @filename mentions first
        let user_msg_expanded = self.resolve_at_mentions(user_msg);
        let user_msg = user_msg_expanded.as_str();
        match mode {
            AiMode::Chat => {
                // If the user toggled "include file", inject it even in Chat mode
                if self.chat.include_current_file {
                    let file_ctx = self.current_file_context();
                    if !file_ctx.is_empty() {
                        return format!("{}\n\n---\n{}", user_msg, file_ctx);
                    }
                }
                user_msg.to_string()
            }

            AiMode::Ask => {
                let file_ctx = self.current_file_context();
                if file_ctx.is_empty() {
                    user_msg.to_string()
                } else {
                    format!("{}\n\n---\n{}", user_msg, file_ctx)
                }
            }

            AiMode::Debug => {
                let file_ctx = self.current_file_context();
                let term_out = self.terminal.recent_output(50);
                let mut ctx = user_msg.to_string();
                if !file_ctx.is_empty() {
                    ctx.push_str(&format!("\n\n---\n{}", file_ctx));
                }
                if !term_out.trim().is_empty() {
                    ctx.push_str(&format!(
                        "\n\n---\n**Recent terminal output:**\n```\n{}\n```",
                        term_out
                    ));
                }
                ctx
            }

            AiMode::Plan => {
                let proj_ctx = self.project_structure_context();
                let file_ctx = self.current_file_context();
                let mut ctx = user_msg.to_string();
                if !proj_ctx.is_empty() {
                    ctx.push_str(&format!("\n\n---\n**Project structure:**\n```\n{}\n```", proj_ctx));
                }
                if !file_ctx.is_empty() {
                    ctx.push_str(&format!("\n\n---\n{}", file_ctx));
                }
                ctx
            }

            AiMode::Edit => {
                let file_ctx = self.current_file_context();
                if file_ctx.is_empty() {
                    user_msg.to_string()
                } else {
                    format!("{}\n\n---\n{}", user_msg, file_ctx)
                }
            }
        }
    }

    /// Get the active file's content as a markdown code block with path header.
    fn current_file_context(&self) -> String {
        let tab = match self.editor.tabs.get(self.editor.active_tab) {
            Some(t) => t,
            None => return String::new(),
        };

        let path_str = tab.path.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Untitled".to_string());

        let lang = if tab.language.is_empty() { "" } else { tab.language.as_str() };

        // Get selected text or full file (capped at 8000 chars to avoid blowing context)
        let content = if let Some(ref sel) = tab.selection {
            if !sel.is_empty() {
                let (start, end) = sel.ordered();
                let rope = &tab.rope;
                let start_char = rope.line_to_char(start.line) + start.col;
                let end_char = rope.line_to_char(end.line) + end.col;
                rope.slice(start_char..end_char).to_string()
            } else {
                let full = tab.rope.to_string();
                if full.len() > 8000 { full[..8000].to_string() + "\n... (truncated)" } else { full }
            }
        } else {
            let full = tab.rope.to_string();
            if full.len() > 8000 { full[..8000].to_string() + "\n... (truncated)" } else { full }
        };

        if content.trim().is_empty() {
            return String::new();
        }

        format!("**Current file:** `{}`\n```{}\n{}\n```", path_str, lang, content)
    }

    /// Get a compact project structure listing.
    fn project_structure_context(&self) -> String {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut lines = Vec::new();
        collect_tree(&cwd, &mut lines, 0, 3);
        lines.join("\n")
    }

    fn check_settings_changes(&mut self) {
        if self.settings_panel.settings_changed {
            self.settings_panel.settings_changed = false;

            // Update theme if changed
            let new_preset = self.settings_panel.theme_preset.clone();
            if new_preset != self.theme_preset {
                self.theme_preset = new_preset;
                self.theme = ThemeColors::from_preset(&self.theme_preset);
            }

            // Update editor font size
            self.editor.font_size = self.settings_panel.settings.editor.font_size;
            self.editor.show_line_numbers = self.settings_panel.settings.editor.show_line_numbers;

            // Update settings reference
            self.settings = self.settings_panel.settings.clone();
        }
    }

    fn check_explorer_file_open(&mut self) {
        if let Some(path) = self.explorer.file_to_open.take() {
            self.editor.open_file(path);
            // LSP notification happens in drain_file_loads() once the file is loaded.
        }
    }

    fn check_search_file_open(&mut self) {
        if let Some((path, line)) = self.search.file_to_open.take() {
            self.editor.open_file_at_line(path, line);
        }
    }

    /// Drain the async file-load channel. Notify LSP of each newly opened file.
    fn drain_file_loads(&mut self) {
        let opened = self.editor.drain_file_loads();
        for path in opened {
            if let Some(tab) = self.editor.tabs.iter().find(|t| t.path.as_ref() == Some(&path)) {
                let text = tab.rope.to_string();
                self.lsp_did_open(&path, &text);
            }
        }
    }

    fn handle_approval_response(&mut self, approved: bool) {
        self.pending_approval = None;
        if let Ok(mut state) = self.approval_state.lock() {
            if let Some((_, _, _, tx)) = state.pending.take() {
                let _ = tx.send(approved);
            }
        }
    }

    fn check_approval_requests(&mut self) {
        if self.pending_approval.is_some() {
            return; // already showing one
        }
        let extracted = {
            if let Ok(state) = self.approval_state.lock() {
                state.pending.as_ref().map(|(name, summary, params, _)| {
                    (name.clone(), summary.clone(), params.clone())
                })
            } else {
                None
            }
        };
        if let Some((tool_name, params_summary, full_params)) = extracted {
            // For file edit tools, compute diff + structured hunks
            let (diff, hunks, old_content, new_content, file_path) =
                compute_diff_data(&tool_name, &full_params);
            self.pending_approval = Some(PendingApproval {
                tool_name,
                params_summary,
                diff,
                hunks,
                old_content,
                new_content,
                file_path,
                full_params,
            });
        }
    }

    fn open_command_palette(&mut self) {
        self.show_palette = true;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.palette_just_opened = true;

        // Collect project files for the palette
        self.palette_files.clear();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        collect_project_files(&cwd, &mut self.palette_files, 0);
        self.palette_files.sort();
    }

    fn palette_get_filtered(&self) -> Vec<(String, PaletteAction)> {
        let query = self.palette_query.to_lowercase();
        let mut results: Vec<(String, PaletteAction)> = Vec::new();

        // Commands (shown when query starts with '>' or is empty)
        let show_commands = query.starts_with('>') || query.is_empty();
        if show_commands {
            let cmd_query = query.strip_prefix('>').unwrap_or("").trim();
            let commands: Vec<(&str, PaletteAction)> = vec![
                ("New File", PaletteAction::NewFile),
                ("Open File", PaletteAction::OpenFileDialog),
                ("Save", PaletteAction::Save),
                ("Toggle Explorer", PaletteAction::ToggleExplorer),
                ("Toggle Chat", PaletteAction::ToggleChat),
                ("Toggle Terminal", PaletteAction::ToggleTerminal),
                ("Search in Workspace", PaletteAction::ToggleSearch),
                ("Inline Chat (Ctrl+K)", PaletteAction::ToggleInlineChat),
                ("Settings", PaletteAction::OpenSettings),
                // AI mode switching
                ("Mode: Chat", PaletteAction::SwitchMode(AiMode::Chat)),
                ("Mode: Ask (current file)", PaletteAction::SwitchMode(AiMode::Ask)),
                ("Mode: Debug (file + terminal)", PaletteAction::SwitchMode(AiMode::Debug)),
                ("Mode: Plan (project context)", PaletteAction::SwitchMode(AiMode::Plan)),
                ("Mode: Edit (AI edits files)", PaletteAction::SwitchMode(AiMode::Edit)),
            ];
            for (label, action) in commands {
                if cmd_query.is_empty() || label.to_lowercase().contains(cmd_query) {
                    results.push((format!("> {}", label), action));
                }
            }
        }

        // Files (shown when query doesn't start with '>')
        if !query.starts_with('>') {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            for path in &self.palette_files {
                let display = path
                    .strip_prefix(&cwd)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                if query.is_empty() || fuzzy_match(&display.to_lowercase(), &query) {
                    results.push((display, PaletteAction::OpenFile(path.clone())));
                }
                if results.len() >= 50 {
                    break;
                }
            }
        }

        results
    }

    fn execute_palette_action(&mut self, action: PaletteAction) {
        match action {
            PaletteAction::OpenFile(path) => self.editor.open_file(path),
            PaletteAction::NewFile => self.editor.new_tab(),
            PaletteAction::OpenFileDialog => self.open_file_dialog(),
            PaletteAction::Save => self.editor.save_active(),
            PaletteAction::ToggleExplorer => self.show_explorer = !self.show_explorer,
            PaletteAction::ToggleChat => self.show_chat = !self.show_chat,
            PaletteAction::ToggleTerminal => self.show_terminal = !self.show_terminal,
            PaletteAction::ToggleSearch => {
                self.show_explorer = true;
                self.left_tab = LeftPanelTab::Search;
            }
            PaletteAction::OpenSettings => self.settings_panel.toggle(),
            PaletteAction::SwitchMode(mode) => {
                self.chat.mode = mode;
                self.show_chat = true;
            }
            PaletteAction::ToggleInlineChat => {
                self.inline_chat.visible = !self.inline_chat.visible;
                if self.inline_chat.visible {
                    self.inline_chat.query.clear();
                    self.inline_chat.just_opened = true;
                }
            }
        }
    }

    fn render_command_palette(&mut self, ctx: &egui::Context) {
        // Handle Escape to close
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_palette = false;
            return;
        }

        let filtered = self.palette_get_filtered();

        // Handle Up/Down/Enter
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.palette_selected = self.palette_selected.saturating_sub(1);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !filtered.is_empty() {
            self.palette_selected = (self.palette_selected + 1).min(filtered.len() - 1);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !filtered.is_empty() {
            let idx = self.palette_selected.min(filtered.len() - 1);
            let action = filtered[idx].1.clone();
            self.show_palette = false;
            self.execute_palette_action(action);
            return;
        }

        // Render the palette as a centered window
        egui::Area::new(egui::Id::new("palette_overlay"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                // Semi-transparent backdrop
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    egui::Color32::from_black_alpha(100),
                );
            });

        egui::Window::new("Command Palette")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .fixed_size([500.0, 350.0])
            .show(ctx, |ui| {
                // Search input
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .hint_text("Type to search files, '>' for commands...")
                        .desired_width(f32::INFINITY),
                );

                // Focus the text field when palette just opened
                if self.palette_just_opened {
                    response.request_focus();
                    self.palette_just_opened = false;
                }

                // Reset selection when query changes
                if response.changed() {
                    self.palette_selected = 0;
                }

                ui.separator();

                // Results list
                let filtered = self.palette_get_filtered();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, (label, action)) in filtered.iter().enumerate() {
                            let is_selected = i == self.palette_selected;
                            let response = ui.selectable_label(is_selected, label);
                            if response.clicked() {
                                let action = action.clone();
                                self.show_palette = false;
                                self.execute_palette_action(action);
                                return;
                            }
                            if is_selected {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                        }

                        if filtered.is_empty() {
                            ui.colored_label(
                                self.theme.text_muted,
                                "No results found",
                            );
                        }
                    });
            });
    }

    fn process_chat_apply(&mut self) {
        if let Some(code) = self.chat.pending_apply.take() {
            if let Some(tab) = self.editor.tabs.get_mut(self.editor.active_tab) {
                // Replace selection if any, otherwise append at cursor
                if let Some(ref sel) = tab.selection.clone() {
                    if !sel.is_empty() {
                        let (start, end) = sel.ordered();
                        let start_char = tab.rope.line_to_char(start.line) + start.col;
                        let end_char = tab.rope.line_to_char(end.line) + end.col;
                        tab.rope.remove(start_char..end_char);
                        tab.rope.insert(start_char, &code);
                        tab.cursor = start;
                        tab.selection = None;
                        tab.modified = true;
                        return;
                    }
                }
                // No selection: insert at cursor position
                let cursor_char = tab.rope.line_to_char(tab.cursor.line) + tab.cursor.col;
                tab.rope.insert(cursor_char, &code);
                tab.modified = true;
                self.status_notification = Some((
                    "Code applied to editor".to_string(),
                    true,
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn process_chat_cancellation(&mut self) {
        if self.chat.pending_cancel {
            self.chat.pending_cancel = false;
            if let Some(cancel_tx) = self.agent_cancel.take() {
                let _ = cancel_tx.send(());
            }
            self.chat.finish_streaming();
            self.chat.add_assistant_message("*[Cancelled by user]*");
        }
    }

    fn process_inline_events(&mut self) {
        if let Some(ref mut rx) = self.inline_event_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    AgentEvent::Thinking { .. } => {
                        self.inline_chat.is_streaming = true;
                        self.inline_chat.streaming_text.clear();
                    }
                    AgentEvent::TextDelta(text) => {
                        self.inline_chat.streaming_text.push_str(&text);
                    }
                    AgentEvent::Complete { .. } | AgentEvent::Error(_) => {
                        self.inline_chat.is_streaming = false;
                    }
                    _ => {}
                }
            }
        }
    }

    fn render_inline_chat(&mut self, ctx: &egui::Context) {
        if !self.inline_chat.visible { return; }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.inline_chat.visible = false;
            return;
        }

        let theme = self.theme.clone();
        let mut send_query: Option<String> = None;

        egui::Window::new("Inline Chat  (Ctrl+K)")
            .title_bar(true)
            .resizable(true)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -120.0])
            .default_size([500.0, 320.0])
            .show(ctx, |ui| {
                // Query input
                ui.horizontal(|ui| {
                    let hint = "Describe what to do with the current file...";
                    let edit = egui::TextEdit::multiline(&mut self.inline_chat.query)
                        .desired_width(ui.available_width() - 70.0)
                        .desired_rows(2)
                        .font(egui::FontId::monospace(13.0))
                        .hint_text(hint);

                    let resp = ui.add(edit);

                    if self.inline_chat.just_opened {
                        resp.request_focus();
                        self.inline_chat.just_opened = false;
                    }

                    let enter = resp.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

                    let send_btn = ui.add_enabled(
                        !self.inline_chat.is_streaming && !self.inline_chat.query.trim().is_empty(),
                        egui::Button::new("Send"),
                    );

                    if (send_btn.clicked() || enter)
                        && !self.inline_chat.query.trim().is_empty()
                        && !self.inline_chat.is_streaming
                    {
                        send_query = Some(self.inline_chat.query.trim().to_string());
                    }
                });

                ui.separator();

                // Response area
                let response_height = if self.inline_chat.streaming_text.is_empty() { 60.0 } else { 200.0 };
                egui::ScrollArea::vertical()
                    .max_height(response_height)
                    .auto_shrink([false, false])
                    .stick_to_bottom(self.inline_chat.is_streaming)
                    .show(ui, |ui| {
                        if self.inline_chat.is_streaming && self.inline_chat.streaming_text.is_empty() {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.colored_label(theme.text_muted, "Editing...");
                            });
                        } else if !self.inline_chat.streaming_text.is_empty() {
                            for line in self.inline_chat.streaming_text.lines() {
                                let color = if line.starts_with("+ ") {
                                    theme.success
                                } else if line.starts_with("- ") {
                                    theme.error
                                } else {
                                    theme.text_secondary
                                };
                                ui.colored_label(color, egui::RichText::new(line).monospace().size(11.0));
                            }
                        } else {
                            ui.colored_label(theme.text_muted, "Response will appear here...");
                        }
                    });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.colored_label(theme.text_muted, egui::RichText::new("Edit mode  ·  Esc to close").size(10.0));
                    if ui.small_button("Clear").clicked() {
                        self.inline_chat.streaming_text.clear();
                        self.inline_chat.query.clear();
                    }
                });
            });

        // Send query to agent using Edit mode + file context
        if let Some(query) = send_query {
            let file_ctx = self.current_file_context();
            let augmented = if file_ctx.is_empty() {
                query.clone()
            } else {
                format!("{}\n\n---\n{}", query, file_ctx)
            };

            let system_prompt = AiMode::Edit.system_prompt().to_string();
            let tx = self.inline_event_tx.clone();
            let settings = self.settings.clone();

            // Inline chat uses a fresh conversation each time
            let conversation = Arc::new(tokio::sync::Mutex::new({
                let mut conv = phazeai_core::ConversationHistory::new();
                conv.set_system_prompt(system_prompt);
                conv
            }));

            self.runtime.spawn(async move {
                match settings.build_llm_client() {
                    Ok(llm) => {
                        let agent = phazeai_core::Agent::new(llm)
                            .with_shared_conversation(conversation);
                        if let Err(e) = agent.run_with_events(&augmented, tx.clone()).await {
                            let _ = tx.send(AgentEvent::Error(e.to_string()));
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(AgentEvent::Error(format!("LLM error: {e}")));
                    }
                }
            });

            self.inline_chat.is_streaming = true;
            self.inline_chat.streaming_text.clear();
        }
    }

    fn render_welcome_screen(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.add_space(ui.available_height() * 0.15);
        ui.vertical_centered(|ui| {
            ui.heading(RichText::new("🔥 PhazeAI IDE").color(theme.accent).size(36.0));
            ui.add_space(8.0);
            ui.colored_label(theme.text_secondary, "Local AI-native code editor");
            ui.add_space(32.0);

            let btn_size = [200.0, 36.0];

            if ui.add_sized(btn_size, egui::Button::new(
                RichText::new("📂  Open Folder").color(theme.text).size(14.0)
            ).fill(theme.background_secondary)).clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.explorer.set_root(path.clone());
                    self.search.set_root(path.clone());
                    self.terminal.set_cwd(path.clone());
                    self.diff.set_git_root(path.clone());
                    self.init_lsp(path);
                    self.show_explorer = true;
                }
            }

            ui.add_space(8.0);

            if ui.add_sized(btn_size, egui::Button::new(
                RichText::new("📄  New File").color(theme.text).size(14.0)
            ).fill(theme.background_secondary)).clicked() {
                self.editor.new_tab();
            }

            ui.add_space(32.0);
            ui.separator();
            ui.add_space(16.0);

            ui.colored_label(theme.text_muted, "Keyboard shortcuts:");
            ui.add_space(8.0);

            let shortcuts = [
                ("Ctrl+O", "Open file"),
                ("Ctrl+K", "Inline AI chat"),
                ("Ctrl+L", "Focus chat panel"),
                ("Ctrl+Shift+F", "Workspace search"),
                ("Ctrl+P", "Command palette"),
                ("Ctrl+Z", "Undo"),
            ];
            for (key, desc) in &shortcuts {
                ui.horizontal(|ui| {
                    ui.add_space(80.0);
                    ui.colored_label(theme.accent,
                        RichText::new(*key).monospace().size(12.0)
                    );
                    ui.add_space(16.0);
                    ui.colored_label(theme.text_secondary, RichText::new(*desc).size(12.0));
                });
            }
        });
    }

    fn render_approval_window(&mut self, ctx: &egui::Context) {
        if self.pending_approval.is_none() {
            return;
        }

        let is_file_edit = self.pending_approval.as_ref()
            .map(|a| matches!(a.tool_name.as_str(), "write_file" | "edit_file"))
            .unwrap_or(false);
        let has_hunks = self.pending_approval.as_ref()
            .map(|a| !a.hunks.is_empty())
            .unwrap_or(false);
        let window_height = if has_hunks { 560.0 } else if is_file_edit { 500.0 } else { 250.0 };

        let mut decision: Option<bool> = None;
        let mut apply_partial = false;

        let tool_name = self.pending_approval.as_ref().unwrap().tool_name.clone();
        let params_summary = self.pending_approval.as_ref().unwrap().params_summary.clone();

        egui::Window::new(format!("Tool Request: {}", tool_name))
            .collapsible(false)
            .resizable(true)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_size([680.0, window_height])
            .show(ctx, |ui| {
                // Header
                let icon_color = if is_file_edit { self.theme.warning } else { self.theme.accent };
                ui.colored_label(icon_color, RichText::new(&tool_name).strong().size(14.0));
                ui.add_space(4.0);

                if has_hunks {
                    // ── Per-hunk view ────────────────────────────────────────
                    let accepted_count = self.pending_approval.as_ref().unwrap().hunks.iter()
                        .filter(|h| h.accepted).count();
                    let total_count = self.pending_approval.as_ref().unwrap().hunks.len();

                    ui.horizontal(|ui| {
                        ui.colored_label(self.theme.text_muted,
                            format!("Review changes — {} / {} hunks selected", accepted_count, total_count));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Select All").clicked() {
                                if let Some(a) = self.pending_approval.as_mut() {
                                    for h in &mut a.hunks { h.accepted = true; }
                                }
                            }
                            if ui.small_button("Deselect All").clicked() {
                                if let Some(a) = self.pending_approval.as_mut() {
                                    for h in &mut a.hunks { h.accepted = false; }
                                }
                            }
                        });
                    });
                    ui.add_space(4.0);

                    egui::ScrollArea::vertical()
                        .max_height(400.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let hunk_count = self.pending_approval.as_ref().unwrap().hunks.len();
                            for i in 0..hunk_count {
                                let accepted = self.pending_approval.as_ref().unwrap().hunks[i].accepted;
                                let header = self.pending_approval.as_ref().unwrap().hunks[i].header.clone();
                                let lines = self.pending_approval.as_ref().unwrap().hunks[i].lines.clone();

                                let hunk_bg = if accepted {
                                    self.theme.success.linear_multiply(0.08)
                                } else {
                                    self.theme.background_secondary
                                };

                                egui::Frame::none()
                                    .fill(hunk_bg)
                                    .stroke(egui::Stroke::new(1.0,
                                        if accepted { self.theme.success } else { self.theme.border }
                                    ))
                                    .rounding(egui::Rounding::same(4.0))
                                    .inner_margin(egui::Margin::same(6.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let mut acc = accepted;
                                            if ui.checkbox(&mut acc, "").changed() {
                                                if let Some(a) = self.pending_approval.as_mut() {
                                                    a.hunks[i].accepted = acc;
                                                }
                                            }
                                            ui.colored_label(self.theme.accent,
                                                RichText::new(&header).monospace().size(11.0));
                                        });

                                        for (kind, content) in &lines {
                                            let (prefix, color) = match kind {
                                                HunkLineKind::Added    => ("+", self.theme.success),
                                                HunkLineKind::Removed  => ("-", self.theme.error),
                                                HunkLineKind::Context  => (" ", self.theme.text_muted),
                                            };
                                            let text = format!("{} {}", prefix,
                                                content.trim_end_matches('\n'));
                                            ui.colored_label(color,
                                                RichText::new(&text).monospace().size(11.0));
                                        }
                                    });
                                ui.add_space(4.0);
                            }
                        });

                    ui.separator();
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let apply_btn = egui::Button::new(
                            RichText::new("  Apply Selected  ").color(Color32::WHITE)
                        ).fill(self.theme.success);
                        if ui.add(apply_btn).clicked() {
                            apply_partial = true;
                        }
                        ui.add_space(4.0);
                        let all_btn = egui::Button::new(
                            RichText::new("  Accept All  (Y)  ").color(Color32::WHITE)
                        ).fill(self.theme.accent);
                        if ui.add(all_btn).clicked() {
                            decision = Some(true);
                        }
                        ui.add_space(4.0);
                        let deny_btn = egui::Button::new(
                            RichText::new("  Deny  (N)  ").color(Color32::WHITE)
                        ).fill(self.theme.error);
                        if ui.add(deny_btn).clicked() {
                            decision = Some(false);
                        }
                    });
                } else if is_file_edit {
                    // ── Fallback: plain diff view ─────────────────────────
                    ui.colored_label(self.theme.text_muted, "Review the proposed changes:");
                    ui.add_space(4.0);
                    if let Some(diff_str) = self.pending_approval.as_ref()
                        .and_then(|a| a.diff.clone())
                    {
                        egui::ScrollArea::vertical()
                            .max_height(380.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for line in diff_str.lines() {
                                    let (color, text) = if line.starts_with("+ ") {
                                        (self.theme.success, line)
                                    } else if line.starts_with("- ") {
                                        (self.theme.error, line)
                                    } else if line.starts_with("---") || line.starts_with("+++") {
                                        (self.theme.accent, line)
                                    } else {
                                        (self.theme.text_muted, line)
                                    };
                                    ui.colored_label(color,
                                        RichText::new(text).monospace().size(11.0));
                                }
                            });
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(
                            RichText::new("  Allow  (Y)  ").color(Color32::WHITE)
                        ).fill(self.theme.success)).clicked() { decision = Some(true); }
                        ui.add_space(8.0);
                        if ui.add(egui::Button::new(
                            RichText::new("  Deny  (N)  ").color(Color32::WHITE)
                        ).fill(self.theme.error)).clicked() { decision = Some(false); }
                    });
                } else {
                    // ── Generic tool: params summary ─────────────────────
                    ui.colored_label(self.theme.text_muted, "Parameters:");
                    ui.add_space(4.0);
                    egui::Frame::none()
                        .fill(self.theme.background)
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.colored_label(self.theme.text_secondary,
                                RichText::new(&params_summary).monospace().size(11.0));
                        });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(
                            RichText::new("  Allow  (Y)  ").color(Color32::WHITE)
                        ).fill(self.theme.success)).clicked() { decision = Some(true); }
                        ui.add_space(8.0);
                        if ui.add(egui::Button::new(
                            RichText::new("  Deny  (N)  ").color(Color32::WHITE)
                        ).fill(self.theme.error)).clicked() { decision = Some(false); }
                    });
                }
            });

        // Handle apply-partial: reconstruct file with only accepted hunks
        if apply_partial {
            if let Some(approval) = self.pending_approval.take() {
                if let Some(ref path) = approval.file_path {
                    let reconstructed = apply_selected_hunks(&approval.old_content, &approval.hunks);
                    let _ = std::fs::write(path, &reconstructed);
                    // Force editor to reload
                    self.last_file_check = std::time::Instant::now()
                        .checked_sub(std::time::Duration::from_secs(10))
                        .unwrap_or(std::time::Instant::now());
                }
                // Deny original tool write (we already wrote the partial result)
                if let Ok(mut state) = self.approval_state.lock() {
                    if let Some((_, _, _, tx)) = state.pending.take() {
                        let _ = tx.send(false);
                    }
                }
            }
        } else if let Some(d) = decision {
            self.handle_approval_response(d);
        }
    }

    fn render_agent_history(&mut self, ctx: &egui::Context) {
        if !self.show_agent_history {
            return;
        }
        let mut close = false;
        egui::Window::new("Agent History")
            .collapsible(false)
            .resizable(true)
            .anchor(egui::Align2::RIGHT_TOP, [-8.0, 40.0])
            .default_size([360.0, 400.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(self.theme.text_secondary,
                        format!("{} recent runs", self.agent_history.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕ Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.separator();

                if self.agent_history.is_empty() {
                    ui.add_space(20.0);
                    ui.colored_label(self.theme.text_muted,
                        "No agent runs yet. Start chatting to see history here.");
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for run in self.agent_history.iter().rev() {
                            egui::Frame::none()
                                .fill(self.theme.background_secondary)
                                .rounding(egui::Rounding::same(4.0))
                                .inner_margin(egui::Margin::same(8.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(self.theme.accent,
                                            RichText::new(&run.timestamp).size(11.0));
                                        ui.add_space(6.0);
                                        ui.colored_label(self.theme.text_muted,
                                            RichText::new(&run.mode_label).size(11.0));
                                        if run.tool_count > 0 {
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.colored_label(self.theme.text_muted,
                                                        RichText::new(
                                                            format!("{} tools", run.tool_count)
                                                        ).size(10.0));
                                                }
                                            );
                                        }
                                    });
                                    ui.add_space(2.0);
                                    ui.colored_label(self.theme.text_secondary,
                                        RichText::new(&run.summary).size(12.0));
                                });
                            ui.add_space(4.0);
                        }
                    });
                }
            });
        if close {
            self.show_agent_history = false;
        }
    }

    fn render_menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New File  (Ctrl+N)").clicked() {
                    self.editor.new_tab();
                    ui.close_menu();
                }
                if ui.button("Open File (Ctrl+O)").clicked() {
                    self.open_file_dialog();
                    ui.close_menu();
                }
                if ui.button("Save      (Ctrl+S)").clicked() {
                    self.editor.save_active();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Open Folder").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.explorer.set_root(path.clone());
                        self.search.set_root(path.clone());
                        self.terminal.set_cwd(path.clone());
                        self.diff.set_git_root(path.clone());
                        self.init_lsp(path);
                    }
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Settings").clicked() {
                    self.settings_panel.toggle();
                    ui.close_menu();
                }
            });

            ui.menu_button("View", |ui| {
                if ui
                    .checkbox(&mut self.show_explorer, "Explorer / Search (Ctrl+E)")
                    .clicked()
                {
                    ui.close_menu();
                }
                if ui.button("Search in Workspace (Ctrl+Shift+F)").clicked() {
                    self.show_explorer = true;
                    self.left_tab = LeftPanelTab::Search;
                    ui.close_menu();
                }
                if ui.button("Git Panel (Ctrl+Shift+G)").clicked() {
                    self.show_explorer = true;
                    self.left_tab = LeftPanelTab::Git;
                    self.diff.refresh();
                    ui.close_menu();
                }
                ui.separator();
                if ui
                    .checkbox(&mut self.show_chat, "AI Chat (Ctrl+J)")
                    .clicked()
                {
                    ui.close_menu();
                }
                if ui
                    .checkbox(&mut self.show_terminal, "Terminal (Ctrl+`)")
                    .clicked()
                {
                    ui.close_menu();
                }
                ui.separator();
                if ui
                    .checkbox(&mut self.show_browser, "Docs Browser (Ctrl+Shift+B)")
                    .clicked()
                {
                    ui.close_menu();
                }
                if ui
                    .checkbox(&mut self.editor.show_minimap, "Minimap")
                    .clicked()
                {
                    ui.close_menu();
                }
                if ui
                    .checkbox(&mut self.editor.show_line_numbers, "Line Numbers")
                    .clicked()
                {
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Split Editor Right").clicked() {
                    let font_size = self.settings.editor.font_size;
                    let mut split = EditorPanel::new(font_size);
                    // Open the same file in the split (if any)
                    if let Some(tab) = self.editor.tabs.get(self.editor.active_tab) {
                        if let Some(path) = tab.path.clone() {
                            split.open_file(path);
                        }
                    }
                    self.split_editor = Some(split);
                    ui.close_menu();
                }
                if self.split_editor.is_some() {
                    if ui.button("Close Split").clicked() {
                        self.split_editor = None;
                        ui.close_menu();
                    }
                }
            });

            ui.menu_button("Help", |ui| {
                if ui.button("About").clicked() {
                    self.show_about = true;
                    ui.close_menu();
                }
            });

            // Right-aligned status
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let model = &self.settings.llm.model;
                ui.colored_label(
                    self.theme.text_muted,
                    RichText::new(model).small(),
                );
                ui.colored_label(self.theme.text_muted, RichText::new("|").small());
                ui.colored_label(
                    self.theme.text_muted,
                    RichText::new(self.theme_preset.name()).small(),
                );
            });
        });
    }

    fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Current file info
            if let Some(tab) = self.editor.tabs.get(self.editor.active_tab) {
                let file_name = tab
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Untitled".to_string());
                ui.colored_label(self.theme.text_secondary, RichText::new(&file_name).small());
                ui.colored_label(self.theme.text_muted, RichText::new("|").small());
                ui.colored_label(
                    self.theme.text_muted,
                    RichText::new(format!("Ln {}, Col {}", tab.cursor_line + 1, tab.cursor_col + 1)).small(),
                );
                if !tab.language.is_empty() {
                    ui.colored_label(self.theme.text_muted, RichText::new("|").small());
                    ui.colored_label(self.theme.text_secondary, RichText::new(&tab.language).small());
                }
                if tab.modified {
                    ui.colored_label(self.theme.warning, RichText::new(" ●").small());
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(
                    self.theme.text_muted,
                    RichText::new("PhazeAI IDE v0.1.0").small(),
                );

                // Token usage meter (estimated from conversation history)
                if let Ok(conv) = self.agent_conversation.try_lock() {
                    let tokens = conv.estimate_tokens();
                    if tokens > 0 {
                        let token_color = if tokens > 24000 {
                            self.theme.error
                        } else if tokens > 12000 {
                            self.theme.warning
                        } else {
                            self.theme.text_muted
                        };
                        ui.colored_label(self.theme.text_muted, RichText::new("|").small());
                        ui.colored_label(token_color,
                            RichText::new(format!("~{}k ctx", tokens / 1000 + 1)).small()
                        );
                    }
                }

                // AI mode indicator
                let mode_color = match self.chat.mode {
                    crate::panels::chat::AiMode::Chat  => self.theme.accent,
                    crate::panels::chat::AiMode::Ask   => self.theme.text_secondary,
                    crate::panels::chat::AiMode::Debug => self.theme.error,
                    crate::panels::chat::AiMode::Plan  => self.theme.warning,
                    crate::panels::chat::AiMode::Edit  => self.theme.success,
                };
                ui.colored_label(self.theme.text_muted, RichText::new("|").small());
                ui.colored_label(mode_color,
                    RichText::new(format!("{} {}", self.chat.mode.icon(), self.chat.mode.label())).small()
                );

                // Agent streaming indicator
                if self.chat.is_streaming {
                    ui.colored_label(self.theme.text_muted, RichText::new("|").small());
                    ui.spinner();
                    ui.colored_label(self.theme.accent, RichText::new("AI").small());
                }

                // Timed notification (expires after 4s)
                if let Some((ref msg, success, since)) = self.status_notification {
                    if since.elapsed().as_secs() < 4 {
                        let color = if success { self.theme.success } else { self.theme.error };
                        ui.colored_label(self.theme.text_muted, RichText::new("|").small());
                        ui.colored_label(color, RichText::new(msg.as_str()).small());
                    } else {
                        self.status_notification = None;
                    }
                }
            });
        });
    }
}

impl eframe::App for PhazeApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.update_raw(ctx, Some(frame));
    }
}

impl PhazeApp {
    pub fn update_raw(&mut self, ctx: &egui::Context, _frame: Option<&mut eframe::Frame>) {
        // Apply theme
        self.theme.apply(ctx);

        // Handle keybindings
        self.handle_keybindings(ctx);

        // Process events
        self.process_agent_events();
        self.process_chat_messages();
        self.process_chat_cancellation();
        self.process_chat_apply();
        self.process_inline_events();
        self.check_settings_changes();
        self.check_explorer_file_open();
        self.check_search_file_open();
        self.drain_file_loads();
        self.check_approval_requests();
        self.check_file_changes();
        self.process_lsp_events();
        self.lsp_poll_hover();
        self.save_state_periodically();

        // Settings window (floating)
        self.settings_panel.show(ctx, &self.theme);

        // Command palette (floating overlay)
        if self.show_palette {
            self.render_command_palette(ctx);
        }

        // About dialog (floating)
        if self.show_about {
            egui::Window::new("About PhazeAI IDE")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        ui.heading(RichText::new("PhazeAI IDE").size(24.0));
                        ui.add_space(5.0);
                        ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).size(14.0));
                        ui.add_space(15.0);
                        ui.label("An AI-powered coding assistant");
                        ui.add_space(20.0);
                        ui.label(RichText::new("Built with:").strong());
                        ui.add_space(5.0);
                        ui.label("Rust, egui, Tokio");
                        ui.add_space(20.0);
                        if ui.button("Close").clicked() {
                            self.show_about = false;
                        }
                        ui.add_space(10.0);
                    });
                });
        }

        // Tool approval window (floating)
        self.render_approval_window(ctx);

        // Agent history popup
        if self.chat.pending_show_history {
            self.chat.pending_show_history = false;
            self.show_agent_history = !self.show_agent_history;
        }
        self.render_agent_history(ctx);

        // Inline chat popup (Ctrl+K)
        self.render_inline_chat(ctx);

        // Top menu bar
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            self.render_menu_bar(ui);
        });

        // Status bar at bottom
        egui::TopBottomPanel::bottom("status_bar")
            .exact_height(24.0)
            .show(ctx, |ui| {
                self.render_status_bar(ui);
            });

        if self.show_browser && self.browser.loading {
            self.process_browser_requests();
        }

        // Terminal panel at bottom (above status bar)
        if self.show_terminal {
            egui::TopBottomPanel::bottom("terminal_panel")
                .resizable(true)
                .default_height(self.terminal_height)
                .min_height(100.0)
                .max_height(400.0)
                .show(ctx, |ui| {
                    let theme = self.theme.clone();
                    self.terminal.show(ui, &theme);
                });
        }

        // Left panel: tabbed Explorer / Search
        if self.show_explorer {
            egui::SidePanel::left("explorer_panel")
                .resizable(true)
                .default_width(self.explorer_width)
                .min_width(150.0)
                .max_width(400.0)
                .show(ctx, |ui| {
                    let theme = self.theme.clone();
                    // Tab bar
                    ui.horizontal(|ui| {
                        let exp_sel = self.left_tab == LeftPanelTab::Explorer;
                        let srch_sel = self.left_tab == LeftPanelTab::Search;
                        let git_sel = self.left_tab == LeftPanelTab::Git;
                        let out_sel = self.left_tab == LeftPanelTab::Outline;
                        if ui.selectable_label(exp_sel, "  Files  ").clicked() {
                            self.left_tab = LeftPanelTab::Explorer;
                        }
                        if ui.selectable_label(srch_sel, "  Search  ").clicked() {
                            self.left_tab = LeftPanelTab::Search;
                        }
                        if ui.selectable_label(git_sel, "  Git  ").clicked() {
                            self.left_tab = LeftPanelTab::Git;
                            self.diff.refresh();
                        }
                        if ui.selectable_label(out_sel, "  Outline  ").clicked() {
                            self.left_tab = LeftPanelTab::Outline;
                            // Refresh outline from current tab
                            let idx = self.editor.active_tab;
                            if let Some(tab) = self.editor.tabs.get(idx) {
                                let content = tab.rope.to_string();
                                let lang = tab.language.clone();
                                self.outline.update(&content, &lang);
                            }
                        }
                    });
                    ui.separator();
                    match self.left_tab {
                        LeftPanelTab::Explorer => self.explorer.show(ui, &theme),
                        LeftPanelTab::Search => self.search.show(ui, &theme),
                        LeftPanelTab::Git => self.diff.show(ui, &theme),
                        LeftPanelTab::Outline => {
                            // Keep outline fresh each frame when visible
                            let idx = self.editor.active_tab;
                            if let Some(tab) = self.editor.tabs.get(idx) {
                                let content = tab.rope.to_string();
                                let lang = tab.language.clone();
                                self.outline.update(&content, &lang);
                            }
                            self.outline.show(ui, &theme);
                            // Handle jump-to-line requests
                            if let Some(line) = self.outline.jump_to_line.take() {
                                let idx = self.editor.active_tab;
                                if let Some(tab) = self.editor.tabs.get_mut(idx) {
                                    use crate::panels::editor::TextPosition;
                                    tab.cursor = TextPosition::new(line, 0);
                                    tab.cursor_changed = true;
                                    tab.selection = None;
                                }
                            }
                        }
                    }
                });
        }

        // Chat panel on the right
        if self.show_chat {
            egui::SidePanel::right("chat_panel")
                .resizable(true)
                .default_width(self.chat_width)
                .min_width(250.0)
                .max_width(600.0)
                .show(ctx, |ui| {
                    let theme = self.theme.clone();
                    self.chat.show(ui, &theme);
                });
        }

        // Browser panel on the right (will take space next to chat)
        if self.show_browser {
            egui::SidePanel::right("browser_panel")
                .resizable(true)
                .default_width(400.0)
                .min_width(200.0)
                .max_width(800.0)
                .show(ctx, |ui| {
                    let theme = self.theme.clone();
                    self.browser.show(ui, &theme);
                });
        }

        // Central panel - editor, diff view, or welcome screen
        let no_workspace = self.explorer.root().is_none();
        let no_tabs_open = self.editor.tabs.len() == 1
            && self.editor.tabs[0].path.is_none()
            && self.editor.tabs[0].rope.len_chars() == 0;
        let showing_git_diff = self.show_explorer
            && self.left_tab == LeftPanelTab::Git
            && self.diff.has_diff_selected();

        let editor_resp = egui::CentralPanel::default().show(ctx, |ui| {
            let theme = self.theme.clone();
            if showing_git_diff {
                self.diff.show_diff_central(ui, &theme);
            } else if no_workspace && no_tabs_open && self.split_editor.is_none() {
                self.render_welcome_screen(ui, &theme);
            } else if self.split_editor.is_some() {
                // ── Split view ─────────────────────────────────────────────
                let available = ui.available_rect_before_wrap();
                let total_w = available.width();
                let split_x = available.left() + total_w * self.split_ratio;

                // Left pane
                {
                    let left_rect = egui::Rect::from_min_max(
                        available.min,
                        egui::pos2(split_x - 2.0, available.max.y),
                    );
                    let mut child = ui.child_ui(left_rect, ui.layout().clone(), None);
                    self.editor.show(&mut child, &theme);
                }

                // Draggable separator
                let sep_rect = egui::Rect::from_min_max(
                    egui::pos2(split_x - 2.0, available.min.y),
                    egui::pos2(split_x + 2.0, available.max.y),
                );
                ui.painter().rect_filled(sep_rect, 0.0, theme.border);
                let sep_resp = ui.interact(
                    sep_rect,
                    ui.id().with("split_sep"),
                    egui::Sense::drag(),
                );
                if sep_resp.dragged() {
                    let delta = sep_resp.drag_delta().x;
                    let new_ratio = ((self.split_ratio * total_w + delta) / total_w)
                        .clamp(0.15, 0.85);
                    self.split_ratio = new_ratio;
                }
                if sep_resp.hovered() || sep_resp.dragged() {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }

                // Right pane
                {
                    let right_rect = egui::Rect::from_min_max(
                        egui::pos2(split_x + 2.0, available.min.y),
                        available.max,
                    );
                    let mut child = ui.child_ui(right_rect, ui.layout().clone(), None);
                    // Also drain file loads for split editor
                    if let Some(ref mut split) = self.split_editor {
                        split.drain_file_loads();
                        split.show(&mut child, &theme);
                    }
                }
            } else {
                self.editor.show(ui, &theme);
            }
        });

        // Update editor focus (not when showing git diff)
        if !showing_git_diff {
            if let Some(pos) = ctx.input(|i| i.pointer.press_origin()) {
                if editor_resp.response.rect.contains(pos) {
                    self.editor.has_focus = true;
                } else {
                    self.editor.has_focus = false;
                }
            }
        }
        
        // If anything else is explicitly focused (like a TextEdit), editor must lose focus
        if let Some(_focus_id) = ctx.memory(|mem| mem.focused()) {
             self.editor.has_focus = false;
        }

        // Request repaint for streaming updates
        if self.chat.is_streaming || self.inline_chat.is_streaming {
            ctx.request_repaint();
        }
    }
}

fn resolve_theme_preset(name: &str) -> ThemePreset {
    for preset in ThemePreset::all() {
        if preset.name().eq_ignore_ascii_case(name) {
            return preset;
        }
    }
    ThemePreset::Dark
}

/// Simple fuzzy match: all characters in the pattern must appear in order in the haystack.
fn fuzzy_match(haystack: &str, pattern: &str) -> bool {
    let mut hay_chars = haystack.chars();
    for p in pattern.chars() {
        loop {
            match hay_chars.next() {
                Some(h) if h == p => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// Recursively collect source files from a directory, skipping common noise.
fn collect_project_files(dir: &std::path::Path, files: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden, build artifacts, and noise
        if name.starts_with('.')
            || name == "node_modules"
            || name == "target"
            || name == "__pycache__"
            || name == "_archive"
            || name == "dist"
            || name == "build"
        {
            continue;
        }

        if path.is_dir() {
            collect_project_files(&path, files, depth + 1);
        } else {
            files.push(path);
        }

        if files.len() >= 5000 {
            return;
        }
    }
}

/// Compute diff data for write_file / edit_file tool calls.
/// Returns (diff_string, hunks, old_content, new_content, file_path).
fn compute_diff_data(
    tool_name: &str,
    params: &serde_json::Value,
) -> (Option<String>, Vec<DiffHunk>, String, String, Option<String>) {
    let empty = (None, Vec::new(), String::new(), String::new(), None);
    match tool_name {
        "write_file" => {
            let path = match params.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return empty,
            };
            let new_content = params.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let old_content = std::fs::read_to_string(path).unwrap_or_default();
            let diff = build_unified_diff(&old_content, &new_content, path);
            let hunks = compute_diff_hunks(&old_content, &new_content);
            (Some(diff), hunks, old_content, new_content, Some(path.to_string()))
        }
        "edit_file" => {
            let path = match params.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return empty,
            };
            let old_text = params.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
            let new_text = params.get("new_text").and_then(|v| v.as_str()).unwrap_or("");
            let old_content = std::fs::read_to_string(path).unwrap_or_default();
            let new_content = old_content.replacen(old_text, new_text, 1);
            let diff = build_unified_diff(&old_content, &new_content, path);
            let hunks = compute_diff_hunks(&old_content, &new_content);
            (Some(diff), hunks, old_content, new_content, Some(path.to_string()))
        }
        _ => empty,
    }
}

/// Parse a unified diff into structured hunks using similar's grouped_ops.
fn compute_diff_hunks(old: &str, new: &str) -> Vec<DiffHunk> {
    let diff = TextDiff::from_lines(old, new);
    let old_lines: Vec<&str> = diff.old_slices().to_vec();
    let new_lines: Vec<&str> = diff.new_slices().to_vec();
    let mut hunks = Vec::new();

    for group in diff.grouped_ops(3) {
        if group.is_empty() {
            continue;
        }
        // Compute hunk ranges from the group
        let old_start = group.first().map(|op| match op {
            DiffOp::Equal { old_index, .. } => *old_index,
            DiffOp::Delete { old_index, .. } => *old_index,
            DiffOp::Insert { old_index, .. } => *old_index,
            DiffOp::Replace { old_index, .. } => *old_index,
        }).unwrap_or(0);
        let old_end = group.last().map(|op| match op {
            DiffOp::Equal { old_index, len, .. } => old_index + len,
            DiffOp::Delete { old_index, old_len, .. } => old_index + old_len,
            DiffOp::Insert { old_index, .. } => *old_index,
            DiffOp::Replace { old_index, old_len, .. } => old_index + old_len,
        }).unwrap_or(0);
        let new_start = group.first().map(|op| match op {
            DiffOp::Equal { new_index, .. } => *new_index,
            DiffOp::Delete { new_index, .. } => *new_index,
            DiffOp::Insert { new_index, .. } => *new_index,
            DiffOp::Replace { new_index, .. } => *new_index,
        }).unwrap_or(0);
        let new_end = group.last().map(|op| match op {
            DiffOp::Equal { new_index, len, .. } => new_index + len,
            DiffOp::Delete { new_index, .. } => *new_index,
            DiffOp::Insert { new_index, new_len, .. } => new_index + new_len,
            DiffOp::Replace { new_index, new_len, .. } => new_index + new_len,
        }).unwrap_or(0);

        let header = format!("@@ -{},{} +{},{} @@",
            old_start + 1, old_end.saturating_sub(old_start),
            new_start + 1, new_end.saturating_sub(new_start));

        let mut lines = Vec::new();
        for op in &group {
            match op {
                DiffOp::Equal { old_index, len, .. } => {
                    for i in 0..*len {
                        let content = old_lines.get(old_index + i)
                            .copied().unwrap_or("").to_string();
                        lines.push((HunkLineKind::Context, content));
                    }
                }
                DiffOp::Delete { old_index, old_len, .. } => {
                    for i in 0..*old_len {
                        let content = old_lines.get(old_index + i)
                            .copied().unwrap_or("").to_string();
                        lines.push((HunkLineKind::Removed, content));
                    }
                }
                DiffOp::Insert { new_index, new_len, .. } => {
                    for i in 0..*new_len {
                        let content = new_lines.get(new_index + i)
                            .copied().unwrap_or("").to_string();
                        lines.push((HunkLineKind::Added, content));
                    }
                }
                DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                    for i in 0..*old_len {
                        let content = old_lines.get(old_index + i)
                            .copied().unwrap_or("").to_string();
                        lines.push((HunkLineKind::Removed, content));
                    }
                    for i in 0..*new_len {
                        let content = new_lines.get(new_index + i)
                            .copied().unwrap_or("").to_string();
                        lines.push((HunkLineKind::Added, content));
                    }
                }
            }
        }

        hunks.push(DiffHunk {
            header,
            lines,
            old_start,
            old_end,
            accepted: true, // default: all hunks accepted
        });
    }
    hunks
}

/// Reconstruct file content applying only accepted hunks, keeping rejected regions as-is.
fn apply_selected_hunks(old_content: &str, hunks: &[DiffHunk]) -> String {
    let old_lines: Vec<&str> = old_content.split_inclusive('\n').collect();
    let mut result = String::new();
    let mut old_pos = 0usize;

    for hunk in hunks {
        // Copy unchanged lines before this hunk
        while old_pos < hunk.old_start && old_pos < old_lines.len() {
            result.push_str(old_lines[old_pos]);
            old_pos += 1;
        }

        if hunk.accepted {
            // Apply hunk: context lines advance old_pos, added lines are emitted, removed are skipped
            for (kind, content) in &hunk.lines {
                match kind {
                    HunkLineKind::Context => {
                        if old_pos < old_lines.len() {
                            result.push_str(old_lines[old_pos]);
                        }
                        old_pos += 1;
                    }
                    HunkLineKind::Removed => {
                        old_pos += 1; // skip old line
                    }
                    HunkLineKind::Added => {
                        result.push_str(content);
                        if !content.ends_with('\n') {
                            result.push('\n');
                        }
                    }
                }
            }
        } else {
            // Reject hunk: keep original old lines verbatim
            while old_pos < hunk.old_end && old_pos < old_lines.len() {
                result.push_str(old_lines[old_pos]);
                old_pos += 1;
            }
        }
    }

    // Copy remaining lines after all hunks
    while old_pos < old_lines.len() {
        result.push_str(old_lines[old_pos]);
        old_pos += 1;
    }

    result
}

fn build_unified_diff(old: &str, new: &str, path: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut result = format!("--- {}\n+++ {}\n", path, path);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => result.push_str(&format!("- {}", change)),
            ChangeTag::Insert => result.push_str(&format!("+ {}", change)),
            ChangeTag::Equal  => result.push_str(&format!("  {}", change)),
        }
    }
    // Limit to 200 lines
    let lines: Vec<&str> = result.lines().collect();
    if lines.len() > 200 {
        lines[..200].join("\n") + "\n... (diff truncated)"
    } else {
        result
    }
}

fn collect_tree(dir: &std::path::Path, lines: &mut Vec<String>, depth: usize, max_depth: usize) {
    if depth > max_depth { return; }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let indent = "  ".repeat(depth);
    let mut sorted: Vec<_> = entries.flatten().collect();
    sorted.sort_by_key(|e| {
        let is_dir = e.path().is_dir();
        (!is_dir, e.file_name().to_string_lossy().to_lowercase())
    });

    for entry in sorted.iter().take(50) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules"
            || name == "__pycache__" || name == "dist" { continue; }
        let path = entry.path();
        if path.is_dir() {
            lines.push(format!("{}{}/", indent, name));
            collect_tree(&path, lines, depth + 1, max_depth);
        } else {
            lines.push(format!("{}{}", indent, name));
        }
        if lines.len() >= 200 { return; }
    }
}

fn extract_title(html: &str) -> Option<String> {
    let start_tag = "<title>";
    let end_tag = "</title>";
    if let Some(start) = html.find(start_tag) {
        if let Some(end) = html[start..].find(end_tag) {
            return Some(html[start + start_tag.len()..start + end].trim().to_string());
        }
    }
    None
}

fn clean_html_to_markdown(html: &str) -> String {
    // Remove script and style blocks entirely
    let mut text = html.to_string();
    for tag in &["script", "style", "nav", "header", "footer"] {
        let open = format!("<{}", tag);
        let close = format!("</{}>", tag);
        while let Some(start) = text.to_lowercase().find(&open) {
            if let Some(rel_end) = text.to_lowercase()[start..].find(&close) {
                let end = start + rel_end + close.len();
                text.replace_range(start..end, "");
            } else {
                break;
            }
        }
    }

    let mut result = String::new();
    let mut chars = text.chars().peekable();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    let mut last_was_newline = false;

    while let Some(c) = chars.next() {
        if c == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if c == '>' && in_tag {
            in_tag = false;
            let tag = tag_buf.trim().to_lowercase();
            let tag_name = tag.split_whitespace().next().unwrap_or("");
            // Extract href from <a href="...">
            let href = if tag_name == "a" {
                extract_attr(&tag_buf, "href")
            } else {
                None
            };
            match tag_name {
                "p" | "div" | "section" | "article" | "main" | "h1" | "h2" | "h3"
                | "h4" | "h5" | "h6" | "li" | "tr" | "blockquote" => {
                    if !last_was_newline && !result.is_empty() {
                        result.push('\n');
                    }
                    if matches!(tag_name, "h1" | "h2" | "h3") {
                        result.push_str("## ");
                    }
                    last_was_newline = true;
                }
                "/p" | "/div" | "/section" | "/article" | "/main" | "/h1" | "/h2"
                | "/h3" | "/h4" | "/h5" | "/h6" | "/li" | "/tr" | "/blockquote" => {
                    if !last_was_newline {
                        result.push('\n');
                        last_was_newline = true;
                    }
                }
                "br" | "br/" => {
                    result.push('\n');
                    last_was_newline = true;
                }
                "hr" | "hr/" => {
                    result.push_str("\n---\n");
                    last_was_newline = true;
                }
                _ => {}
            }
            if let Some(url) = href {
                if !url.is_empty() && !url.starts_with("javascript:") {
                    result.push_str(&format!(" [{}] ", url));
                }
            }
        } else if in_tag {
            tag_buf.push(c);
        } else {
            // Decode common HTML entities
            if c == '&' {
                let mut entity = String::new();
                let mut consumed = false;
                for ec in chars.by_ref() {
                    if ec == ';' {
                        consumed = true;
                        break;
                    }
                    entity.push(ec);
                    if entity.len() > 10 { break; }
                }
                if consumed {
                    let decoded = match entity.as_str() {
                        "amp"  => "&",
                        "lt"   => "<",
                        "gt"   => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        "nbsp" => " ",
                        "mdash" | "#8212" => "—",
                        "ndash" | "#8211" => "–",
                        "hellip" | "#8230" => "...",
                        "laquo" | "#171"  => "«",
                        "raquo" | "#187"  => "»",
                        _ => {
                            // Numeric entity &#NNN;
                            if let Some(num_str) = entity.strip_prefix('#') {
                                let cp: u32 = if let Some(hex) = num_str.strip_prefix('x') {
                                    u32::from_str_radix(hex, 16).unwrap_or(32)
                                } else {
                                    num_str.parse().unwrap_or(32)
                                };
                                let ch = char::from_u32(cp).unwrap_or(' ');
                                result.push(ch);
                                last_was_newline = ch == '\n';
                                continue;
                            }
                            result.push('&');
                            result.push_str(&entity);
                            result.push(';');
                            last_was_newline = false;
                            continue;
                        }
                    };
                    result.push_str(decoded);
                    last_was_newline = false;
                } else {
                    result.push('&');
                    result.push_str(&entity);
                    last_was_newline = false;
                }
            } else if c == '\n' || c == '\r' {
                if !last_was_newline {
                    result.push('\n');
                    last_was_newline = true;
                }
            } else if c.is_whitespace() {
                if !last_was_newline && !result.ends_with(' ') {
                    result.push(' ');
                }
            } else {
                result.push(c);
                last_was_newline = false;
            }
        }
    }

    // Collapse 3+ blank lines into 2
    let mut out = String::new();
    let mut blank_count = 0usize;
    for line in result.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 2 { out.push('\n'); }
        } else {
            blank_count = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let search = format!("{}=\"", attr);
    if let Some(start) = tag.to_lowercase().find(&search) {
        let after = &tag[start + search.len()..];
        if let Some(end) = after.find('"') {
            return Some(after[..end].to_string());
        }
    }
    None
}

/// Decode percent-encoded URI path component (e.g. %20 → space).
fn percent_decode_uri(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h1), Some(h2)) = (hex_val(bytes[i+1]), hex_val(bytes[i+2])) {
                result.push(char::from(h1 * 16 + h2));
                i += 3;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
