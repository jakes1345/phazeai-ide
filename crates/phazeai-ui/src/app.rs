mod menu;
mod overlays;
use menu::menu_bar;
use overlays::{
    branch_picker_overlay, code_actions_overlay, command_palette, completion_popup, file_picker,
    goto_overlay, hover_tooltip, inline_edit_overlay, peek_def_overlay, rename_overlay,
    sig_help_overlay, toast_overlay, vim_ex_overlay, workspace_symbols_overlay,
};

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use floem::{
    action::show_context_menu,
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, Modifiers},
    menu::{Menu, MenuItem},
    peniko::kurbo::Size,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{canvas, container, dyn_stack, empty, label, scroll, stack, Decorators},
    window::WindowConfig,
    Application, IntoView, Renderer,
};
use phazeai_core::config::LlmProvider;
use phazeai_core::constants::ui as ui_const;
use phazeai_core::Settings;
use phazeai_sidecar::{SidecarClient, SidecarManager};

use crate::lsp_bridge::{
    start_lsp_bridge, DiagEntry, DiagSeverity, LspCommand, ReferenceEntry, SymbolEntry,
};

use crate::{
    commands::{execute_command, match_global_shortcut},
    components::icon::{icons, phaze_icon},
    domain_state::{AiState, EditorState, IdeState, ProjectState, WorkbenchState},
    panels::{
        account::account_panel, chat::chat_panel, containers::containers_panel,
        editor::editor_panel, explorer::explorer_panel, extensions::extensions_panel,
        git::git_panel, github_actions::github_actions_panel, makefile::makefile_panel,
        remote::remote_panel, run_debug::run_debug_panel, search, settings::settings_panel,
        terminal::terminal_panel, tests::tests_panel,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Explorer,
    Search,
    Git,
    Composer,
    Settings,
    Terminal,
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
    Tests,
}

impl std::fmt::Debug for IdeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdeState")
            .field(
                "workspace_root",
                &self.project.workspace_root.get_untracked(),
            )
            .finish()
    }
}

impl IdeState {}

/// Persisted layout state from ~/.config/phazeai/session.toml.
/// `version` lets migrate() handle old files without panic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SessionState {
    version: u32,
    open_tabs: Vec<PathBuf>,
    active_tab_index: Option<usize>,
    left_panel_width: f64,
    show_left_panel: bool,
    show_right_panel: bool,
    show_bottom_panel: bool,
    split_editor: bool,
    split_editor_down: bool,
    vim_mode: bool,
    theme: String,
    zen_mode: bool,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
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
            zen_mode: false,
        }
    }
}

impl SessionState {
    const CURRENT_VERSION: u32 = 1;

    /// Apply any forward migrations and stamp the current version.
    fn migrate(mut self) -> Self {
        // v0 → v1: no field changes; just stamp the version.
        self.version = Self::CURRENT_VERSION;
        self
    }

    fn active_file(&self) -> Option<PathBuf> {
        let idx = self.active_tab_index?;
        self.open_tabs.get(idx).cloned()
    }

    /// Load from disk, migrate, and apply validity fixes (dead tab paths, bad index).
    fn load() -> Self {
        let Some(dir) = dirs_next_config() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(dir.join("session.toml")) else {
            return Self::default();
        };
        let mut s: Self = toml::from_str(&text).unwrap_or_default();
        s = s.migrate();
        s.open_tabs.retain(|p| p.exists());
        if let Some(idx) = s.active_tab_index {
            if s.open_tabs.is_empty() {
                s.active_tab_index = None;
            } else if idx >= s.open_tabs.len() {
                s.active_tab_index = Some(s.open_tabs.len().saturating_sub(1));
            }
        }
        s
    }

    /// Synchronous write to disk. Use `save_debounced` for reactive effects.
    fn save(&self) {
        let Some(dir) = dirs_next_config() else {
            return;
        };
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(content) = toml::to_string_pretty(self) {
            let _ = std::fs::write(dir.join("session.toml"), content);
        }
    }

    /// Debounced write: collapses rapid signal changes into one disk write per second.
    fn save_debounced(self, gen: std::sync::Arc<std::sync::atomic::AtomicU64>) {
        use std::sync::atomic::Ordering;
        let ticket = gen.fetch_add(1, Ordering::Relaxed) + 1;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(1));
            if gen.load(Ordering::Relaxed) == ticket {
                self.save();
            }
        });
    }

    /// Build a snapshot from live signals — call inside `create_effect` so the
    /// `.get()` calls register reactive subscriptions.
    fn from_signals(
        open_tabs: Vec<PathBuf>,
        active_file: Option<PathBuf>,
        left_panel_width: f64,
        show_left_panel: bool,
        show_right_panel: bool,
        show_bottom_panel: bool,
        split_editor: bool,
        split_editor_down: bool,
        vim_mode: bool,
        theme: String,
        zen_mode: bool,
    ) -> Self {
        let active_tab_index = active_file
            .as_ref()
            .and_then(|f| open_tabs.iter().position(|t| t == f));
        Self {
            version: Self::CURRENT_VERSION,
            open_tabs,
            active_tab_index,
            left_panel_width,
            show_left_panel,
            show_right_panel,
            show_bottom_panel,
            split_editor,
            split_editor_down,
            vim_mode,
            theme,
            zen_mode,
        }
    }

    /// Build an untracked snapshot from IdeState — use in WindowClosed handler.
    fn from_ide_state_untracked(state: &IdeState) -> Self {
        let open_tabs = state.editor.open_tabs.get_untracked();
        let active_file = state.editor.open_file.get_untracked();
        let active_tab_index = active_file
            .as_ref()
            .and_then(|f| open_tabs.iter().position(|t| t == f));
        Self {
            version: Self::CURRENT_VERSION,
            open_tabs,
            active_tab_index,
            left_panel_width: state.workbench.left_panel_width.get_untracked(),
            show_left_panel: state.workbench.show_left_panel.get_untracked(),
            show_right_panel: state.workbench.show_right_panel.get_untracked(),
            show_bottom_panel: state.workbench.show_bottom_panel.get_untracked(),
            split_editor: state.editor.split_editor.get_untracked(),
            split_editor_down: state.editor.split_editor_down.get_untracked(),
            vim_mode: state.editor.vim_mode.get_untracked(),
            theme: state
                .workbench
                .theme
                .get_untracked()
                .variant
                .name()
                .to_string(),
            zen_mode: state.workbench.zen_mode.get_untracked(),
        }
    }
}

fn dirs_next_config() -> Option<PathBuf> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .inspect_err(
            |e| tracing::warn!(target: "phazeai_ui", error = %e, "Failed to get HOME env var"),
        )
        .ok()?;
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

fn check_provider_ready(settings: &Settings) -> bool {
    match settings.llm.provider {
        LlmProvider::Ollama | LlmProvider::LmStudio => true,
        _ => {
            !settings.llm.api_key_env.is_empty()
                && std::env::var(&settings.llm.api_key_env)
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
        }
    }
}

fn check_python_ready(settings: &Settings) -> bool {
    let bins = [settings.sidecar.python_path.as_str(), "python3", "python"];
    bins.iter().any(|bin| {
        std::process::Command::new(bin)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

fn check_lsp_ready() -> bool {
    std::process::Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
        let client_guard = match shared_client.lock() {
            Ok(g) => g,
            Err(e) => {
                tracing::error!(target: "phazeai_ui", error = %e, "Failed to lock shared client");
                let _ = ready_tx.send(false);
                return;
            }
        };

        if client_guard.clone().is_some() {
            let _ = ready_tx.send(true);
            if build_after_start {
                let _ = build_tx.send(true);
            }
            return;
        }
        drop(client_guard);

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
            .map(|out| {
                if out.status.success() {
                    String::from_utf8(out.stdout)
                        .map(|s| PathBuf::from(s.trim()))
                        .unwrap_or(cwd.clone())
                } else {
                    tracing::debug!(target: "phazeai_ui", "Not in git repo, using cwd");
                    cwd.clone()
                }
            })
            .unwrap_or(cwd);

        // Sandbox every filesystem-touching tool to the resolved workspace root.
        // After this call, agent tools (bash, edit_file, write_file, download,
        // move_path, copy_path, create_directory, delete_path) refuse paths
        // outside the workspace, including `..` and symlink escapes.
        phazeai_core::tools::sandbox::set_workspace_root(Some(workspace.clone()));

        let git_branch = create_rw_signal("main".to_string());

        // Spawn a background thread to read the real git branch and push it to
        // the signal via a sync channel + create_signal_from_channel.
        let (branch_tx, branch_rx) = std::sync::mpsc::sync_channel::<String>(1);
        std::thread::spawn(move || {
            let branch = std::process::Command::new("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .output()
                .map(|out| {
                    if out.status.success() {
                        String::from_utf8(out.stdout)
                            .map(|s| s.trim().to_string())
                            .ok()
                            .and_then(|s| if s.is_empty() { None } else { Some(s) })
                            .unwrap_or_else(|| "main".to_string())
                    } else {
                        tracing::debug!(target: "phazeai_ui", "Failed to get git branch");
                        "main".to_string()
                    }
                })
                .unwrap_or_else(|e| {
                    tracing::warn!(target: "phazeai_ui", error = %e, "git command failed");
                    "main".to_string()
                });
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
        let session = SessionState::load();

        // Load editor config from ~/.config/phazeai/config.toml via toml crate.
        let editor_cfg = load_editor_settings();

        let open_file: RwSignal<Option<PathBuf>> = create_rw_signal(session.active_file());
        let open_tabs_sig: RwSignal<Vec<PathBuf>> = create_rw_signal(Vec::new());
        let initial_tabs = session.open_tabs.clone();

        // Start LSP bridge — background tokio thread running LspManager.
        // Must be called in a Floem reactive scope (we're inside the window callback).
        let lsp = start_lsp_bridge(workspace.clone());
        let lsp_cmd = lsp.cmd_tx;
        let diagnostics = lsp.diagnostics;
        let completions = lsp.completions;
        let goto_definition = lsp.goto_definition;
        let hover_text = lsp.hover_text;
        let references = lsp.references;
        let code_actions = lsp.code_actions;
        let _sig_help = lsp.sig_help;
        let doc_symbols = lsp.doc_symbols;
        let _workspace_symbols = lsp.workspace_symbols;
        let lsp_progress = lsp.lsp_progress;
        let peek_def_lines = lsp.peek_def_lines;
        let code_lens = lsp.code_lens;
        let folding_ranges = lsp.folding_ranges;
        let inlay_hints_lsp = lsp.inlay_hints;

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
        // Wire: send LSP did_open when the active file changes.
        // Session persistence is now handled entirely by the unified debounced effect below.
        {
            let lsp_tx = lsp_cmd.clone();
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    // Read file + send LSP did_open in a background thread to avoid
                    // blocking the UI thread with synchronous I/O.
                    let lsp = lsp_tx.clone();
                    let p = path.clone();
                    std::thread::spawn(move || {
                        if let Ok(text) = std::fs::read_to_string(&p) {
                            let _ = lsp.send(LspCommand::OpenFile {
                                path: p.clone(),
                                text,
                            });
                        }
                    });
                }
            });
        }

        // Batch file-open LSP requests in one effect to reduce reactive fanout.
        {
            let lsp_tx = lsp_cmd.clone();
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    let _ = lsp_tx.send(LspCommand::RequestDocumentSymbols { path: path.clone() });
                    let _ = lsp_tx.send(LspCommand::RequestFoldingRanges { path: path.clone() });
                    let _ = lsp_tx.send(LspCommand::RequestCodeLens { path: path.clone() });
                    let _ = lsp_tx.send(LspCommand::RequestInlayHints {
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
                            .unwrap_or_else(|e| {
                                tracing::warn!(target: "phazeai_ui", error = %e, path = %path.display(), "Failed to read file for line ending detection");
                                "LF"
                            });
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

        // ── Sidecar startup ────────────────────────────────────────────────────
        // Locate server.py: first try <exe_dir>/sidecar/server.py, then
        // the repo-relative path, then ~/.config/phazeai/sidecar/server.py.
        let sidecar_ready_sig = create_rw_signal(false);
        let sidecar_status_sig = create_rw_signal(if !settings.sidecar.enabled {
            "Code search disabled in settings.".to_string()
        } else {
            "Code search not started.".to_string()
        });
        let sidecar_building_sig = create_rw_signal(false);
        let sidecar_results_sig: RwSignal<Vec<(String, String)>> = create_rw_signal(Vec::new());
        let sidecar_build_nonce_sig = create_rw_signal(0u64);
        let sidecar_search_nonce_sig = create_rw_signal(0u64);
        let sidecar_query_sig = create_rw_signal(String::new());

        let script_candidates: Vec<PathBuf> = {
            let exe_dir = std::env::current_exe()
                .map(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or(None);
            let mut candidates = vec![
                PathBuf::from("sidecar/server.py"),
                PathBuf::from("../sidecar/server.py"),
            ];
            if let Some(dir) = &exe_dir {
                // Binary-adjacent: <exe>/sidecar/server.py and siblings
                candidates.push(dir.join("sidecar/server.py"));
                candidates.push(dir.join("../sidecar/server.py"));
                candidates.push(dir.join("../../sidecar/server.py"));
                // Installed alongside binary: <exe>/../share/phazeai/sidecar/server.py
                candidates.push(dir.join("../share/phazeai/sidecar/server.py"));
                candidates.push(dir.join("../lib/phazeai/sidecar/server.py"));
            }
            // XDG data dir: ~/.local/share/phazeai/sidecar/server.py
            if let Ok(home_str) = std::env::var("HOME") {
                let home = PathBuf::from(home_str);
                candidates.push(home.join(".local/share/phazeai/sidecar/server.py"));
            }
            // XDG_DATA_HOME override
            if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
                candidates.push(PathBuf::from(xdg).join("phazeai/sidecar/server.py"));
            }
            // Config dir fallback: ~/.config/phazeai/sidecar/server.py
            if let Some(cfg) = dirs_next_config() {
                candidates.push(cfg.join("sidecar/server.py"));
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
                let client_guard = match shared_client_for_build.lock() {
                    Ok(g) => g,
                    Err(e) => {
                        tracing::error!(target: "phazeai_ui", error = %e, "Failed to lock shared client");
                        return;
                    }
                };

                let Some(client) = client_guard.clone() else {
                    drop(client_guard);
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
                };
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
                        let client_guard = match client_cell.lock() {
                            Ok(g) => g,
                            Err(e) => {
                                tracing::error!(target: "phazeai_ui", error = %e, "Failed to lock client for search");
                                let _ = status_tx3
                                    .send("Internal error: failed to access client".to_string());
                                let _ = tx.send(vec![(
                                    "error".to_string(),
                                    "internal error".to_string(),
                                )]);
                                return;
                            }
                        };
                        let client = client_guard.clone();
                        drop(client_guard);

                        let Some(client) = client else {
                            let _ = status_tx3.send(
                                "Code search unavailable. Build the index to start the sidecar."
                                    .to_string(),
                            );
                            let _ = tx.send(vec![(
                                "sidecar unavailable".to_string(),
                                "code search is not connected".to_string(),
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
                            match client.search_code(&query, 8).await {
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
                                        "code index not built yet — click Reindex".to_string()
                                    } else {
                                        format!("code search failed: {e}")
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
                sidecar_status_sig.set("Starting code search...".to_string());
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
                sidecar_status_sig
                    .set("Code search idle. Click Reindex to start and build the index.".into());
            }
        } else {
            sidecar_status_sig.set("Code search sidecar script not found.".to_string());
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

        // Thread-safe editor snapshot consulted by the plugin host. We push
        // updates into it from a create_effect on `open_file` below, so a
        // plugin calling `host.get_active_file_path()` from any thread always
        // sees the current value. Cheap inner RwLock — no signal coupling.
        let editor_snapshot = Arc::new(phazeai_core::ext_host::EditorSnapshot::new());
        {
            let snap = editor_snapshot.clone();
            create_effect(move |_| {
                let path_str = open_file
                    .get()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                snap.set_active_file_path(path_str);
            });
        }

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

        // ── Session-persisted layout signals ─────────────────────────────────
        // Create these before `Self {}` so the debounced-save effect can capture them.
        let show_left_panel_sig = create_rw_signal(session.show_left_panel);
        let show_right_panel_sig = create_rw_signal(session.show_right_panel);
        let show_bottom_panel_sig = create_rw_signal(session.show_bottom_panel);
        let split_editor_sig = create_rw_signal(session.split_editor);
        let split_editor_down_sig = create_rw_signal(session.split_editor_down);
        let vim_mode_sig = create_rw_signal(session.vim_mode);
        let zen_mode_sig = create_rw_signal(session.zen_mode);
        let left_panel_width_sig = create_rw_signal(session.left_panel_width);

        // Debounce cancel token: shared between the effect and spawned threads.
        let session_gen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

        // Unified debounced session persistence effect.
        // Subscribes to all session-relevant signals; collapses rapid changes into a
        // single disk write 1 second after the last change.
        {
            let gen = session_gen.clone();
            create_effect(move |_| {
                SessionState::from_signals(
                    open_tabs_sig.get(),
                    open_file.get(),
                    left_panel_width_sig.get(),
                    show_left_panel_sig.get(),
                    show_right_panel_sig.get(),
                    show_bottom_panel_sig.get(),
                    split_editor_sig.get(),
                    split_editor_down_sig.get(),
                    vim_mode_sig.get(),
                    theme_signal.get().variant.name().to_string(),
                    zen_mode_sig.get(),
                )
                .save_debounced(gen.clone());
            });
        }

        // Channel that off-thread plugin code uses to ask the UI thread to
        // mutate the active editor. Receiver is drained below via a Floem
        // signal-from-channel effect.
        let (editor_cmd_tx, editor_cmd_rx) =
            std::sync::mpsc::sync_channel::<crate::editor_command::EditorCommand>(32);

        let workbench = WorkbenchState {
            theme: theme_signal,
            editor_cmd_tx,
            left_panel_tab: create_rw_signal(Tab::Explorer),
            bottom_panel_tab: create_rw_signal(Tab::Terminal),
            show_left_panel: show_left_panel_sig,
            show_right_panel: show_right_panel_sig,
            show_bottom_panel: show_bottom_panel_sig,
            left_panel_width: left_panel_width_sig,
            zen_mode: zen_mode_sig,
            bottom_panel_maximized: create_rw_signal(false),
            panel_drag_active: create_rw_signal(false),
            panel_drag_start_x: create_rw_signal(0.0),
            command_palette_open: create_rw_signal(false),
            command_palette_query: create_rw_signal(String::new()),
            status_toast: status_toast_sig,
            file_picker_open: create_rw_signal(false),
            file_picker_query: create_rw_signal(String::new()),
            file_picker_files: create_rw_signal(Vec::new()),
            search_query: create_rw_signal(String::new()),
            search_results: create_rw_signal(Vec::new()),
            output_log: create_rw_signal(Vec::new()),
            run_in_terminal_text: create_rw_signal(None),
            debug_console_log: create_rw_signal(String::new()),
            panel_drag_start_width: create_rw_signal(0.0),
            extensions: create_rw_signal(Vec::new()),
            ext_loading: create_rw_signal(false),
            ext_manager: ext_manager.clone(),
            editor_snapshot: editor_snapshot.clone(),
        };

        let editor = EditorState {
            open_file,
            open_tabs: open_tabs_sig,
            active_cursor: create_rw_signal(None),
            vim_mode: vim_mode_sig,
            vim_normal_mode: create_rw_signal(false),
            vim_pending_key: create_rw_signal(None),
            vim_motion: create_rw_signal(None),
            vim_visual_mode: create_rw_signal(false),
            vim_visual_line: create_rw_signal(false),
            vim_marks: create_rw_signal(std::collections::HashMap::new()),
            vim_last_motion: create_rw_signal(None),
            vim_ex_open: create_rw_signal(false),
            vim_ex_input: create_rw_signal(String::new()),
            font_size: font_size_signal,
            tab_size: tab_size_signal,
            auto_save: auto_save_signal,
            word_wrap: word_wrap_signal,
            relative_line_numbers: relative_line_numbers_signal,
            line_ending: line_ending_sig,
            active_readonly: active_readonly_sig,
            diagnostics,
            completions,
            completion_open: create_rw_signal(false),
            completion_selected: create_rw_signal(0),
            completion_filter_text: create_rw_signal(String::new()),
            hover_text,
            goto_line: goto_line_sig,
            goto_definition,
            references,
            references_visible: create_rw_signal(false),
            code_actions,
            code_actions_open: create_rw_signal(false),
            doc_symbols,
            inlay_hints: inlay_hints_lsp,
            code_lens,
            folding_ranges,
            comment_toggle_nonce: create_rw_signal(0),
            ctrl_d_nonce: create_rw_signal(0),
            fold_nonce: create_rw_signal(0),
            unfold_nonce: create_rw_signal(0),
            fold_all_nonce: create_rw_signal(0),
            unfold_all_nonce: create_rw_signal(0),
            move_line_up_nonce: create_rw_signal(0),
            move_line_down_nonce: create_rw_signal(0),
            duplicate_line_nonce: create_rw_signal(0),
            delete_line_nonce: create_rw_signal(0),
            transform_upper_nonce: create_rw_signal(0),
            transform_lower_nonce: create_rw_signal(0),
            transform_title_nonce: create_rw_signal(0),
            join_line_nonce: create_rw_signal(0),
            sort_lines_nonce: create_rw_signal(0),
            format_selection_nonce: create_rw_signal(0),
            split_editor: create_rw_signal(false),
            active_blame: create_rw_signal(String::new()),
            pending_completion: create_rw_signal(None),
            yank_ring: create_rw_signal(Vec::new()),
            yank_ring_idx: create_rw_signal(0),
            split_editor_down: create_rw_signal(false),
            rename_open: create_rw_signal(false),
            rename_query: create_rw_signal(String::new()),
            rename_target: create_rw_signal(String::new()),
            sig_help: create_rw_signal(None),
            ws_syms_open: create_rw_signal(false),
            ws_syms_query: create_rw_signal(String::new()),
            workspace_symbols: create_rw_signal(Vec::new()),
            goto_overlay_open: create_rw_signal(false),
            goto_overlay_input: create_rw_signal(String::new()),
            peek_def_open: create_rw_signal(false),
            peek_def_lines: create_rw_signal(Vec::new()),
            col_cursor_up_nonce: create_rw_signal(0),
            col_cursor_down_nonce: create_rw_signal(0),
            sticky_lines: create_rw_signal(Vec::new()),
            expand_selection_nonce: create_rw_signal(0),
            shrink_selection_nonce: create_rw_signal(0),
            save_no_format_nonce: create_rw_signal(0),
            code_lens_visible: create_rw_signal(true),
            organize_imports_on_save: create_rw_signal(true),
            inlay_hints_sig: create_rw_signal(Vec::new()),
            inlay_hints_toggle: create_rw_signal(true),
            minimap_visible: create_rw_signal(true),
            split_open_file: create_rw_signal(None),
            split_active_cursor: create_rw_signal(None),
            split_open_tabs: create_rw_signal(Vec::new()),
            split_down_file: create_rw_signal(None),
            split_down_cursor: create_rw_signal(None),
            split_down_tabs: create_rw_signal(Vec::new()),
            close_active_tab_nonce: create_rw_signal(0u64),
        };

        let ai = AiState {
            provider: ai_provider_sig,
            model: ai_model_sig,
            thinking: create_rw_signal(false),
            ghost_text: create_rw_signal(None),
            pending_chat_inject: create_rw_signal(None),
            inline_edit_open: create_rw_signal(false),
            inline_edit_query: create_rw_signal(String::new()),
            token_usage_input: create_rw_signal(0),
            token_usage_output: create_rw_signal(0),
        };

        let project = ProjectState {
            workspace_root: create_rw_signal(workspace),
            git_branch,
            sidecar_client: shared_client,
            sidecar_ready: sidecar_ready_sig,
            sidecar_status: sidecar_status_sig,
            sidecar_building: sidecar_building_sig,
            sidecar_results: sidecar_results_sig,
            sidecar_query: sidecar_query_sig,
            sidecar_build_nonce: create_rw_signal(0),
            sidecar_search_nonce: create_rw_signal(0),
            branch_picker_open: create_rw_signal(false),
            branch_list: create_rw_signal(Vec::new()),
            lsp_progress,
            lsp_cmd,
            scratch_paths: create_rw_signal(Vec::new()),
            scratch_counter: create_rw_signal(0),
            initial_tabs: initial_tabs.clone(),
        };

        let state = Self {
            workbench,
            editor,
            ai,
            project,
        };

        // Drain plugin-originated EditorCommands on the UI thread.
        {
            use crate::editor_command::EditorCommand;
            let pending_completion = state.editor.pending_completion;
            let ext_manager = state.workbench.ext_manager.clone();
            let cmd_signal = floem::ext_event::create_signal_from_channel(editor_cmd_rx);
            floem::reactive::create_effect(move |_| {
                if let Some(cmd) = cmd_signal.get() {
                    match cmd {
                        EditorCommand::InsertText(text) => {
                            pending_completion.set(Some((text, 0)));
                        }
                        EditorCommand::ExecuteCommand { cmd, args, reply } => {
                            // Route through the loaded plugins. If no plugin
                            // claims the command id, the manager returns Err
                            // which is forwarded verbatim to the caller.
                            let result = match ext_manager.lock() {
                                Ok(mut mgr) => mgr.execute_command(&cmd, &args),
                                Err(e) => Err(format!("plugin manager lock poisoned: {e}")),
                            };
                            let _ = reply.try_send(result);
                        }
                    }
                }
            });
        }

        state
    }
}

// ── Command palette commands ──────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct PaletteCommand {
    pub label: &'static str,
    pub action: fn(IdeState),
}

pub(crate) fn all_commands() -> Vec<PaletteCommand> {
    vec![
        PaletteCommand {
            label: "Open File…",
            action: |s| {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    s.editor.open_file.set(Some(path));
                    s.workbench.show_left_panel.set(true);
                    s.workbench.left_panel_width.set(260.0);
                }
            },
        },
        PaletteCommand {
            label: "Open Folder…",
            action: |s| {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    s.project.workspace_root.set(folder);
                    // Clear file picker cache so it re-walks on next open
                    s.workbench.file_picker_files.set(Vec::new());
                    s.workbench.show_left_panel.set(true);
                    s.workbench.left_panel_width.set(300.0);
                    s.workbench.left_panel_tab.set(crate::app::Tab::Explorer);
                }
            },
        },
        PaletteCommand {
            label: "Toggle Terminal",
            action: |s| {
                s.workbench.show_bottom_panel.update(|v| *v = !*v);
            },
        },
        PaletteCommand {
            label: "Toggle Explorer",
            action: |s| {
                s.workbench.show_left_panel.update(|v| *v = !*v);
                let open = s.workbench.show_left_panel.get();
                s.workbench
                    .left_panel_width
                    .set(if open { 260.0 } else { 0.0 });
            },
        },
        PaletteCommand {
            label: "Toggle AI Chat",
            action: |s| {
                s.workbench.show_right_panel.update(|v| *v = !*v);
            },
        },
        // ── All 12 themes ────────────────────────────────────────────────────
        PaletteCommand {
            label: "Theme: Midnight Blue",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::MidnightBlue));
            },
        },
        PaletteCommand {
            label: "Theme: Cyberpunk 2077",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Cyberpunk));
            },
        },
        PaletteCommand {
            label: "Theme: Synthwave '84",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Synthwave84));
            },
        },
        PaletteCommand {
            label: "Theme: Andromeda",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Andromeda));
            },
        },
        PaletteCommand {
            label: "Theme: Dark",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Dark));
            },
        },
        PaletteCommand {
            label: "Theme: Dracula",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Dracula));
            },
        },
        PaletteCommand {
            label: "Theme: Tokyo Night",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::TokyoNight));
            },
        },
        PaletteCommand {
            label: "Theme: Monokai",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Monokai));
            },
        },
        PaletteCommand {
            label: "Theme: Nord Dark",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::NordDark));
            },
        },
        PaletteCommand {
            label: "Theme: Matrix Green",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::MatrixGreen));
            },
        },
        PaletteCommand {
            label: "Theme: Root Shell",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::RootShell));
            },
        },
        PaletteCommand {
            label: "Theme: Light",
            action: |s| {
                s.workbench
                    .theme
                    .set(PhazeTheme::from_variant(ThemeVariant::Light));
            },
        },
        PaletteCommand {
            label: "Transform: To Uppercase",
            action: |s| s.editor.transform_upper_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Transform: To Lowercase",
            action: |s| s.editor.transform_lower_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Join Lines",
            action: |s| s.editor.join_line_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Sort Lines (Ascending)",
            action: |s| s.editor.sort_lines_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Toggle Relative Line Numbers",
            action: |s| s.editor.relative_line_numbers.update(|v| *v = !*v),
        },
        PaletteCommand {
            label: "New Scratch File",
            action: |s| {
                let n = s.project.scratch_counter.get() + 1;
                s.project.scratch_counter.set(n);
                let p = std::path::PathBuf::from(format!("scratch://untitled-{n}"));
                s.project
                    .scratch_paths
                    .update(|v: &mut Vec<PathBuf>| v.push(p.clone()));
                s.editor.open_file.set(Some(p));
            },
        },
        PaletteCommand {
            label: "Go to Line/Column",
            action: |s| {
                s.editor.goto_overlay_open.set(true);
                s.editor.goto_overlay_input.set(String::new());
            },
        },
        PaletteCommand {
            label: "Toggle Organize Imports on Save",
            action: |s| s.editor.organize_imports_on_save.update(|v| *v = !*v),
        },
        PaletteCommand {
            label: "Transform: To Title Case",
            action: |s| s.editor.transform_title_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Format Selection",
            action: |s| s.editor.format_selection_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Save Without Formatting",
            action: |s| s.editor.save_no_format_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Fold All",
            action: |s| s.editor.fold_all_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Unfold All",
            action: |s| s.editor.unfold_all_nonce.update(|v| *v += 1),
        },
        PaletteCommand {
            label: "Toggle Code Lens",
            action: |s| s.editor.code_lens_visible.update(|v| *v = !*v),
        },
    ]
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
        cx.fill(&floem::kurbo::Rect::ZERO.with_size(size), p.bg_deep, 0.0);

        if !t.is_cosmic() {
            return;
        }

        // 2. Subtle hex grid — faint accent dots for an "engineered" technical feel
        let hex_size = 40.0;
        let grid_color = p.accent.with_alpha(0.18);
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
    let active = move || {
        state.workbench.left_panel_tab.get() == tab && state.workbench.show_left_panel.get()
    };

    let icon_color = move |p: &crate::theme::PhazePalette| {
        if active() {
            p.accent
        } else {
            p.text_secondary
        }
    };

    container(phaze_icon(
        icon_svg,
        22.0,
        icon_color,
        state.workbench.theme,
    ))
    .style(move |s| {
        let t = state.workbench.theme.get();
        let p = &t.palette;
        let is_active = active();
        let is_hov = is_hovered.get();

        s.width(40.0)
            .height(40.0)
            .border_radius(10.0)
            .items_center()
            .justify_center()
            .cursor(floem::style::CursorStyle::Pointer)
            .margin_bottom(4.0)
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
        if state.workbench.left_panel_tab.get() == tab && state.workbench.show_left_panel.get() {
            state.workbench.show_left_panel.set(false);
            state.workbench.left_panel_width.set(0.0);
        } else {
            state.workbench.left_panel_tab.set(tab);
            state.workbench.show_left_panel.set(true);
            state.workbench.left_panel_width.set(260.0);
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
        activity_bar_btn(icons::COMPOSE, Tab::Composer, state.clone()),
        activity_bar_btn(icons::DEBUG, Tab::Debug, state.clone()),
        activity_bar_btn(icons::REMOTE, Tab::Remote, state.clone()),
        activity_bar_btn(icons::CONTAINER, Tab::Containers, state.clone()),
        activity_bar_btn(icons::LIST_CHECKS, Tab::Makefile, state.clone()),
        activity_bar_btn(icons::GITHUB, Tab::GitHub, state.clone()),
        activity_bar_btn(icons::LIST_CHECKS, Tab::Tests, state.clone()),
        stack((
            activity_bar_btn(icons::EXTENSIONS, Tab::Extensions, state.clone()),
            activity_bar_btn(icons::SETTINGS, Tab::Settings, state.clone()),
            activity_bar_btn(icons::ACCOUNT, Tab::Account, state.clone()),
        ))
        .style(|s| s.flex_col().margin_top(floem::unit::PxPctAuto::Auto)),
    ))
    .style(|s| s.flex_col().padding(8.0).gap(2.0))
    .style(move |s| {
        let t = state.workbench.theme.get();
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

fn left_panel(state: IdeState) -> impl IntoView {
    let explorer = explorer_panel(
        state.project.workspace_root,
        state.editor.open_file,
        state.workbench.theme,
        state.editor.open_tabs,
    );

    let explorer_wrap = container(explorer).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Explorer, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let search_wrap = container(search::search_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Search, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let git_wrap = container(git_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Git, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let debug_wrap = container(run_debug_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Debug, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let extensions_wrap = container(extensions_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full().apply_if(
                state.workbench.left_panel_tab.get() != Tab::Extensions,
                |s| s.display(floem::style::Display::None),
            )
        }
    });

    let remote_wrap = container(remote_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Remote, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let container_wrap = container(containers_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full().apply_if(
                state.workbench.left_panel_tab.get() != Tab::Containers,
                |s| s.display(floem::style::Display::None),
            )
        }
    });

    let makefile_wrap = container(makefile_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Makefile, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let github_wrap = container(github_actions_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::GitHub, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let tests_wrap = container(tests_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Tests, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let symbols_wrap = container(symbol_outline_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Symbols, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let composer_wrap = container(crate::panels::composer::composer_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Composer, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let settings_wrap = container(settings_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Settings, |s| {
                    s.display(floem::style::Display::None)
                })
        }
    });

    let account_wrap = container(account_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full()
                .height_full()
                .apply_if(state.workbench.left_panel_tab.get() != Tab::Account, |s| {
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
            tests_wrap,
            composer_wrap,
            settings_wrap,
            account_wrap,
        ))
        .style(|s| s.width_full().height_full()),
    )
    .style(move |s| {
        let t = state.workbench.theme.get();
        let p = &t.palette;
        let show = state.workbench.show_left_panel.get();
        let width = if show {
            state.workbench.left_panel_width.get()
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
            .box_shadow_blur(16.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
            .apply_if(!show, |s| s.display(floem::style::Display::None))
    })
}

fn bottom_panel_tab(label_str: &'static str, tab: Tab, state: IdeState) -> impl IntoView {
    let is_hovered = create_rw_signal(false);
    container(label(move || label_str))
        .style(move |s| {
            let t = state.workbench.theme.get();
            let p = &t.palette;
            let active = state.workbench.bottom_panel_tab.get() == tab;
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
            state.workbench.bottom_panel_tab.set(tab);
            state.workbench.show_bottom_panel.set(true);
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
            let t = state.workbench.theme.get();
            let p = &t.palette;
            let active = state.workbench.bottom_panel_tab.get() == tab;
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
            state.workbench.bottom_panel_tab.set(tab);
            state.workbench.show_bottom_panel.set(true);
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
    //         let p = state.workbench.theme.get().palette;
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
                phaze_icon(icons::BRANCH, 12.0, move |p| p.accent, state.workbench.theme),
                label(move || format!(" {} ", s.project.git_branch.get())).style(move |s2| {
                    s2.color(state.workbench.theme.get().palette.text_secondary)
                        .font_size(11.0)
                }),
            ))
            .style(|s| s.items_center()),
        )
        .style(move |s| {
            let p = s2.workbench.theme.get().palette;
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
            let picker_open_sig = state.project.branch_picker_open;
            let branch_list_sig = state.project.branch_list;
            create_effect(move |_| {
                if let Some(branches) = branch_sig.get() {
                    branch_list_sig.set(branches);
                    picker_open_sig.set(true);
                }
            });
            move |_| {
                let root = s3.project.workspace_root.get();
                let tx = branch_tx.clone();
                std::thread::spawn(move || {
                    let branches = std::process::Command::new("git")
                        .args(["branch", "--list"])
                        .current_dir(&root)
                        .output()
                        .map(|out| {
                            String::from_utf8_lossy(&out.stdout)
                                .lines()
                                .map(|l| l.trim_start_matches(['*', ' ']).trim().to_string())
                                .filter(|l| !l.is_empty())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_else(|e| {
                            tracing::warn!(target: "phazeai_ui", error = %e, "Failed to list git branches");
                            vec![]
                        });
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
        phaze_icon(
            icons::BRANCH,
            12.0,
            move |p| p.accent,
            state.workbench.theme,
        ),
        label(move || format!(" {}", state.ai.model.get())).style(move |s| {
            s.color(state.workbench.theme.get().palette.text_secondary)
                .font_size(11.0)
        }),
    ))
    .style(|s| s.items_center().padding_horiz(8.0));

    // VIM mode toggle button — shows INSERT/NORMAL when active
    let vim_btn = {
        let s = state.clone();
        let s_label = state.clone();
        container(label(move || {
            if !s_label.editor.vim_mode.get() {
                return "NORMAL".to_string();
            }
            if s_label.editor.vim_normal_mode.get() {
                "-- NORMAL --".to_string()
            } else {
                "-- INSERT --".to_string()
            }
        }))
        .style(move |s2| {
            let p = state.workbench.theme.get().palette;
            let vim = state.editor.vim_mode.get();
            let normal = state.editor.vim_normal_mode.get();
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
            s2.editor.vim_mode.update(|v| *v = !*v);
            // When enabling vim mode, start in Normal mode.
            if s2.editor.vim_mode.get() {
                s2.editor.vim_normal_mode.set(true);
            } else {
                s2.editor.vim_normal_mode.set(false);
            }
            // Session is persisted by the unified debounced effect watching vim_mode.
        })
    };

    let right = stack((
        // Line / column indicator — reads from active_cursor (set by editor on every move).
        label(move || {
            if let Some((_, line, col)) = state.editor.active_cursor.get() {
                format!("Ln {},  Col {}  ", line + 1, col + 1)
            } else {
                String::new()
            }
        })
        .style(move |s| {
            s.color(state.workbench.theme.get().palette.text_secondary)
                .font_size(11.0)
        }),
        // LSP diagnostic counts — live from the reactive diagnostics signal.
        label(move || {
            let diags = state.editor.diagnostics.get();
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
            let p = state.workbench.theme.get().palette;
            let has_errs = state
                .editor
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
            if let Some((ref path, line, _col)) = state.editor.active_cursor.get() {
                let diags = state.editor.diagnostics.get();
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
            let p = state.workbench.theme.get().palette;
            let has_err = state
                .editor
                .active_cursor
                .get()
                .map(|(ref path, line, _)| {
                    state.editor.diagnostics.get().iter().any(|d| {
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
                .project
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
            s.color(state.workbench.theme.get().palette.text_muted)
                .font_size(10.0)
                .apply_if(state.project.lsp_progress.get().is_none(), |s| {
                    s.display(floem::style::Display::None)
                })
        }),
        label(|| "AI Ready  ").style(move |s| {
            s.color(state.workbench.theme.get().palette.success)
                .font_size(11.0)
        }),
        // Git blame for current cursor line
        label(move || {
            let blame = state.editor.active_blame.get();
            if blame.is_empty() {
                String::new()
            } else {
                format!("  {}  ", blame)
            }
        })
        .style(move |s| {
            let p = state.workbench.theme.get().palette;
            s.font_size(10.0)
                .color(p.text_muted)
                .apply_if(state.editor.active_blame.get().is_empty(), |s| {
                    s.display(floem::style::Display::None)
                })
        }),
        // Dynamic encoding + line ending indicator — clickable to toggle CRLF/LF
        {
            let le_state = state.clone();
            let le_theme = state.workbench.theme;
            let le_hov = create_rw_signal(false);
            container(
                label(move || format!("UTF-8 {}  ", le_state.editor.line_ending.get())).style(
                    move |s| {
                        let p = le_theme.get().palette;
                        s.color(if le_hov.get() { p.accent } else { p.text_muted })
                            .font_size(11.0)
                            .cursor(floem::style::CursorStyle::Pointer)
                    },
                ),
            )
            .on_click_stop(move |_| {
                // Toggle line ending and convert file bytes
                let current = state.editor.line_ending.get();
                let new_le: &'static str = if current == "CRLF" { "LF" } else { "CRLF" };
                state.editor.line_ending.set(new_le);
                // Convert open file bytes
                if let Some(path) = state.editor.open_file.get_untracked() {
                    if path.exists() {
                        let toast = state.workbench.status_toast;
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
                .editor
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
            s.color(state.workbench.theme.get().palette.text_muted)
                .font_size(11.0)
        }),
        // Read-only indicator
        {
            let ro_theme = state.workbench.theme;
            let ro_sig = state.editor.active_readonly;
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
        let t = state.workbench.theme.get();
        let p = &t.palette;
        s.height(22.0)
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
    let diags = state.editor.diagnostics;
    let theme = state.workbench.theme;
    let open_file = state.editor.open_file;
    let goto_line = state.editor.goto_line;

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
                let theme = state.workbench.theme;
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
    let refs = state.editor.references;
    let theme = state.workbench.theme;
    let open_file = state.editor.open_file;
    let goto_line = state.editor.goto_line;

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
                let theme = state.workbench.theme;
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
                        .map(|c| {
                            c.lines()
                                .nth(entry.line.saturating_sub(1) as usize)
                                .map(|l| l.trim().to_string())
                                .unwrap_or_default()
                        })
                        .unwrap_or_else(|e| {
                            tracing::debug!(target: "phazeai_ui", error = %e, path = %entry.path.display(), "Failed to read file for snippet");
                            String::new()
                        });

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
    let log = state.workbench.output_log;
    let theme = state.workbench.theme;
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
    let theme = state.workbench.theme;
    let log_sig = state.workbench.debug_console_log;

    let clear_btn = container(label(|| "Clear".to_string()))
        .style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .padding_horiz(10.0)
                .padding_vert(4.0)
                .border_radius(4.0)
                .border(1.0)
                .border_color(p.border)
                .cursor(floem::style::CursorStyle::Pointer)
                .color(p.accent)
        })
        .on_click_stop(move |_| log_sig.set(String::new()));

    let hint = container(label(|| {
        "Output from Makefile / Containers / SSH / Run & Debug presets is echoed here.".to_string()
    }))
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(10.5)
            .color(p.text_muted)
            .padding_left(12.0)
            .flex_grow(1.0)
    });

    let toolbar = stack((hint, clear_btn)).style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .justify_between()
            .width_full()
            .padding_horiz(8.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(p.border)
    });

    let list = scroll(
        dyn_stack(
            move || {
                let text = log_sig.get();
                if text.trim().is_empty() {
                    vec![(
                        0usize,
                        "No output yet — use Makefile, Run & Debug, Containers, or Remote."
                            .to_string(),
                    )]
                } else {
                    text.lines()
                        .enumerate()
                        .map(|(i, line)| (i + 1, line.to_string()))
                        .collect()
                }
            },
            |(i, _)| *i,
            move |(num, txt)| {
                let muted = num == 0 && txt.starts_with("No output yet");
                let lbl = txt.clone();
                container(label(move || {
                    let prefix = if num == 0 {
                        String::new()
                    } else {
                        format!("{:>4} │ ", num)
                    };
                    format!("{}{}", prefix, lbl)
                }))
                .style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(11.5)
                        .color(if muted {
                            p.text_muted
                        } else {
                            p.text_secondary
                        })
                        .font_family(
                            "JetBrains Mono, Fira Code, ui-monospace, monospace".to_string(),
                        )
                        .padding_horiz(10.0)
                        .padding_vert(1.0)
                        .width_full()
                })
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    container(stack((toolbar, list)).style(|s| {
        s.flex_col()
            .width_full()
            .height_full()
            .background(floem::peniko::Color::TRANSPARENT)
    }))
}

fn ports_view(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let lines = create_rw_signal(Vec::<String>::new());
    let err_msg = create_rw_signal(String::new());
    let (ports_tx, ports_rx) = std::sync::mpsc::sync_channel::<Result<Vec<String>, String>>(4);
    let ports_result = create_signal_from_channel(ports_rx);
    create_effect(move |_| {
        if let Some(res) = ports_result.get() {
            match res {
                Ok(ls) => {
                    lines.set(ls);
                    err_msg.set(String::new());
                }
                Err(e) => err_msg.set(e),
            }
        }
    });

    let refresh_lbl = container(label(|| "⟳ Refresh".to_string())).style(move |s| {
        let p = theme.get().palette;
        s.font_size(11.0)
            .padding_horiz(10.0)
            .padding_vert(4.0)
            .border_radius(4.0)
            .border(1.0)
            .border_color(p.border)
            .cursor(floem::style::CursorStyle::Pointer)
            .color(p.accent)
    });

    let hdr = refresh_lbl.on_click_stop(move |_| {
        let tx = ports_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::util::snapshot_listening_ports());
        });
    });

    let hint = container(label(|| {
        "Processes bound to TCP listening sockets (requires `ss` on Linux or `lsof` on macOS)."
            .to_string()
    }))
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(10.5)
            .color(p.text_muted)
            .flex_grow(1.0)
            .padding_right(8.0)
    });

    let toolbar = stack((hint, hdr)).style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .justify_between()
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(p.border)
    });

    let err_label = label(move || err_msg.get()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(11.0)
            .color(p.warning)
            .padding_horiz(12.0)
            .padding_vert(4.0)
            .apply_if(err_msg.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let rows = dyn_stack(
        move || lines.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(i, line)| {
            let ln = line.clone();
            container(label(move || format!("{}. {}", i + 1, ln))).style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.5)
                    .color(p.text_secondary)
                    .font_family("JetBrains Mono, Fira Code, ui-monospace, monospace".to_string())
                    .padding_horiz(10.0)
                    .padding_vert(1.0)
                    .width_full()
            })
        },
    );

    let list = scroll(rows.style(|s| s.flex_col().width_full()))
        .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    container(stack((toolbar, err_label, list)).style(|s| s.flex_col().width_full().height_full()))
}

/// Symbol outline panel — displayed in the left sidebar under the "Symbols" tab.
fn symbol_outline_panel(state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal as crws;
    let symbols = state.editor.doc_symbols;
    let theme = state.workbench.theme;
    let open_file = state.editor.open_file;
    let goto_line = state.editor.goto_line;
    let lsp_cmd = state.project.lsp_cmd.clone();

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
                let theme = state.workbench.theme;
                move |(_, sym): (usize, SymbolEntry)| {
                    let hovered = crws(false);
                    let name = sym.name.clone();
                    let kind = sym.kind.clone();
                    let line_no = sym.line;
                    let indent = sym.depth * 12;

                    let pal = &theme.get().palette;
                    let kind_color = match kind.as_str() {
                        "fn" => pal.syn_keyword,
                        "struct" => pal.syn_type,
                        "enum" => pal.syn_macro,
                        "trait" => pal.syn_function,
                        "impl" => pal.syn_string,
                        "mod" => pal.syn_number,
                        _ => pal.text_muted,
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
    let theme = state.workbench.theme;
    let open_file = state.editor.open_file;

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
                let pal = &theme.get().palette;
                let color = match kind {
                    1 => pal.diff_added_fg,
                    2 => pal.diff_removed_fg,
                    3 => pal.diff_header_fg,
                    _ => pal.text_secondary,
                };
                let bg = match kind {
                    1 => pal.diff_added_bg,
                    2 => pal.diff_removed_bg,
                    3 => pal.diff_header_bg,
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
    let current_tab = state.workbench.bottom_panel_tab;
    let maximized = state.workbench.bottom_panel_maximized;

    container(
        stack((
            // Tab bar — double-click to maximize/restore
            stack((
                bottom_panel_tab("TERMINAL", Tab::Terminal, state.clone()),
                bottom_panel_tab_dyn(
                    {
                        let diags = state.editor.diagnostics;
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
                phaze_icon(
                    icons::CLOSE,
                    12.0,
                    move |p| p.text_muted,
                    state.workbench.theme,
                )
                .style(move |s| {
                    s.margin_left(floem::unit::PxPctAuto::Auto)
                        .padding(4.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                })
                .on_click_stop(move |_| {
                    state.workbench.show_bottom_panel.set(false);
                }),
            ))
            .style(move |s| {
                let t = state.workbench.theme.get();
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
                    state.clone(),
                    state.workbench.run_in_terminal_text,
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
        let t = state.workbench.theme.get();
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
            .apply_if(!state.workbench.show_bottom_panel.get(), |s| {
                s.display(floem::style::Display::None)
            })
    })
}

fn ide_root(state: IdeState) -> impl IntoView {
    let raw_editor = editor_panel(
        state.editor.open_file,
        state.workbench.theme,
        state.ai.thinking,
        state.project.lsp_cmd.clone(),
        state.editor.active_cursor,
        state.editor.pending_completion,
        state.editor.diagnostics,
        state.editor.goto_line,
        state.editor.comment_toggle_nonce,
        state.project.initial_tabs.clone(),
        state.editor.open_tabs,
        state.editor.vim_motion,
        state.ai.ghost_text,
        state.editor.auto_save,
        state.project.workspace_root.get_untracked(),
        state.editor.font_size,
        state.editor.word_wrap,
        state.editor.ctrl_d_nonce,
        state.editor.fold_nonce,
        state.editor.unfold_nonce,
        state.editor.move_line_up_nonce,
        state.editor.move_line_down_nonce,
        state.editor.duplicate_line_nonce,
        state.editor.delete_line_nonce,
        state.editor.active_blame,
        state.editor.col_cursor_up_nonce,
        state.editor.col_cursor_down_nonce,
        state.editor.sticky_lines,
        state.editor.transform_upper_nonce,
        state.editor.transform_lower_nonce,
        state.editor.join_line_nonce,
        state.editor.sort_lines_nonce,
        state.editor.vim_visual_mode,
        state.editor.vim_marks,
        state.editor.vim_last_motion,
        state.editor.expand_selection_nonce,
        state.editor.shrink_selection_nonce,
        state.editor.relative_line_numbers,
        state.editor.yank_ring,
        state.editor.tab_size,
        state.editor.line_ending,
        state.editor.folding_ranges,
        state.editor.transform_title_nonce,
        state.editor.format_selection_nonce,
        state.editor.save_no_format_nonce,
        state.editor.fold_all_nonce,
        state.editor.unfold_all_nonce,
        state.editor.code_lens,
        state.editor.code_lens_visible,
        state.editor.organize_imports_on_save,
        state.editor.inlay_hints_sig,
        state.editor.inlay_hints_toggle,
        state.editor.minimap_visible,
        state.editor.close_active_tab_nonce,
    );

    // ── Split editor (Ctrl+Alt+\) — second independent editor pane ──────────
    let split_raw = editor_panel(
        state.editor.split_open_file,
        state.workbench.theme,
        state.ai.thinking,
        state.project.lsp_cmd.clone(),
        state.editor.split_active_cursor,
        state.editor.pending_completion,
        state.editor.diagnostics,
        create_rw_signal(0u32), // independent goto_line for split pane
        create_rw_signal(0u64), // independent comment nonce
        vec![],                 // no session restore for split pane
        state.editor.split_open_tabs,
        state.editor.vim_motion,
        state.ai.ghost_text,
        state.editor.auto_save,
        state.project.workspace_root.get_untracked(),
        state.editor.font_size,
        state.editor.word_wrap,
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
        state.editor.vim_visual_mode,
        state.editor.vim_marks,
        state.editor.vim_last_motion,
        create_rw_signal(0u64),                     // expand_selection
        create_rw_signal(0u64),                     // shrink_selection
        create_rw_signal(false),                    // relative_line_numbers
        create_rw_signal(Vec::<String>::new()),     // yank_ring
        state.editor.tab_size,                      // tab_size
        state.editor.line_ending,                   // line_ending_out
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
        state.editor.minimap_visible,
        create_rw_signal(0u64), // close_active_tab_nonce (split: no-op)
    );
    let split_pane = container(split_raw).style(move |s| {
        s.flex_grow(1.0)
            .min_width(0.0)
            .min_height(0.0)
            .apply_if(!state.editor.split_editor.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });
    let split_divider = container(floem::views::empty()).style(move |s| {
        let t = state.workbench.theme.get();
        s.width(3.0)
            .height_full()
            .background(t.palette.glass_border)
            .apply_if(!state.editor.split_editor.get(), |s| {
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
                                        s2.editor.pending_completion.set(Some((text, 0)));
                                    }
                                }
                            }))
                            .separator()
                            .entry(MenuItem::new("Go to Definition\tF12").action(move || {
                                if let Some((path, line, col)) = s3.editor.active_cursor.get() {
                                    let _ = s3
                                        .project
                                        .lsp_cmd
                                        .send(LspCommand::RequestDefinition { path, line, col });
                                }
                            }))
                            .entry(MenuItem::new("Find All References\tShift+F12").action(
                                move || {
                                    if let Some((path, line, col)) = s4.editor.active_cursor.get() {
                                        let _ = s4.project.lsp_cmd.send(
                                            LspCommand::RequestReferences { path, line, col },
                                        );
                                        s4.workbench.show_bottom_panel.set(true);
                                        s4.workbench.bottom_panel_tab.set(Tab::References);
                                    }
                                },
                            ))
                            .entry(MenuItem::new("Rename Symbol\tF2").action(move || {
                                s5.editor.rename_open.set(true);
                            }))
                            .entry(MenuItem::new("Code Actions\tCtrl+.").action(move || {
                                if let Some((path, line, col)) = s6.editor.active_cursor.get() {
                                    let _ = s6
                                        .project
                                        .lsp_cmd
                                        .send(LspCommand::RequestCodeActions { path, line, col });
                                }
                            }))
                            .separator()
                            .entry(MenuItem::new("Toggle Comment\tCtrl+/").action(move || {
                                s7.editor.comment_toggle_nonce.update(|v| *v += 1);
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
                                if let Some((ref path, line, _)) =
                                    s_explain.editor.active_cursor.get()
                                {
                                    let fname = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_else(|| "file".to_string());
                                    s_explain.ai.pending_chat_inject.set(Some(format!(
                                        "Explain the code around line {} in {}",
                                        line + 1,
                                        fname
                                    )));
                                    s_explain.workbench.show_right_panel.set(true);
                                }
                            }))
                            .entry(MenuItem::new("🧪 Generate Tests").action(move || {
                                if let Some((ref path, line, _)) =
                                    s_tests.editor.active_cursor.get()
                                {
                                    let fname = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_else(|| "file".to_string());
                                    s_tests.ai.pending_chat_inject.set(Some(format!(
                                        "Generate unit tests for the function at line {} in {}",
                                        line + 1,
                                        fname
                                    )));
                                    s_tests.workbench.show_right_panel.set(true);
                                }
                            }))
                            .entry(MenuItem::new("🔧 Fix with AI").action(move || {
                                if let Some((ref path, line, _)) = s_fix.editor.active_cursor.get()
                                {
                                    let diags = s_fix.editor.diagnostics.get();
                                    let cur_diag = diags
                                        .iter()
                                        .find(|d| d.path == *path && d.line == (line + 1));
                                    if let Some(d) = cur_diag {
                                        s_fix
                                            .ai
                                            .pending_chat_inject
                                            .set(Some(format!("Fix this error: {}", d.message)));
                                        s_fix.workbench.show_right_panel.set(true);
                                    } else {
                                        show_toast(
                                            s_fix.workbench.status_toast,
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
                                        .workbench
                                        .run_in_terminal_text
                                        .set(Some(text.trim().to_string()));
                                    s_run.workbench.show_bottom_panel.set(true);
                                    s_run.workbench.bottom_panel_tab.set(Tab::Terminal);
                                }
                            }))
                            .entry(MenuItem::new("Run File").action(move || {
                                // Build a shell command based on the active file's extension.
                                if let Some(ref path) = s_run_file.editor.open_file.get() {
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
                                    s_run_file.workbench.run_in_terminal_text.set(Some(cmd));
                                    s_run_file.workbench.show_bottom_panel.set(true);
                                    s_run_file.workbench.bottom_panel_tab.set(Tab::Terminal);
                                }
                            }));
                        show_context_menu(menu, None);
                    }
                }
            })
    };

    let chat = chat_panel(
        state.workbench.theme,
        state.ai.thinking,
        state.ai.pending_chat_inject,
        state.project.workspace_root,
        state.project.sidecar_client.clone(),
        state.workbench.status_toast,
    );

    let chat_wrap = container(chat).style(move |s| {
        let t = state.workbench.theme.get();
        let p = &t.palette;
        s.width(320.0)
            .height_full()
            .min_width(320.0)
            .max_width(320.0)
            .background(p.glass_bg)
            .border_left(1.0)
            .border_color(p.glass_border)
            .box_shadow_h_offset(-6.0)
            .box_shadow_v_offset(0.0)
            .box_shadow_blur(16.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
            .apply_if(!state.workbench.show_right_panel.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    // Drag handle between left panel and editor.
    let divider = {
        let style_s = state.clone();
        let down_s = state.clone();
        container(empty())
            .style(move |s| {
                let t = style_s.workbench.theme.get();
                let active = style_s.workbench.panel_drag_active.get();
                let shown = style_s.workbench.show_left_panel.get();
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
                    down_s.workbench.panel_drag_active.set(true);
                    down_s.workbench.panel_drag_start_x.set(pe.pos.x);
                    down_s
                        .workbench
                        .panel_drag_start_width
                        .set(down_s.workbench.left_panel_width.get());
                }
            })
    };

    // Zen mode — hide sidebars / bottom / status bar for distraction-free editing
    let zen = state.workbench.zen_mode;

    let activity_wrap = container(activity_bar(state.clone()))
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));
    let left_wrap = container(left_panel(state.clone()))
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));
    let divider_wrap = container(divider)
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));
    let chat_zen_wrap = container(chat_wrap).style(|s| s);

    // ── Horizontal (down) split editor pane ──────────────────────────────────
    let down_raw = editor_panel(
        state.editor.split_down_file,
        state.workbench.theme,
        state.ai.thinking,
        state.project.lsp_cmd.clone(),
        state.editor.split_down_cursor,
        state.editor.pending_completion,
        state.editor.diagnostics,
        create_rw_signal(0u32),
        create_rw_signal(0u64),
        vec![],
        state.editor.split_down_tabs,
        state.editor.vim_motion,
        state.ai.ghost_text,
        state.editor.auto_save,
        state.project.workspace_root.get_untracked(),
        state.editor.font_size,
        state.editor.word_wrap,
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
        state.editor.vim_visual_mode,
        state.editor.vim_marks,
        state.editor.vim_last_motion,
        create_rw_signal(0u64),
        create_rw_signal(0u64),
        create_rw_signal(false),                    // relative_line_numbers
        create_rw_signal(Vec::<String>::new()),     // yank_ring
        state.editor.tab_size,                      // tab_size
        state.editor.line_ending,                   // line_ending_out
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
        state.editor.minimap_visible,
        create_rw_signal(0u64), // close_active_tab_nonce (split: no-op)
    );
    let down_pane = container(down_raw).style(move |s| {
        s.flex_grow(1.0)
            .min_width(0.0)
            .min_height(0.0)
            .apply_if(!state.editor.split_editor_down.get(), |s| {
                s.display(floem::style::Display::None)
            })
    });
    let down_divider = container(floem::views::empty()).style(move |s| {
        let t = state.workbench.theme.get();
        s.height(3.0)
            .width_full()
            .background(t.palette.glass_border)
            .apply_if(!state.editor.split_editor_down.get(), |s| {
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
    let bottom_panel_max = state.workbench.bottom_panel_maximized;
    let bottom = container(bottom_raw).style(move |s| {
        let s = s.apply_if(zen.get(), |s| s.display(floem::style::Display::None));
        if bottom_panel_max.get() {
            s.flex_grow(10.0).min_height(0.0)
        } else {
            s.height(220.0)
        }
    });

    let state_for_status = state.clone();
    let status_raw = status_bar(state_for_status);
    let status_wrap = container(status_raw)
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));

    stack((content_row, bottom, status_wrap)).style(move |s| {
        let t = state.workbench.theme.get();
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

/// Launch the PhazeAI IDE.
pub fn launch_phaze_ide() {
    // Anonymous telemetry — single fire-and-forget ping, no personal data
    phazeai_core::telemetry::report_launch(phazeai_core::telemetry::AppKind::Ide);

    let is_first_run = !Settings::config_path().exists();
    let settings = Settings::load();

    // Compute readiness items before entering the reactive scope (fast, synchronous).
    let readiness_items: Vec<(&'static str, bool, &'static str)> = if is_first_run {
        vec![
            (
                "AI provider configured",
                check_provider_ready(&settings),
                "Set your API key env var in Settings → AI Provider",
            ),
            (
                "Python available",
                check_python_ready(&settings),
                "Install Python 3 or set sidecar.python_path in settings.toml",
            ),
            (
                "rust-analyzer found",
                check_lsp_ready(),
                "Install rust-analyzer for LSP features (cargo install rust-analyzer)",
            ),
        ]
    } else {
        vec![]
    };

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
                            let active = style_s.workbench.panel_drag_active.get();
                            s.absolute()
                                .inset(0)
                                .z_index(ui_const::Z_DRAG_OVERLAY)
                                .cursor(floem::style::CursorStyle::ColResize)
                                .apply_if(!active, |s| s.display(floem::style::Display::None))
                        })
                        .on_event_stop(EventListener::PointerMove, move |e| {
                            if let Event::PointerMove(pe) = e {
                                let delta = pe.pos.x - move_s.workbench.panel_drag_start_x.get();
                                let new_w = (move_s.workbench.panel_drag_start_width.get() + delta)
                                    .clamp(80.0, 700.0);
                                move_s.workbench.left_panel_width.set(new_w);
                                move_s.workbench.show_left_panel.set(true);
                            }
                        })
                        .on_event_stop(EventListener::PointerUp, move |_| {
                            up_s.workbench.panel_drag_active.set(false);
                        })
                };

                // Root: cosmic canvas + menu bar + IDE + overlays (overlays use z_index)
                let ide_with_menu = stack((menu_bar(state.clone()), ide_root(state.clone())))
                    .style(|s| s.flex_col().width_full().height_full());

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

                // First-run readiness banner — shown once if any checks fail.
                let readiness_banner = {
                    let items = readiness_items.clone();
                    let has_issues = items.iter().any(|(_, ok, _)| !ok);
                    let visible_sig = create_rw_signal(is_first_run && has_issues);
                    let theme = state.workbench.theme;

                    let rows = {
                        let items2 = items.clone();
                        dyn_stack(
                            move || items2.clone().into_iter().enumerate(),
                            |(i, _)| *i,
                            move |(_, (name, ok, hint))| {
                                let status = if ok {
                                    format!("✓  {name}")
                                } else {
                                    format!("✗  {name}")
                                };
                                let detail = if ok {
                                    String::new()
                                } else {
                                    format!("   {hint}")
                                };
                                let is_ok = ok;
                                stack((
                                    label(move || status.clone()).style(move |s| {
                                        let p = theme.get().palette;
                                        let c = if is_ok { p.success } else { p.error };
                                        s.font_size(12.0).color(c)
                                    }),
                                    label(move || detail.clone()).style(move |s| {
                                        let p = theme.get().palette;
                                        s.font_size(10.5).color(p.text_muted).apply_if(is_ok, |s| {
                                            s.display(floem::style::Display::None)
                                        })
                                    }),
                                ))
                                .style(|s| s.flex_col().margin_bottom(6.0))
                            },
                        )
                        .style(|s| s.flex_col())
                    };

                    let title_theme = theme;
                    let header = stack((
                        label(|| "First Run Setup".to_string()).style(move |s| {
                            let p = title_theme.get().palette;
                            s.font_size(13.0).color(p.text_primary)
                        }),
                        container(label(|| "Dismiss".to_string()).style(move |s| {
                            let p = title_theme.get().palette;
                            s.font_size(11.0).color(p.text_muted)
                        }))
                        .on_click_stop(move |_| visible_sig.set(false))
                        .style(|s| {
                            s.padding_horiz(8.0)
                                .padding_vert(3.0)
                                .cursor(floem::style::CursorStyle::Pointer)
                        }),
                    ))
                    .style(|s| {
                        s.items_center()
                            .justify_between()
                            .width_full()
                            .margin_bottom(10.0)
                    });

                    container(stack((header, rows)).style(|s| s.flex_col().width(300.0))).style(
                        move |s| {
                            let p = theme.get().palette;
                            let shown = visible_sig.get();
                            s.absolute()
                                .inset_bottom(60.0)
                                .inset_right(20.0)
                                .z_index(ui_const::Z_TOAST)
                                .padding(16.0)
                                .background(p.bg_elevated)
                                .border_radius(10.0)
                                .border(1.0)
                                .border_color(p.border)
                                .box_shadow_h_offset(0.0)
                                .box_shadow_v_offset(4.0)
                                .box_shadow_blur(20.0)
                                .box_shadow_color(p.glow)
                                .box_shadow_spread(0.0)
                                .apply_if(!shown, |s| s.display(floem::style::Display::None))
                        },
                    )
                };

                stack((
                    cosmic_bg_canvas(state.workbench.theme),
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
                    readiness_banner,    // Z_TOAST(450) — first-run checklist
                    overlays_b,
                ))
                .style(move |s| {
                    let t = state.workbench.theme.get();
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

                            // ── Global shortcut dispatch (unified via execute_command) ──
                            if let Some(cmd) = match_global_shortcut(
                                &key_event.key.logical_key,
                                &key_event.modifiers,
                            ) {
                                execute_command(&cmd, &state);
                                return;
                            }

                            // ── Named keys ───────────────────────────────────────
                            if let Key::Named(ref named) = key_event.key.logical_key {
                                match named {
                                    floem::keyboard::NamedKey::Escape => {
                                        if state.editor.peek_def_open.get() {
                                            state.editor.peek_def_open.set(false);
                                            state.editor.peek_def_lines.set(vec![]);
                                            return;
                                        }
                                        if state.project.branch_picker_open.get() {
                                            state.project.branch_picker_open.set(false);
                                            return;
                                        }
                                        if state.editor.rename_open.get() {
                                            state.editor.rename_open.set(false);
                                            return;
                                        }
                                        if state.editor.sig_help.get().is_some() {
                                            state.editor.sig_help.set(None);
                                            return;
                                        }
                                        if state.editor.code_actions_open.get() {
                                            state.editor.code_actions_open.set(false);
                                            return;
                                        }
                                        if state.ai.inline_edit_open.get() {
                                            state.ai.inline_edit_open.set(false);
                                            state.ai.inline_edit_query.set(String::new());
                                            return;
                                        }
                                        if state.editor.completion_open.get() {
                                            state.editor.completion_open.set(false);
                                            return;
                                        }
                                        if state.workbench.file_picker_open.get() {
                                            state.workbench.file_picker_open.set(false);
                                            state.workbench.file_picker_query.set(String::new());
                                            return;
                                        }
                                        if state.workbench.command_palette_open.get() {
                                            state.workbench.command_palette_open.set(false);
                                            state
                                                .workbench
                                                .command_palette_query
                                                .set(String::new());
                                            return;
                                        }
                                        // Vim: Escape enters Normal mode / exits ex/visual
                                        if state.editor.vim_mode.get() {
                                            if state.editor.vim_ex_open.get() {
                                                state.editor.vim_ex_open.set(false);
                                                state.editor.vim_ex_input.set(String::new());
                                                return;
                                            }
                                            if state.editor.vim_visual_mode.get() {
                                                state.editor.vim_visual_mode.set(false);
                                            }
                                            state.editor.vim_normal_mode.set(true);
                                            state.editor.vim_pending_key.set(None);
                                            return;
                                        }
                                    }
                                    floem::keyboard::NamedKey::Tab => {
                                        // Tab accepts ghost text (FIM) suggestion first.
                                        if let Some(suggestion) = state.ai.ghost_text.get() {
                                            // Ghost text: insert at cursor, no prefix to delete.
                                            state
                                                .editor
                                                .pending_completion
                                                .set(Some((suggestion, 0)));
                                            state.ai.ghost_text.set(None);
                                            return;
                                        }
                                        // Tab also accepts LSP completion popup.
                                        if state.editor.completion_open.get() {
                                            let items = state.editor.completions.get();
                                            let sel = state.editor.completion_selected.get();
                                            let prefix_b =
                                                state.editor.completion_filter_text.get().len();
                                            if let Some(entry) = items.get(sel) {
                                                let text = if entry.insert_text.is_empty() {
                                                    entry.label.clone()
                                                } else {
                                                    entry.insert_text.clone()
                                                };
                                                state
                                                    .editor
                                                    .pending_completion
                                                    .set(Some((text, prefix_b)));
                                            }
                                            state.editor.completion_open.set(false);
                                            state.editor.completion_filter_text.set(String::new());
                                            return;
                                        }
                                    }
                                    floem::keyboard::NamedKey::Enter => {
                                        if state.editor.completion_open.get() {
                                            let items = state.editor.completions.get();
                                            let sel = state.editor.completion_selected.get();
                                            let prefix_b =
                                                state.editor.completion_filter_text.get().len();
                                            if let Some(entry) = items.get(sel) {
                                                let text = if entry.insert_text.is_empty() {
                                                    entry.label.clone()
                                                } else {
                                                    entry.insert_text.clone()
                                                };
                                                state
                                                    .editor
                                                    .pending_completion
                                                    .set(Some((text, prefix_b)));
                                            }
                                            state.editor.completion_open.set(false);
                                            state.editor.completion_filter_text.set(String::new());
                                            return;
                                        }
                                    }
                                    // F12 — go to definition; Shift+F12 — find all references; Alt+F12 — peek definition; Ctrl+F12 — go to implementation
                                    floem::keyboard::NamedKey::F12 => {
                                        if let Some((path, line, col)) =
                                            state.editor.active_cursor.get()
                                        {
                                            if ctrl {
                                                // Ctrl+F12: go to implementation
                                                let _ = state.project.lsp_cmd.send(
                                                    LspCommand::RequestImplementation {
                                                        path,
                                                        line,
                                                        col,
                                                    },
                                                );
                                            } else if shift {
                                                // Shift+F12: find all references
                                                let _ = state.project.lsp_cmd.send(
                                                    LspCommand::RequestReferences {
                                                        path,
                                                        line,
                                                        col,
                                                    },
                                                );
                                                state.editor.references_visible.set(true);
                                                state.workbench.show_bottom_panel.set(true);
                                                state
                                                    .workbench
                                                    .bottom_panel_tab
                                                    .set(Tab::References);
                                            } else if alt {
                                                // Alt+F12: peek definition
                                                state.editor.peek_def_lines.set(vec![]);
                                                state.editor.peek_def_open.set(false);
                                                let _ = state.project.lsp_cmd.send(
                                                    LspCommand::RequestPeekDefinition {
                                                        path,
                                                        line,
                                                        col,
                                                    },
                                                );
                                            } else {
                                                // F12: go to definition
                                                let _ = state.project.lsp_cmd.send(
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
                                                state.editor.active_cursor.get()
                                            {
                                                let _ = state.project.lsp_cmd.send(
                                                    LspCommand::RequestHover { path, line, col },
                                                );
                                            }
                                            return;
                                        }
                                    }
                                    // F2 — rename symbol at cursor
                                    floem::keyboard::NamedKey::F2 => {
                                        if let Some((path, line, col)) =
                                            state.editor.active_cursor.get()
                                        {
                                            let word = std::fs::read_to_string(&path)
                                                .ok()
                                                .and_then(|content| {
                                                    let target_line =
                                                        content.lines().nth(line as usize)?;
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
                                                        .map(|(i, _)| i + col)
                                                        .unwrap_or(target_line.len());
                                                    Some(target_line[start..end].to_string())
                                                })
                                                .unwrap_or_default();
                                            state.editor.rename_target.set(word.clone());
                                            state.editor.rename_query.set(word);
                                            state.editor.rename_open.set(true);
                                        }
                                        return;
                                    }
                                    // Alt+Up/Down — move or duplicate line
                                    floem::keyboard::NamedKey::ArrowUp
                                        if alt && !ctrl && !shift =>
                                    {
                                        state.editor.move_line_up_nonce.update(|n| *n += 1);
                                        return;
                                    }
                                    floem::keyboard::NamedKey::ArrowDown if alt && !ctrl => {
                                        if shift {
                                            state.editor.duplicate_line_nonce.update(|n| *n += 1);
                                        } else {
                                            state.editor.move_line_down_nonce.update(|n| *n += 1);
                                        }
                                        return;
                                    }
                                    // Ctrl+Alt+Up/Down — add column cursor on adjacent line
                                    floem::keyboard::NamedKey::ArrowUp if ctrl && alt && !shift => {
                                        state.editor.col_cursor_up_nonce.update(|n| *n += 1);
                                        return;
                                    }
                                    floem::keyboard::NamedKey::ArrowDown
                                        if ctrl && alt && !shift =>
                                    {
                                        state.editor.col_cursor_down_nonce.update(|n| *n += 1);
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
                                if let Some((path, line, col)) = state.editor.active_cursor.get() {
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
                                    state.editor.completion_filter_text.set(prefix);
                                    let _ = state
                                        .project
                                        .lsp_cmd
                                        .send(LspCommand::RequestCompletions { path, line, col });
                                }
                                state.editor.completion_selected.set(0);
                                state.editor.completion_open.set(true);
                                return;
                            }

                            // Ctrl+G → goto line/col overlay
                            if ctrl
                                && !shift
                                && !alt
                                && key_event.key.logical_key == Key::Character("g".into())
                            {
                                state.editor.goto_overlay_open.set(true);
                                state.editor.goto_overlay_input.set(String::new());
                                return;
                            }

                            // Ctrl+Alt+S → save without formatting
                            if ctrl
                                && !shift
                                && alt
                                && key_event.key.logical_key == Key::Character("s".into())
                            {
                                state.editor.save_no_format_nonce.update(|v| *v += 1);
                                return;
                            }

                            // Ctrl+Alt+I → toggle inlay hints
                            if ctrl
                                && !shift
                                && alt
                                && key_event.key.logical_key == Key::Character("i".into())
                            {
                                state.editor.inlay_hints_toggle.update(|v| *v = !*v);
                                let msg = if state.editor.inlay_hints_toggle.get() {
                                    "Inlay Hints: on"
                                } else {
                                    "Inlay Hints: off"
                                };
                                show_toast(state.workbench.status_toast, msg);
                                return;
                            }

                            // Ctrl+N → new scratch file (untitled buffer)
                            if ctrl
                                && !shift
                                && !alt
                                && key_event.key.logical_key == Key::Character("n".into())
                            {
                                let n = state.project.scratch_counter.get() + 1;
                                state.project.scratch_counter.set(n);
                                let scratch_path =
                                    std::path::PathBuf::from(format!("scratch://untitled-{n}"));
                                state
                                    .project
                                    .scratch_paths
                                    .update(|v: &mut Vec<PathBuf>| v.push(scratch_path.clone()));
                                state.editor.open_file.set(Some(scratch_path));
                                return;
                            }

                            // Ctrl+T → workspace symbols overlay
                            if ctrl
                                && !shift
                                && key_event.key.logical_key == Key::Character("t".into())
                            {
                                let open = state.editor.ws_syms_open.get();
                                state.editor.ws_syms_open.set(!open);
                                if !open {
                                    state.editor.ws_syms_query.set(String::new());
                                    // Kick off an empty-query search to pre-populate list.
                                    let _ = state.project.lsp_cmd.send(
                                        LspCommand::RequestWorkspaceSymbols {
                                            query: String::new(),
                                        },
                                    );
                                }
                                return;
                            }

                            // Ctrl+. → code actions
                            if ctrl && key_event.key.logical_key == Key::Character(".".into()) {
                                if let Some((path, line, col)) = state.editor.active_cursor.get() {
                                    let _ = state
                                        .project
                                        .lsp_cmd
                                        .send(LspCommand::RequestCodeActions { path, line, col });
                                }
                                state.editor.code_actions_open.set(true);
                                return;
                            }

                            // Ctrl+Shift+Space → signature help
                            if ctrl
                                && shift
                                && key_event.key.logical_key
                                    == Key::Named(floem::keyboard::NamedKey::Space)
                            {
                                if let Some((path, line, col)) = state.editor.active_cursor.get() {
                                    let _ = state
                                        .project
                                        .lsp_cmd
                                        .send(LspCommand::RequestSignatureHelp { path, line, col });
                                }
                                return;
                            }

                            if let Key::Character(ref ch) = key_event.key.logical_key {
                                let ch = ch.clone();

                                // Alt+Z — toggle word wrap
                                if alt && !ctrl && !shift && ch.as_str() == "z" {
                                    state.editor.word_wrap.update(|v| *v = !*v);
                                    let msg = if state.editor.word_wrap.get() {
                                        "Word wrap on"
                                    } else {
                                        "Word wrap off"
                                    };
                                    show_toast(state.workbench.status_toast, msg);
                                    return;
                                }

                                if ctrl && !shift && !alt {
                                    match ch.as_str() {
                                        // Ctrl+= / Ctrl++ — zoom in editor font
                                        "=" | "+" => {
                                            state
                                                .editor
                                                .font_size
                                                .update(|v| *v = (*v + 1).min(40));
                                            return;
                                        }
                                        // Ctrl+- — zoom out editor font
                                        "-" => {
                                            state
                                                .editor
                                                .font_size
                                                .update(|v| *v = v.saturating_sub(1).max(8));
                                            return;
                                        }
                                        // Ctrl+0 — reset editor font to default
                                        "0" => {
                                            state.editor.font_size.set(14);
                                            return;
                                        }
                                        // Ctrl+D — vim half-page down OR multi-cursor
                                        "d" => {
                                            if state.editor.vim_mode.get()
                                                && state.editor.vim_normal_mode.get()
                                            {
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::HalfPageDown));
                                            } else {
                                                state.editor.ctrl_d_nonce.update(|v| *v += 1);
                                            }
                                            return;
                                        }
                                        // Ctrl+U — vim half-page up
                                        "u" => {
                                            if state.editor.vim_mode.get()
                                                && state.editor.vim_normal_mode.get()
                                            {
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::HalfPageUp));
                                                return;
                                            }
                                        }
                                        // Ctrl+K — open inline AI edit overlay
                                        "k" => {
                                            state.ai.inline_edit_open.set(true);
                                            state.ai.inline_edit_query.set(String::new());
                                        }
                                        // Ctrl+/ — toggle line comment
                                        "/" => {
                                            state.editor.comment_toggle_nonce.update(|v| *v += 1);
                                        }
                                        _ => {}
                                    }
                                }

                                // Ctrl+Shift+Z is handled above by match_global_shortcut → execute_command.
                                if ctrl && shift && !alt {
                                    // Ctrl+Shift+[ → fold block at cursor
                                    if ch.as_str() == "[" {
                                        state.editor.fold_nonce.update(|v| *v += 1);
                                        show_toast(state.workbench.status_toast, "Folded");
                                        return;
                                    }
                                    // Ctrl+Shift+] → unfold block at cursor
                                    if ch.as_str() == "]" {
                                        state.editor.unfold_nonce.update(|v| *v += 1);
                                        show_toast(state.workbench.status_toast, "Unfolded");
                                        return;
                                    }
                                    // Ctrl+Shift+K → delete entire line
                                    if ch.as_str() == "k" {
                                        state.editor.delete_line_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+U → transform uppercase
                                    if ch.as_str() == "u" {
                                        state.editor.transform_upper_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+L → transform lowercase
                                    if ch.as_str() == "l" {
                                        state.editor.transform_lower_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+T → transform title case
                                    if ch.as_str() == "t" {
                                        state.editor.transform_title_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+J → join lines
                                    if ch.as_str() == "j" {
                                        state.editor.join_line_nonce.update(|v| *v += 1);
                                        return;
                                    }
                                    // Ctrl+Shift+V → cycle yank ring and paste
                                    if ch.as_str() == "v" {
                                        let ring = state.editor.yank_ring.get();
                                        if !ring.is_empty() {
                                            let idx =
                                                (state.editor.yank_ring_idx.get() + 1) % ring.len();
                                            state.editor.yank_ring_idx.set(idx);
                                            let text = ring[idx].clone();
                                            state.editor.pending_completion.set(Some((text, 0)));
                                        }
                                        return;
                                    }
                                }

                                // Ctrl+Alt+Shift+D → split editor down toggle
                                if ctrl && alt && shift && ch.as_str() == "d" {
                                    state.editor.split_editor_down.update(|v| *v = !*v);
                                    return;
                                }

                                // ── Vim normal-mode keys (no Ctrl) ───────────────
                                if state.editor.vim_mode.get()
                                    && state.editor.vim_normal_mode.get()
                                    && !ctrl
                                    && !alt
                                {
                                    let pending = state.editor.vim_pending_key.get();
                                    let ch_str = ch.as_str();

                                    // Two-key sequences
                                    if let Some(prev) = pending {
                                        state.editor.vim_pending_key.set(None);
                                        match (prev, ch_str) {
                                            ('d', "d") => {
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::DeleteLine));
                                                state
                                                    .editor
                                                    .vim_last_motion
                                                    .set(Some(VimMotion::DeleteLine));
                                            }
                                            ('g', "g") => {
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::GotoFileTop));
                                            }
                                            ('y', "y") => {
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::YankLine));
                                            }
                                            ('c', "c") => {
                                                state.editor.vim_normal_mode.set(false);
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::ChangeWholeLine));
                                                state
                                                    .editor
                                                    .vim_last_motion
                                                    .set(Some(VimMotion::ChangeWholeLine));
                                            }
                                            ('c', "w") => {
                                                state.editor.vim_normal_mode.set(false);
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::ChangeWord));
                                                state
                                                    .editor
                                                    .vim_last_motion
                                                    .set(Some(VimMotion::ChangeWord));
                                            }
                                            ('r', _) => {
                                                if let Some(c) = ch_str.chars().next() {
                                                    state
                                                        .editor
                                                        .vim_motion
                                                        .set(Some(VimMotion::ReplaceChar(c)));
                                                    state
                                                        .editor
                                                        .vim_last_motion
                                                        .set(Some(VimMotion::ReplaceChar(c)));
                                                }
                                            }
                                            ('m', _) => {
                                                if let Some(c) = ch_str.chars().next() {
                                                    state
                                                        .editor
                                                        .vim_motion
                                                        .set(Some(VimMotion::SetMark(c)));
                                                }
                                            }
                                            ('`', _) => {
                                                if let Some(c) = ch_str.chars().next() {
                                                    state
                                                        .editor
                                                        .vim_motion
                                                        .set(Some(VimMotion::GotoMark(c)));
                                                }
                                            }
                                            _ => {}
                                        }
                                        return;
                                    }

                                    // Visual mode intercepts d/y/c to operate on selection
                                    if state.editor.vim_visual_mode.get_untracked() {
                                        match ch_str {
                                            "d" | "x" => {
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::DeleteVisualSelection));
                                                state.editor.vim_visual_mode.set(false);
                                                state
                                                    .editor
                                                    .vim_last_motion
                                                    .set(Some(VimMotion::DeleteVisualSelection));
                                                return;
                                            }
                                            "y" => {
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::YankVisualSelection));
                                                state.editor.vim_visual_mode.set(false);
                                                return;
                                            }
                                            "c" => {
                                                state.editor.vim_visual_mode.set(false);
                                                state.editor.vim_normal_mode.set(false);
                                                state
                                                    .editor
                                                    .vim_motion
                                                    .set(Some(VimMotion::ChangeVisualSelection));
                                                state
                                                    .editor
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
                                            state.editor.vim_motion.set(Some(VimMotion::Left));
                                        }
                                        "j" => {
                                            state.editor.vim_motion.set(Some(VimMotion::Down));
                                        }
                                        "k" => {
                                            state.editor.vim_motion.set(Some(VimMotion::Up));
                                        }
                                        "l" => {
                                            state.editor.vim_motion.set(Some(VimMotion::Right));
                                        }
                                        "w" => {
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::WordForward));
                                        }
                                        "b" => {
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::WordBackward));
                                        }
                                        "0" => {
                                            state.editor.vim_motion.set(Some(VimMotion::LineStart));
                                        }
                                        "$" => {
                                            state.editor.vim_motion.set(Some(VimMotion::LineEnd));
                                        }
                                        "x" => {
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::DeleteChar));
                                        }
                                        "i" => {
                                            state.editor.vim_normal_mode.set(false);
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::EnterInsert));
                                        }
                                        "a" => {
                                            state.editor.vim_normal_mode.set(false);
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::EnterInsertAfter));
                                        }
                                        "o" => {
                                            state.editor.vim_normal_mode.set(false);
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::EnterInsertNewlineBelow));
                                        }
                                        // p / P — paste from vim register
                                        "p" => {
                                            state.editor.vim_motion.set(Some(VimMotion::Paste));
                                        }
                                        "P" => {
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::PasteBefore));
                                        }
                                        // G — go to end of file
                                        "G" => {
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::GotoFileBottom));
                                        }
                                        // A — insert at end of line
                                        "A" => {
                                            state.editor.vim_normal_mode.set(false);
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::InsertAtLineEnd));
                                        }
                                        // I — insert at start of line
                                        "I" => {
                                            state.editor.vim_normal_mode.set(false);
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::InsertAtLineStart));
                                        }
                                        // C — change to end of line (delete + insert)
                                        "C" => {
                                            state.editor.vim_normal_mode.set(false);
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::ChangeToLineEnd));
                                        }
                                        // D — delete to end of line
                                        "D" => {
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::DeleteToLineEnd));
                                            state
                                                .editor
                                                .vim_last_motion
                                                .set(Some(VimMotion::DeleteToLineEnd));
                                        }
                                        // % — jump to matching bracket
                                        "%" => {
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::JumpMatchingBracket));
                                        }
                                        // v — start char-wise visual mode
                                        "v" => {
                                            state.editor.vim_visual_mode.set(true);
                                            state.editor.vim_visual_line.set(false);
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::VisualCharStart));
                                        }
                                        // V — start line-wise visual mode
                                        "V" => {
                                            state.editor.vim_visual_mode.set(true);
                                            state.editor.vim_visual_line.set(true);
                                            state
                                                .editor
                                                .vim_motion
                                                .set(Some(VimMotion::VisualLineStart));
                                        }
                                        // Escape in visual mode — return to normal
                                        // (handled in NamedKey::Escape section below)
                                        // . — repeat last change
                                        "." => {
                                            if let Some(last) = state.editor.vim_last_motion.get() {
                                                state.editor.vim_motion.set(Some(last));
                                            }
                                        }
                                        // : — open ex command bar
                                        ":" => {
                                            state.editor.vim_ex_open.set(true);
                                            state.editor.vim_ex_input.set(String::new());
                                        }
                                        // d, g, y, c, r, m, ` — pending keys for two-key sequences
                                        "d" | "g" | "y" | "c" | "r" | "m" | "`" => {
                                            if let Some(ch) = ch_str.chars().next() {
                                                state.editor.vim_pending_key.set(Some(ch));
                                            }
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
                        if let Ok(guard) = state.project.sidecar_client.lock() {
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
                        // Synchronous save on close — bypasses the debounce so the
                        // final state is never lost.
                        SessionState::from_ide_state_untracked(&state).save();
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
