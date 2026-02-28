use std::path::PathBuf;

use arboard;
use floem::{
    action::show_context_menu,
    event::{Event, EventListener},
    ext_event::{create_ext_action, create_signal_from_channel},
    keyboard::{Key, Modifiers, NamedKey},
    menu::{Menu, MenuItem},
    peniko::kurbo::Size,
    reactive::{create_effect, create_rw_signal, RwSignal, Scope, SignalGet, SignalUpdate},
    views::{canvas, container, dyn_stack, empty, label, scroll, stack, text_input, Decorators},
    window::WindowConfig,
    Application, IntoView, Renderer,
};
use phazeai_core::{Agent, AgentEvent, Settings};
use phazeai_core::config::LlmProvider;

use crate::lsp_bridge::{
    start_lsp_bridge, CodeAction, CompletionEntry, DefinitionResult, DiagEntry, DiagSeverity,
    LspCommand, ReferenceEntry, SymbolEntry,
};

use crate::{
    components::icon::{icons, phaze_icon},
    panels::{
        ai_panel::ai_panel,
        chat::chat_panel,
        editor::editor_panel,
        explorer::explorer_panel,
        git::git_panel,
        search,
        settings::settings_panel,
        terminal::terminal_panel,
    },
    theme::{PhazeTheme, ThemeVariant},
};
use std::time::Duration;

/// Vim normal-mode motions dispatched to the active editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimMotion {
    // Movement
    Left, Right, Up, Down,
    WordForward, WordBackward,
    LineStart, LineEnd,
    // Edit
    DeleteLine,
    DeleteChar,
    // Yank / Paste (vim register)
    YankLine,
    Paste,
    PasteBefore,
    // Mode
    EnterInsert,
    EnterInsertAfter,
    EnterInsertNewlineBelow,
}

/// Global IDE state shared across all panels via Floem reactive system.
#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
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
    /// Rendered width of the left sidebar (260.0 when open, 0.0 when closed).
    /// TODO: drive this through a smooth animation loop once Floem exposes a
    /// stable `create_animation` / `spring` API.  For now it snaps immediately.
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
}

/// Persisted layout state from ~/.config/phazeai/session.toml
struct SessionData {
    /// All open tab paths (replaces the old single open_file).
    open_tabs: Vec<PathBuf>,
    /// The active (focused) tab path, if any.
    open_file: Option<PathBuf>,
    left_panel_width: f64,
    show_bottom_panel: bool,
    vim_mode: bool,
    theme: String,
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            open_tabs: Vec::new(),
            open_file: None,
            left_panel_width: 300.0,
            show_bottom_panel: false,
            vim_mode: false,
            theme: "Midnight Blue".to_string(),
        }
    }
}

/// Load session from `~/.config/phazeai/session.toml`.
fn session_load() -> SessionData {
    let Some(dir) = dirs_next_config() else { return SessionData::default() };
    let Ok(text) = std::fs::read_to_string(dir.join("session.toml")) else {
        return SessionData::default();
    };
    let mut data = SessionData::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("open_file = ") {
            let path = PathBuf::from(rest.trim().trim_matches('"'));
            if path.exists() { data.open_file = Some(path); }
        } else if let Some(rest) = line.strip_prefix("open_tab = ") {
            let path = PathBuf::from(rest.trim().trim_matches('"'));
            if path.exists() { data.open_tabs.push(path); }
        } else if let Some(rest) = line.strip_prefix("left_panel_width = ") {
            if let Ok(v) = rest.trim().parse::<f64>() { data.left_panel_width = v; }
        } else if let Some(rest) = line.strip_prefix("show_bottom_panel = ") {
            data.show_bottom_panel = rest.trim() == "true";
        } else if let Some(rest) = line.strip_prefix("vim_mode = ") {
            data.vim_mode = rest.trim() == "true";
        } else if let Some(rest) = line.strip_prefix("theme = ") {
            data.theme = rest.trim().trim_matches('"').to_string();
        }
    }
    data
}

/// Save session to `~/.config/phazeai/session.toml`.
fn session_save_full(open_file: Option<&PathBuf>, left_panel_width: f64,
                     show_bottom_panel: bool, vim_mode: bool, theme: &str) {
    let Some(dir) = dirs_next_config() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let file_str = open_file
        .map(|p| format!("{:?}", p.to_string_lossy()))
        .unwrap_or_default();
    let content = format!(
        "open_file = {file_str}\nleft_panel_width = {left_panel_width}\n\
         show_bottom_panel = {show_bottom_panel}\nvim_mode = {vim_mode}\n\
         theme = \"{theme}\"\n"
    );
    let _ = std::fs::write(dir.join("session.toml"), content);
}

/// Save the full set of open tabs plus the active file.
fn session_save_tabs(tabs: &[PathBuf], active: Option<&PathBuf>,
                     left_panel_width: f64, show_bottom_panel: bool,
                     vim_mode: bool, theme: &str) {
    let Some(dir) = dirs_next_config() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let file_str = active
        .map(|p| format!("{:?}", p.to_string_lossy()))
        .unwrap_or_default();
    let mut content = format!(
        "open_file = {file_str}\nleft_panel_width = {left_panel_width}\n\
         show_bottom_panel = {show_bottom_panel}\nvim_mode = {vim_mode}\n\
         theme = \"{theme}\"\n"
    );
    for tab in tabs {
        content.push_str(&format!("open_tab = {:?}\n", tab.to_string_lossy()));
    }
    let _ = std::fs::write(dir.join("session.toml"), content);
}

fn dirs_next_config() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))?;
    Some(home.join(".config").join("phazeai"))
}

/// Convert a provider display name back to LlmProvider enum.
fn provider_name_to_llm_provider(name: &str) -> LlmProvider {
    match name {
        "Claude (Anthropic)" => LlmProvider::Claude,
        "OpenAI"             => LlmProvider::OpenAI,
        "Google Gemini"      => LlmProvider::Gemini,
        "Groq"               => LlmProvider::Groq,
        "Together.ai"        => LlmProvider::Together,
        "OpenRouter"         => LlmProvider::OpenRouter,
        "LM Studio (Local)"  => LlmProvider::LmStudio,
        _                    => LlmProvider::Ollama,  // Ollama (Local) + default
    }
}

/// Save editor/theme settings to `~/.config/phazeai/config.toml`.
pub fn save_settings(theme_name: &str, font_size: u32, tab_size: u32) {
    let Some(dir) = dirs_next_config() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let content = format!(
        "[editor]\ntheme = \"{theme_name}\"\nfont_size = {font_size}\ntab_size = {tab_size}\n"
    );
    let _ = std::fs::write(dir.join("config.toml"), content);
}

/// Show a toast notification that auto-dismisses after 3 seconds.
/// Safe to call from any code that has access to `IdeState`.
pub fn show_toast(toast: RwSignal<Option<String>>, msg: impl Into<String>) {
    use floem::ext_event::create_ext_action;
    toast.set(Some(msg.into()));
    let scope = Scope::new();
    let dismiss = create_ext_action(scope, move |_| toast.set(None));
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        dismiss(());
    });
}

/// Load editor config from `~/.config/phazeai/config.toml`.
/// Returns `(font_size, tab_size)` with defaults of (14, 4) if missing.
fn load_editor_config() -> (u32, u32) {
    let Some(dir) = dirs_next_config() else { return (14, 4) };
    let Ok(text) = std::fs::read_to_string(dir.join("config.toml")) else { return (14, 4) };
    let mut font_size = 14u32;
    let mut tab_size = 4u32;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("font_size = ") {
            if let Ok(v) = rest.trim().parse::<u32>() { font_size = v; }
        } else if let Some(rest) = line.strip_prefix("tab_size = ") {
            if let Ok(v) = rest.trim().parse::<u32>() { tab_size = v; }
        }
    }
    (font_size, tab_size)
}

impl IdeState {
    pub fn new(settings: &Settings) -> Self {
        let _theme = PhazeTheme::from_str(&settings.editor.theme);
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

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

        // Load editor config (font_size, tab_size) from ~/.config/phazeai/config.toml.
        let (saved_font_size, saved_tab_size) = load_editor_config();

        let open_file: RwSignal<Option<PathBuf>> = create_rw_signal(session.open_file.clone());
        let open_tabs_sig: RwSignal<Vec<PathBuf>> = create_rw_signal(Vec::new());
        let initial_tabs = session.open_tabs.clone();

        // Start LSP bridge — background tokio thread running LspManager.
        // Must be called in a Floem reactive scope (we're inside the window callback).
        let (lsp_cmd, diagnostics, completions, goto_definition, hover_text, references, code_actions, sig_help, doc_symbols, workspace_symbols) =
            start_lsp_bridge(workspace.clone());

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
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        let _ = lsp_tx.send(LspCommand::OpenFile { path: path.clone(), text });
                    }
                    let current = session_load();
                    let all_tabs = tabs_for_save.get_untracked();
                    session_save_tabs(
                        &all_tabs,
                        Some(&path),
                        current.left_panel_width,
                        current.show_bottom_panel,
                        current.vim_mode,
                        &current.theme,
                    );
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

        // Detect line-ending style when the active file changes.
        let line_ending_sig: RwSignal<&'static str> = create_rw_signal("LF");
        {
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    // Read raw bytes to distinguish CRLF vs LF vs Mixed.
                    let style = std::fs::read(&path)
                        .ok()
                        .map(|bytes| {
                            let crlf_count = bytes.windows(2).filter(|w| *w == b"\r\n").count();
                            let lf_count   = bytes.iter().filter(|&&b| b == b'\n').count();
                            if crlf_count > 0 && lf_count > crlf_count {
                                "Mixed"
                            } else if crlf_count > 0 {
                                "CRLF"
                            } else {
                                "LF"
                            }
                        })
                        .unwrap_or("LF");
                    line_ending_sig.set(style);
                }
            });
        }

        // Create persistent settings signals before Self so we can wire save effects.
        let theme_signal = create_rw_signal(PhazeTheme::from_str(&session.theme));
        let font_size_signal = create_rw_signal(saved_font_size);
        let tab_size_signal = create_rw_signal(saved_tab_size);

        // Whenever theme, font_size, or tab_size changes, persist to config.toml.
        create_effect(move |_| {
            let theme_name = theme_signal.get().variant.name().to_string();
            let fs = font_size_signal.get();
            let ts = tab_size_signal.get();
            save_settings(&theme_name, fs, ts);
        });

        // Also persist theme name to session.toml whenever it changes.
        create_effect(move |_| {
            let theme_name = theme_signal.get().variant.name().to_string();
            let current = session_load();
            session_save_full(
                current.open_file.as_ref(),
                current.left_panel_width,
                current.show_bottom_panel,
                current.vim_mode,
                &theme_name,
            );
        });

        // AI provider / model signals — initialized from current settings file.
        let ai_provider_sig = create_rw_signal(
            settings.llm.provider.to_provider_id().name().to_string()
        );
        let ai_model_sig = create_rw_signal(settings.llm.model.clone());

        // Persist provider + model changes to settings.toml whenever they change.
        create_effect(move |_| {
            let provider_name = ai_provider_sig.get();
            let model = ai_model_sig.get();
            let mut s = Settings::load();
            s.llm.provider = provider_name_to_llm_provider(&provider_name);
            s.llm.model = model;
            let _ = s.save();
        });

        Self {
            theme: theme_signal,
            left_panel_tab: create_rw_signal(Tab::Explorer),
            bottom_panel_tab: create_rw_signal(Tab::Terminal),
            show_left_panel: create_rw_signal(true),
            show_right_panel: create_rw_signal(true),
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
            output_log: create_rw_signal(vec![
                "[PhazeAI] Output panel ready.".to_string(),
            ]),
            references,
            references_visible: create_rw_signal(false),
            code_actions,
            code_actions_open: create_rw_signal(false),
            rename_open:   create_rw_signal(false),
            rename_query:  create_rw_signal(String::new()),
            rename_target: create_rw_signal(String::new()),
            sig_help,
            doc_symbols,
            status_toast: create_rw_signal(None),
            zen_mode: create_rw_signal(false),
            line_ending: line_ending_sig,
            ws_syms_open: create_rw_signal(false),
            ws_syms_query: create_rw_signal(String::new()),
            workspace_symbols,
            branch_picker_open: create_rw_signal(false),
            branch_list: create_rw_signal(Vec::new()),
            auto_save: create_rw_signal(false),
            word_wrap: create_rw_signal(false),
            ctrl_d_nonce: create_rw_signal(0u64),
            fold_nonce: create_rw_signal(0u64),
            unfold_nonce: create_rw_signal(0u64),
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
            action: |s| { s.show_bottom_panel.update(|v| *v = !*v); },
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
            action: |s| { s.show_right_panel.update(|v| *v = !*v); },
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
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::MidnightBlue)); },
        },
        PaletteCommand {
            label: "Theme: Cyberpunk 2077",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Cyberpunk)); },
        },
        PaletteCommand {
            label: "Theme: Synthwave '84",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Synthwave84)); },
        },
        PaletteCommand {
            label: "Theme: Andromeda",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Andromeda)); },
        },
        PaletteCommand {
            label: "Theme: Dark",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dark)); },
        },
        PaletteCommand {
            label: "Theme: Dracula",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dracula)); },
        },
        PaletteCommand {
            label: "Theme: Tokyo Night",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::TokyoNight)); },
        },
        PaletteCommand {
            label: "Theme: Monokai",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Monokai)); },
        },
        PaletteCommand {
            label: "Theme: Nord Dark",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::NordDark)); },
        },
        PaletteCommand {
            label: "Theme: Matrix Green",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::MatrixGreen)); },
        },
        PaletteCommand {
            label: "Theme: Root Shell",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::RootShell)); },
        },
        PaletteCommand {
            label: "Theme: Light",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Light)); },
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
    create_effect(move |_| {
        if !state.file_picker_open.get() { return; }
        let root = state.workspace_root.get();
        if last_root.get().as_ref() == Some(&root) { return; }
        last_root.set(Some(root.clone()));
        let scope = Scope::new();
        let send = create_ext_action(scope, move |files: Vec<std::path::PathBuf>| {
            all_files.set(files);
        });
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
            send(files);
        });
    });

    let filtered = move || -> Vec<(usize, std::path::PathBuf)> {
        let q = query.get().to_lowercase();
        all_files
            .get()
            .into_iter()
            .filter(|p| {
                if q.is_empty() { return true; }
                let name = p.file_name()
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
        dyn_stack(
            filtered,
            |(idx, _)| *idx,
            {
                let state = state.clone();
                move |(idx, path)| {
                    let path_clone = path.clone();
                    let root = state.workspace_root.get();
                    let display = path.strip_prefix(&root)
                        .ok()
                        .map(|r| r.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());
                    let display2 = display.clone();
                    let hov = hovered;
                    let state = state.clone();
                    container(
                        stack((
                            label(move || {
                                path_clone.file_name()
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
                            label(move || format!("  {}", display2))
                                .style({
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
                             .background(if hov.get() == Some(idx) { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
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
                    .on_event_stop(EventListener::PointerEnter, move |_| { hov.set(Some(idx)); })
                    .on_event_stop(EventListener::PointerLeave, move |_| { hov.set(None); })
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(360.0));

    let empty_hint = container(
        label(|| "Searching workspace files…")
            .style(move |s| {
                s.font_size(12.0).color(state.theme.get().palette.text_muted)
            }),
    )
    .style(move |s| {
        let empty = all_files.get().is_empty() && state.file_picker_open.get();
        s.width_full().padding_vert(12.0).items_center().justify_center()
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
                 .z_index(200)
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
        dyn_stack(
            commands_list,
            |(idx, lbl, _action)| (*idx, *lbl),
            {
                let state = state.clone();
                move |(idx, cmd_label, cmd_action)| {
                    let hovered = row_hovered;
                    let state = state.clone();
                    container(
                        label(move || cmd_label)
                            .style({
                                let state = state.clone();
                                move |s| {
                                    s.font_size(13.0)
                                     .color(state.theme.get().palette.text_primary)
                                }
                            })
                    )
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
                             .background(if is_hov { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
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
                    .on_event_stop(EventListener::PointerEnter, move |_| { hovered.set(Some(idx)); })
                    .on_event_stop(EventListener::PointerLeave, move |_| { hovered.set(None); })
                }
            }
        )
        .style(|s| s.flex_col().width_full())
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
                 .z_index(100)
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

/// Cosmic nebula canvas — absolute-positioned behind all UI panels.
/// Uses blurred fills to paint soft gaseous clouds in electric purple/indigo.
fn cosmic_bg_canvas(theme: RwSignal<PhazeTheme>) -> impl IntoView {
    canvas(move |cx, size| {
        let t = theme.get();
        let p = &t.palette;
        let w = size.width;
        let h = size.height;

        // 1. Deep-space base fill — matches MidnightBlue #050310
        cx.fill(
            &floem::kurbo::Rect::ZERO.with_size(size),
            floem::peniko::Color::from_rgb8(5, 3, 16),
            0.0,
        );

        if !t.is_cosmic() {
            return;
        }

        // 2. Subtle hex grid — very faint accent dots for an "engineered" feel
        let hex_size = 40.0;
        let grid_color = p.accent.with_alpha(0.06);
        let horiz_dist = hex_size * 3.0f64.sqrt();
        let vert_dist = hex_size * 1.5;

        for row in 0..((h / vert_dist) as i32 + 2) {
            for col in 0..((w / horiz_dist) as i32 + 2) {
                let x_offset = if row % 2 == 1 { horiz_dist / 2.0 } else { 0.0 };
                let x = col as f64 * horiz_dist + x_offset;
                let y = row as f64 * vert_dist;
                cx.fill(
                    &floem::kurbo::Circle::new(floem::kurbo::Point::new(x, y), 0.8),
                    grid_color,
                    0.0,
                );
            }
        }

        // 3. Nebula glow clouds — violet / indigo palette matching target screenshot
        // Top-Left: Deep violet cloud
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.1, h * 0.12), 520.0),
            floem::peniko::Color::from_rgba8(80, 30, 180, 88),
            150.0,
        );

        // Right-Center: Indigo cloud (replaces old cyan — stays in violet family)
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.92, h * 0.38), 440.0),
            floem::peniko::Color::from_rgba8(40, 20, 180, 78),
            130.0,
        );

        // Bottom-Right: Deep blue-violet
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.8, h * 0.82), 480.0),
            floem::peniko::Color::from_rgba8(60, 10, 200, 72),
            140.0,
        );

        // Center-Bottom: Violet cloud — grounds the composition
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.45, h * 0.88), 400.0),
            floem::peniko::Color::from_rgba8(100, 40, 220, 62),
            125.0,
        );

        // Subtle vignette — darkens edges to draw focus inward
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.5, h * 0.5), (w.max(h)) * 0.68),
            floem::peniko::Color::from_rgba8(2, 1, 10, 35),
            90.0,
        );
    })
    // Absolute-positioned so it doesn't participate in flex layout but
    // covers the full parent container — rendered below all siblings.
    .style(|s| s.absolute().inset(0))
}

fn activity_bar_btn(
    icon_svg: &'static str,
    tab: Tab,
    state: IdeState,
) -> impl IntoView {
    let is_hovered = create_rw_signal(false);
    let active = move || state.left_panel_tab.get() == tab && state.show_left_panel.get();
    
    let icon_color = move |p: &crate::theme::PhazePalette| {
        if active() { p.accent } else { p.text_secondary }
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
         .transition(floem::style::Background, floem::style::Transition::linear(Duration::from_millis(150)))
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

fn placeholder_tab(tab_name: &'static str, state: IdeState) -> impl IntoView {
    container(
        label(move || tab_name)
            .style(move |s| {
                s.color(state.theme.get().palette.text_muted)
                 .font_size(12.0)
                 .padding(16.0)
            }),
    )
    .style(move |s| {
        let t = state.theme.get();
        s.width_full().height_full().background(t.palette.glass_bg)
    })
}

fn left_panel(state: IdeState) -> impl IntoView {
    let explorer = explorer_panel(
        state.workspace_root,
        state.open_file,
        state.theme,
    );

    let explorer_wrap = container(explorer).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Explorer, |s| s.display(floem::style::Display::None))
        }
    });

    let search_wrap = container(search::search_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Search, |s| s.display(floem::style::Display::None))
        }
    });

    let git_wrap = container(git_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Git, |s| s.display(floem::style::Display::None))
        }
    });

    let debug_wrap = container(placeholder_tab("Run and Debug", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Debug, |s| s.display(floem::style::Display::None))
        }
    });

    let extensions_wrap = container(placeholder_tab("Extensions", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Extensions, |s| s.display(floem::style::Display::None))
        }
    });

    let remote_wrap = container(placeholder_tab("Remote Explorer", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Remote, |s| s.display(floem::style::Display::None))
        }
    });

    let container_wrap = container(placeholder_tab("Containers", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Containers, |s| s.display(floem::style::Display::None))
        }
    });

    let makefile_wrap = container(placeholder_tab("Makefile", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Makefile, |s| s.display(floem::style::Display::None))
        }
    });

    let github_wrap = container(placeholder_tab("GitHub Actions", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::GitHub, |s| s.display(floem::style::Display::None))
        }
    });

    let symbols_wrap = container(symbol_outline_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Symbols, |s| s.display(floem::style::Display::None))
        }
    });

    let ai_wrap = container(ai_panel(state.theme)).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::AI, |s| s.display(floem::style::Display::None))
        }
    });

    let settings_wrap = container(settings_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Settings, |s| s.display(floem::style::Display::None))
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
            settings_wrap,
        ))
        .style(|s| s.width_full().height_full()),
    )
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        let show = state.show_left_panel.get();
        let width = if show { state.left_panel_width.get() } else { 0.0 };
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

fn bottom_panel_tab(
    label_str: &'static str,
    tab: Tab,
    state: IdeState,
) -> impl IntoView {
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
             .background(if active { p.bg_surface } else if hovered { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
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
    let cloud_btn = container(label(|| "☁ Sign in"))
        .style(move |s| {
            let p = state.theme.get().palette;
            s.font_size(10.0)
             .padding_horiz(8.0)
             .padding_vert(2.0)
             .margin_right(8.0)
             .border_radius(3.0)
             .cursor(floem::style::CursorStyle::Pointer)
             .color(p.accent)
             .background(p.accent_dim)
        })
        .on_click_stop(|_| {
            // Open PhazeAI cloud sign-in in the system browser.
            let url = "https://phazeai.dev/signin";
            let opener = if cfg!(target_os = "macos") { "open" }
                         else if cfg!(target_os = "windows") { "cmd" }
                         else { "xdg-open" };
            let mut cmd = std::process::Command::new(opener);
            if cfg!(target_os = "windows") {
                cmd.args(["/C", "start", "", url]);
            } else {
                cmd.arg(url);
            }
            let _ = cmd.spawn();
        });

    // Branch clickable button — click to open branch picker overlay
    let branch_btn = {
        let s = state.clone();
        let s2 = state.clone();
        let is_hov = create_rw_signal(false);
        container(stack((
            phaze_icon(icons::BRANCH, 12.0, move |p| p.accent, state.theme),
            label(move || format!(" {} ", s.git_branch.get()))
                .style(move |s2| s2.color(state.theme.get().palette.text_secondary).font_size(11.0)),
        )).style(|s| s.items_center()))
        .style(move |s| {
            let p = s2.theme.get().palette;
            s.padding_horiz(6.0).padding_vert(2.0).border_radius(4.0)
             .cursor(floem::style::CursorStyle::Pointer)
             .background(if is_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
        })
        .on_click_stop({
            let s3 = state.clone();
            move |_| {
                let root = s3.workspace_root.get();
                let picker_open = s3.branch_picker_open;
                let branch_list = s3.branch_list;
                let scope = Scope::new();
                let send = create_ext_action(scope, move |branches: Vec<String>| {
                    branch_list.set(branches);
                    picker_open.set(true);
                });
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
                    send(branches);
                });
            }
        })
        .on_event_stop(EventListener::PointerEnter, move |_| is_hov.set(true))
        .on_event_stop(EventListener::PointerLeave, move |_| is_hov.set(false))
    };

    let left = stack((
        cloud_btn,
        branch_btn,
        label(|| "   ")
            .style(|s| s.font_size(11.0)),
        phaze_icon(icons::BRANCH, 12.0, move |p| p.accent, state.theme),
        label(move || {
            let s = Settings::load();
            format!(" {}", s.llm.model)
        })
        .style(move |s| s.color(state.theme.get().palette.text_secondary).font_size(11.0)),
    ))
    .style(|s| s.items_center().padding_horiz(8.0));

    // VIM mode toggle button — shows INSERT/NORMAL when active
    let vim_btn = {
        let s = state.clone();
        let s_label = state.clone();
        container(label(move || {
            if !s_label.vim_mode.get() { return "NORMAL".to_string(); }
            if s_label.vim_normal_mode.get() { "-- NORMAL --".to_string() }
            else { "-- INSERT --".to_string() }
        }))
            .style(move |s2| {
                let p = state.theme.get().palette;
                let vim = state.vim_mode.get();
                let normal = state.vim_normal_mode.get();
                s2.font_size(10.0)
                  .padding_horiz(6.0).padding_vert(2.0)
                  .margin_right(6.0)
                  .border_radius(3.0)
                  .cursor(floem::style::CursorStyle::Pointer)
                  .color(if vim { p.bg_base } else { p.text_muted })
                  .background(if vim && normal { p.warning }
                               else if vim { p.accent }
                               else { p.bg_elevated })
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
                session_save_full(
                    s2.open_file.get().as_ref(),
                    s2.left_panel_width.get(),
                    s2.show_bottom_panel.get(),
                    s2.vim_mode.get(),
                    s2.theme.get().variant.name(),
                );
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
        .style(move |s| s.color(state.theme.get().palette.text_secondary).font_size(11.0)),
        // LSP diagnostic counts — live from the reactive diagnostics signal.
        label(move || {
            let diags = state.diagnostics.get();
            let errs = diags.iter().filter(|d| d.severity == DiagSeverity::Error).count();
            let warns = diags.iter().filter(|d| d.severity == DiagSeverity::Warning).count();
            if errs == 0 && warns == 0 {
                String::new()
            } else {
                format!("⊗ {errs}  ⚠ {warns}  ")
            }
        })
        .style(move |s| {
            let p = state.theme.get().palette;
            let has_errs = state.diagnostics.get().iter().any(|d| d.severity == DiagSeverity::Error);
            s.font_size(11.0).color(if has_errs { p.error } else { p.warning })
        }),
        vim_btn,
        // Diagnostic message for current cursor line (from LSP).
        label(move || {
            if let Some((ref path, line, _col)) = state.active_cursor.get() {
                let diags = state.diagnostics.get();
                // Find first diagnostic on current line (1-based line = line+1).
                let cur_line_1 = line + 1;
                if let Some(d) = diags.iter().find(|d| d.path == *path && d.line == cur_line_1) {
                    let prefix = match d.severity {
                        DiagSeverity::Error   => "⊗ ",
                        DiagSeverity::Warning => "⚠ ",
                        DiagSeverity::Info    => "ℹ ",
                        DiagSeverity::Hint    => "💡 ",
                    };
                    let msg = if d.message.len() > 60 {
                        format!("{}{}…  ", prefix, &d.message[..60])
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
            let has_err = state.active_cursor.get().map(|(ref path, line, _)| {
                state.diagnostics.get().iter().any(|d| {
                    d.path == *path && d.line == line + 1 && d.severity == DiagSeverity::Error
                })
            }).unwrap_or(false);
            s.font_size(10.0).color(if has_err { p.error } else { p.warning })
        }),
        label(|| "AI Ready  ")
            .style(move |s| s.color(state.theme.get().palette.success).font_size(11.0)),
        // Dynamic encoding + line ending indicator (e.g. "UTF-8 CRLF  ")
        label(move || format!("UTF-8 {}  ", state.line_ending.get()))
            .style(move |s| s.color(state.theme.get().palette.text_muted).font_size(11.0)),
        label(move || {
            state.open_file.get()
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
        .style(move |s| s.color(state.theme.get().palette.text_muted).font_size(11.0)),
    ))
    .style(|s| s.items_center().padding_horiz(8.0));

    stack((left, right))
        .style(move |s| {
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
    let diags     = state.diagnostics;
    let theme     = state.theme;
    let open_file = state.open_file;
    let goto_line = state.goto_line;

    // Filter toggles
    let show_errors   = crws(true);
    let show_warnings = crws(true);

    let err_btn = container(label(move || {
        let n = diags.get().iter().filter(|d| d.severity == DiagSeverity::Error).count();
        format!("⊗ Errors ({n})")
    }))
    .style(move |s| {
        let p = theme.get().palette;
        let on = show_errors.get();
        s.font_size(11.0).padding_horiz(8.0).padding_vert(3.0).border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .color(if on { p.bg_base } else { p.error })
         .background(if on { p.error } else { p.bg_elevated })
    })
    .on_click_stop(move |_| { show_errors.update(|v| *v = !*v); });

    let warn_btn = container(label(move || {
        let n = diags.get().iter().filter(|d| d.severity == DiagSeverity::Warning).count();
        format!("⚠ Warnings ({n})")
    }))
    .style(move |s| {
        let p = theme.get().palette;
        let on = show_warnings.get();
        s.font_size(11.0).padding_horiz(8.0).padding_vert(3.0).border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .color(if on { p.bg_base } else { p.warning })
         .background(if on { p.warning } else { p.bg_elevated })
    })
    .on_click_stop(move |_| { show_warnings.update(|v| *v = !*v); });

    let filter_bar = stack((err_btn, warn_btn))
        .style(move |s| {
            let p = theme.get().palette;
            s.flex_row().gap(6.0).padding_horiz(12.0).padding_vert(6.0)
             .border_bottom(1.0).border_color(p.border).width_full().items_center()
        });

    let empty_msg = container(
        label(move || {
            if diags.get().is_empty() { "No problems detected ✓".to_string() }
            else { String::new() }
        })
        .style(move |s| s.font_size(12.0).color(theme.get().palette.success)),
    )
    .style(move |s| {
        s.width_full().padding(16.0)
         .apply_if(!diags.get().is_empty(), |s| s.display(floem::style::Display::None))
    });

    let list = scroll(
        dyn_stack(
            move || {
                diags.get()
                    .into_iter()
                    .filter(|d| match d.severity {
                        DiagSeverity::Error   => show_errors.get(),
                        DiagSeverity::Warning => show_warnings.get(),
                        _                     => true,
                    })
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(idx, _)| *idx,
            {
                let theme = state.theme;
                move |(_, entry): (usize, DiagEntry)| {
                    let sev      = entry.severity;
                    let icon     = match sev { DiagSeverity::Error => "⊗", DiagSeverity::Warning => "⚠", DiagSeverity::Info => "ℹ", DiagSeverity::Hint => "○" };
                    let filename = entry.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "?".to_string());
                    let loc      = format!("{}:{}", entry.line, entry.col);
                    let msg      = entry.message.clone();
                    let path     = entry.path.clone();
                    let line_no  = entry.line;
                    let hovered  = crws(false);

                    container(stack((
                        label(move || icon.to_string())
                            .style(move |s| {
                                let p = theme.get().palette;
                                let c = match sev { DiagSeverity::Error => p.error, DiagSeverity::Warning => p.warning, DiagSeverity::Info => p.accent, _ => p.text_muted };
                                s.font_size(13.0).color(c).margin_right(8.0)
                            }),
                        label(move || msg.clone())
                            .style(move |s| s.font_size(12.0).color(theme.get().palette.text_primary).flex_grow(1.0)),
                        label(move || filename.clone())
                            .style(move |s| s.font_size(11.0).color(theme.get().palette.accent).margin_left(8.0)),
                        label(move || loc.clone())
                            .style(move |s| s.font_size(10.0).color(theme.get().palette.text_muted).margin_left(6.0)),
                    ))
                    .style(|s| s.flex_row().items_center().width_full()))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.width_full().padding_horiz(12.0).padding_vert(5.0)
                         .cursor(floem::style::CursorStyle::Pointer)
                         .background(if hovered.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                    })
                    .on_click_stop(move |_| {
                        open_file.set(Some(path.clone()));
                        goto_line.set(line_no);
                    })
                    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { hovered.set(true); })
                    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { hovered.set(false); })
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        s.width_full().flex_grow(1.0)
         .apply_if(diags.get().is_empty(), |s| s.display(floem::style::Display::None))
    });

    stack((filter_bar, empty_msg, list))
        .style(|s| s.flex_col().width_full().height_full())
}

fn references_view(state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal as crws;
    let refs      = state.references;
    let theme     = state.theme;
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
        s.width_full().padding(16.0)
         .apply_if(!refs.get().is_empty(), |s| s.display(floem::style::Display::None))
    });

    let count_label = label(move || {
        let n = refs.get().len();
        if n == 0 { String::new() } else { format!("{n} reference{}", if n == 1 { "" } else { "s" }) }
    })
    .style(move |s| {
        s.font_size(11.0).color(theme.get().palette.text_muted)
         .padding_horiz(12.0).padding_vert(4.0).width_full()
         .apply_if(refs.get().is_empty(), |s| s.display(floem::style::Display::None))
    });

    let list = scroll(
        dyn_stack(
            move || refs.get().into_iter().enumerate().collect::<Vec<_>>(),
            |(idx, _)| *idx,
            {
                let theme = state.theme;
                move |(_, entry): (usize, ReferenceEntry)| {
                    let filename = entry.path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "?".to_string());
                    let loc     = format!(":{}", entry.line);
                    let path    = entry.path.clone();
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

                    container(stack((
                        label(move || filename.clone())
                            .style(move |s| {
                                s.font_size(12.0).color(theme.get().palette.accent)
                                 .font_weight(floem::text::Weight::SEMIBOLD)
                            }),
                        label(move || loc.clone())
                            .style(move |s| {
                                s.font_size(11.0).color(theme.get().palette.text_muted).margin_right(8.0)
                            }),
                        label(move || snippet.clone())
                            .style(move |s| {
                                s.font_size(11.5).color(theme.get().palette.text_secondary)
                                 .flex_grow(1.0)
                                 .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                            }),
                    ))
                    .style(|s| s.flex_row().items_center().width_full()))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.width_full().padding_horiz(12.0).padding_vert(5.0)
                         .cursor(floem::style::CursorStyle::Pointer)
                         .background(if hovered.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                    })
                    .on_click_stop(move |_| {
                        open_file.set(Some(path.clone()));
                        goto_line.set(line_no);
                    })
                    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { hovered.set(true); })
                    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { hovered.set(false); })
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        s.width_full().flex_grow(1.0)
         .apply_if(refs.get().is_empty(), |s| s.display(floem::style::Display::None))
    });

    stack((count_label, empty_msg, list))
        .style(|s| s.flex_col().width_full().height_full())
}

fn output_view(state: IdeState) -> impl IntoView {
    let log  = state.output_log;
    let theme = state.theme;
    scroll(
        dyn_stack(
            move || log.get().into_iter().enumerate().collect::<Vec<_>>(),
            |(idx, _)| *idx,
            move |(_, line): (usize, String)| {
                let line2 = line.clone();
                label(move || line.clone())
                    .style(move |s| {
                        let p = theme.get().palette;
                        let color = if line2.starts_with("[error]") || line2.contains("error") && !line2.contains("0 errors") {
                            p.error
                        } else if line2.starts_with("[warn]") || line2.contains("warning") {
                            p.warning
                        } else {
                            p.text_secondary
                        };
                        s.font_size(11.5).color(color)
                         .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                         .padding_horiz(12.0).padding_vert(1.0).width_full()
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
            label(|| "▷  No active debug session")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(13.0).color(p.text_muted)
                }),
            label(|| "Run a debug configuration to start a session.")
                .style(move |s| {
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
            label(|| "No forwarded ports")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(13.0).color(p.text_muted)
                }),
            label(|| "Ports forwarded by running processes will appear here.")
                .style(move |s| {
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
    let symbols   = state.doc_symbols;
    let theme     = state.theme;
    let open_file = state.open_file;
    let goto_line = state.goto_line;
    let lsp_cmd   = state.lsp_cmd.clone();

    // Refresh button
    let refresh_btn = container(
        label(|| " ↺ ".to_string())
            .style(move |s| s.font_size(13.0).color(theme.get().palette.text_muted)
                .cursor(floem::style::CursorStyle::Pointer))
    )
    .on_click_stop(move |_| {
        if let Some(path) = open_file.get_untracked() {
            let _ = lsp_cmd.send(LspCommand::RequestDocumentSymbols { path });
        }
    });

    let header = stack((
        label(|| "OUTLINE".to_string())
            .style(move |s| {
                s.font_size(11.0).font_weight(floem::text::Weight::BOLD)
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
        .style(move |s| s.font_size(12.0).color(theme.get().palette.text_muted).padding(12.0))
    )
    .style(move |s| s.apply_if(!symbols.get().is_empty(), |s| s.display(floem::style::Display::None)));

    let list = scroll(
        dyn_stack(
            move || symbols.get().into_iter().enumerate().collect::<Vec<_>>(),
            |(i, _)| *i,
            {
                let theme = state.theme;
                move |(_, sym): (usize, SymbolEntry)| {
                    let hovered = crws(false);
                    let name    = sym.name.clone();
                    let kind    = sym.kind.clone();
                    let line_no = sym.line;
                    let indent  = sym.depth * 12;

                    let kind_color = match kind.as_str() {
                        "fn"     => floem::peniko::Color::from_rgb8(86, 156, 214),
                        "struct" => floem::peniko::Color::from_rgb8(78, 201, 176),
                        "enum"   => floem::peniko::Color::from_rgb8(197, 134, 192),
                        "trait"  => floem::peniko::Color::from_rgb8(220, 220, 170),
                        "impl"   => floem::peniko::Color::from_rgb8(150, 200, 150),
                        "mod"    => floem::peniko::Color::from_rgb8(200, 200, 100),
                        _        => floem::peniko::Color::from_rgb8(180, 180, 180),
                    };

                    container(stack((
                        label(move || format!("{kind} "))
                            .style(move |s| s.font_size(11.0).color(kind_color)
                                .font_family("JetBrains Mono, monospace".to_string())),
                        label(move || name.clone())
                            .style(move |s| s.font_size(12.0).color(theme.get().palette.text_primary)
                                .font_family("JetBrains Mono, monospace".to_string())),
                    ))
                    .style(move |s| s.flex_row().items_center().padding_left(indent as f64)))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.width_full().padding_horiz(8.0).padding_vert(3.0)
                         .cursor(floem::style::CursorStyle::Pointer)
                         .background(if hovered.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                    })
                    .on_click_stop(move |_| { goto_line.set(line_no); })
                    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { hovered.set(true); })
                    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { hovered.set(false); })
                }
            }
        )
        .style(|s| s.flex_col().width_full())
    )
    .style(move |s| s.flex_grow(1.0).width_full());

    stack((header, empty_msg, list))
        .style(|s| s.flex_col().width_full().height_full())
}

/// Git diff viewer — shown in the bottom panel "GIT DIFF" tab.
fn git_diff_view(state: IdeState) -> impl IntoView {
    use floem::ext_event::create_ext_action;
    use floem::reactive::Scope;

    let theme     = state.theme;
    let open_file = state.open_file;

    // Reactive signal holding the parsed diff lines (text + color-kind).
    // 0=context, 1=added (+), 2=removed (-), 3=header (@@/---/+++)
    let diff_lines: floem::reactive::RwSignal<Vec<(String, u8)>> =
        floem::reactive::create_rw_signal(vec![]);

    // Whenever the open file changes, run git diff in background.
    {
        let diff_sig = diff_lines;
        floem::reactive::create_effect(move |_| {
            if let Some(path) = open_file.get() {
                let scope = Scope::new();
                let send  = create_ext_action(scope, move |lines| { diff_sig.set(lines); });
                std::thread::spawn(move || {
                    let lines = run_git_diff(&path);
                    send(lines);
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
        .style(move |s| s.font_size(12.0).color(theme.get().palette.text_muted).padding(16.0))
    )
    .style(move |s| s.apply_if(!diff_lines.get().is_empty(), |s| s.display(floem::style::Display::None)));

    let diff_scroll = scroll(
        dyn_stack(
            move || diff_lines.get().into_iter().enumerate().collect::<Vec<_>>(),
            |(i, _)| *i,
            move |(_, (text, kind)): (usize, (String, u8))| {
                let color = match kind {
                    1 => floem::peniko::Color::from_rgba8(80,  200,  80, 255),  // added
                    2 => floem::peniko::Color::from_rgba8(220,  60,  60, 255),  // removed
                    3 => floem::peniko::Color::from_rgba8(100, 160, 255, 255),  // header
                    _ => theme.get().palette.text_secondary,                     // context
                };
                let bg = match kind {
                    1 => floem::peniko::Color::from_rgba8( 40,  80,  40, 120),
                    2 => floem::peniko::Color::from_rgba8( 80,  20,  20, 120),
                    3 => floem::peniko::Color::from_rgba8( 20,  40,  80, 100),
                    _ => floem::peniko::Color::TRANSPARENT,
                };
                container(
                    label(move || text.clone())
                        .style(move |s| s.font_size(12.0).color(color)
                            .font_family("JetBrains Mono, Fira Code, monospace".to_string()))
                )
                .style(move |s| s.width_full().padding_horiz(8.0).padding_vert(1.0).background(bg))
            }
        )
        .style(|s| s.flex_col().width_full())
    )
    .style(move |s| {
        s.flex_grow(1.0).width_full()
         .apply_if(diff_lines.get().is_empty(), |s| s.display(floem::style::Display::None))
    });

    stack((empty_msg, diff_scroll))
        .style(|s| s.flex_col().width_full().height_full())
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
        if text2.trim().is_empty() { return vec![]; }
        parse_diff_output(&text2)
    } else {
        parse_diff_output(&text)
    }
}

fn parse_diff_output(text: &str) -> Vec<(String, u8)> {
    text.lines().map(|line| {
        let kind = if line.starts_with('+') && !line.starts_with("+++") { 1 }
                   else if line.starts_with('-') && !line.starts_with("---") { 2 }
                   else if line.starts_with("@@") || line.starts_with("---") || line.starts_with("+++") { 3 }
                   else { 0u8 };
        (line.to_string(), kind)
    }).collect()
}

fn bottom_panel(state: IdeState) -> impl IntoView {
    let current_tab = state.bottom_panel_tab;

    container(
        stack((
            // Tab bar
            stack((
                bottom_panel_tab("TERMINAL", Tab::Terminal, state.clone()),
                bottom_panel_tab("PROBLEMS", Tab::Problems, state.clone()),
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
                    })
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
            }),
            
            // Content
            stack((
                container(terminal_panel(state.theme))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::Terminal, |s| s.display(floem::style::Display::None))
                    }),
                container(problems_view(state.clone()))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::Problems, |s| s.display(floem::style::Display::None))
                    }),
                container(references_view(state.clone()))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::References, |s| s.display(floem::style::Display::None))
                    }),
                container(git_diff_view(state.clone()))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::GitDiff, |s| s.display(floem::style::Display::None))
                    }),
                container(output_view(state.clone()))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::Output, |s| s.display(floem::style::Display::None))
                    }),
                container(debug_console_view(state.clone()))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::DebugConsole, |s| s.display(floem::style::Display::None))
                    }),
                container(ports_view(state.clone()))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::Ports, |s| s.display(floem::style::Display::None))
                    }),
            ))
            .style(|s| s.flex_grow(1.0).width_full())
        ))
        .style(|s| s.flex_col().width_full().height_full())
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
         .apply_if(!state.show_bottom_panel.get(), |s| s.display(floem::style::Display::None))
    })
}

fn completion_popup(state: IdeState) -> impl IntoView {
    let items    = state.completions;
    let selected = state.completion_selected;
    let filter   = state.completion_filter_text;

    // Filtered + enumerated list — index preserves original position so
    // Enter/Tab handler can look up the right entry by `selected`.
    let filtered_items = move || -> Vec<(usize, CompletionEntry)> {
        let f = filter.get().to_lowercase();
        items.get().into_iter().enumerate().filter(|(_, e)| {
            f.is_empty() || e.label.to_lowercase().starts_with(&f)
        }).collect()
    };

    let list = scroll(
        dyn_stack(
            filtered_items,
            |(idx, _)| *idx,
            {
                let state = state.clone();
                move |(idx, entry): (usize, CompletionEntry)| {
                    let is_sel = move || selected.get() == idx;
                    let item_detail = entry.detail.clone().unwrap_or_default();
                    let item_label  = entry.label.clone();
                    stack((
                        label(move || item_label.clone())
                            .style(move |s: floem::style::Style| {
                                let p = state.theme.get().palette;
                                s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
                            }),
                        label(move || item_detail.clone())
                            .style(move |s: floem::style::Style| {
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
                         .background(if is_sel() { p.accent_dim } else { floem::peniko::Color::TRANSPARENT })
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
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(280.0));

    let header = stack((
        label(|| "Completions")
            .style(move |s| {
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
        let count = items.get().into_iter().filter(|e| f.is_empty() || e.label.to_lowercase().starts_with(&f)).count();
        if count == 0 { format!("No completions{}", if f.is_empty() { String::new() } else { format!(" for \"{f}\"") }) }
        else { String::new() }
    })
    .style(move |s| {
        let f = filter.get().to_lowercase();
        let count = items.get().into_iter().filter(|e| f.is_empty() || e.label.to_lowercase().starts_with(&f)).count();
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
             .width(420.0)
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
                        let max = items.get().into_iter().filter(|e| f.is_empty() || e.label.to_lowercase().starts_with(&f)).count().saturating_sub(1);
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
             .z_index(300)
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
    let open  = state.inline_edit_open;
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

    let hint = label(|| "Describe the change (Enter to apply, Esc to cancel)")
        .style(move |s| {
            s.font_size(11.0).color(state.theme.get().palette.text_muted).margin_bottom(6.0)
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
                        let file_ctx = if file_ctx.len() > 4096 { &file_ctx[..4096] } else { &file_ctx };
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

    let badge = label(|| "✦ AI Edit")
        .style(move |s| {
            let p = state.theme.get().palette;
            s.font_size(11.0).color(p.accent).font_weight(floem::text::Weight::BOLD)
             .margin_bottom(8.0)
        });

    let box_view = stack((badge, hint, input))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            s.flex_col().padding(20.0).width(520.0)
             .background(p.bg_panel)
             .border(1.0).border_color(p.glass_border)
             .border_radius(10.0)
             .box_shadow_h_offset(0.0).box_shadow_v_offset(6.0)
             .box_shadow_blur(40.0).box_shadow_color(p.glow)
             .box_shadow_spread(0.0)
        });

    container(box_view)
        .style(move |s| {
            let shown = open.get();
            s.absolute().inset(0).items_start().justify_center()
             .padding_top(200.0)
             .z_index(400)
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
    let theme      = state.theme;

    // Wrap the text in a styled container that looks like a floating doc box.
    let tooltip_box = container(
        label(move || hover_text.get().unwrap_or_default())
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(12.0)
                 .color(p.text_primary)
                 .max_width(480.0)
            })
    )
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
    container(tooltip_box)
        .style(move |s| {
            let shown = hover_text.get().is_some();
            s.absolute()
             .inset_bottom(60.0)
             .inset_left(320.0)
             .z_index(250)
             .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
}

// ── Code Actions dropdown overlay (Ctrl+.) ───────────────────────────────────

fn code_actions_overlay(state: IdeState) -> impl IntoView {
    let open    = state.code_actions_open;
    let actions = state.code_actions;
    let theme   = state.theme;
    let hovered: RwSignal<Option<usize>> = create_rw_signal(None);

    let header = stack((
        label(|| "Quick Fix")
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0)
                 .color(p.accent)
                 .font_weight(floem::text::Weight::SEMIBOLD)
                 .flex_grow(1.0)
            }),
        container(label(|| "Esc"))
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(10.0).color(p.text_muted)
                 .background(p.bg_elevated)
                 .padding_horiz(5.0).padding_vert(2.0)
                 .border_radius(3.0)
                 .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| open.set(false)),
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.items_center().width_full()
         .padding_horiz(12.0).padding_vert(8.0)
         .border_bottom(1.0).border_color(p.border)
         .margin_bottom(4.0)
    });

    let empty_hint = label(move || {
        if actions.get().is_empty() { "No actions available at this position.".to_string() }
        else { String::new() }
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(12.0).color(p.text_muted).padding(12.0)
         .apply_if(!actions.get().is_empty(), |s| s.display(floem::style::Display::None))
    });

    let list = scroll(
        dyn_stack(
            move || actions.get().into_iter().enumerate().collect::<Vec<_>>(),
            |(idx, _)| *idx,
            {
                let state2 = state.clone();
                move |(idx, action): (usize, CodeAction)| {
                    let title   = action.title.clone();
                    let kind    = action.kind.clone();
                    let edits   = action.edit.clone();
                    let hov     = hovered;
                    let state3  = state2.clone();

                    container(
                        stack((
                            label(|| "▶ ")
                                .style(move |s| {
                                    let p = state3.theme.get().palette;
                                    s.font_size(10.0).color(p.accent).margin_right(4.0)
                                }),
                            label(move || title.clone())
                                .style(move |s: floem::style::Style| {
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
                            s.width_full().padding_horiz(12.0).padding_vert(8.0)
                             .border_radius(4.0)
                             .cursor(floem::style::CursorStyle::Pointer)
                             .background(if hov.get() == Some(idx) { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                        }
                    })
                    .on_click_stop({
                        let state5 = state2.clone();
                        let kind2  = kind.clone();
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
                    .on_event_stop(EventListener::PointerEnter, move |_| { hov.set(Some(idx)); })
                    .on_event_stop(EventListener::PointerLeave, move |_| { hov.set(None); })
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
             .border(1.0).border_color(p.glass_border)
             .border_radius(8.0)
             .box_shadow_h_offset(0.0).box_shadow_v_offset(4.0)
             .box_shadow_blur(32.0).box_shadow_color(p.glow)
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
             .z_index(350)
             .background(floem::peniko::Color::from_rgba8(0, 0, 0, 100))
             .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| state.code_actions_open.set(false))
}

/// Rename-symbol overlay (F2): a small text-input dialog centered on screen.
fn rename_overlay(state: IdeState) -> impl IntoView {
    use floem::views::{Decorators, container, stack, label, text_input};
    use floem::reactive::{SignalGet, SignalUpdate};

    let open     = state.rename_open;
    let query    = state.rename_query;
    let target   = state.rename_target;
    let lsp_cmd  = state.lsp_cmd.clone();
    let cursor   = state.active_cursor;
    let ws       = state.workspace_root.get_untracked();

    let input = text_input(query)
        .style(|s| s.width(320.0).padding(8.0).font_size(14.0));

    let confirm = {
        let lsp_cmd2 = lsp_cmd.clone();
        label(|| "Rename".to_string())
            .style(|s| s.padding_horiz(16.0).padding_vert(6.0)
                .background(floem::peniko::Color::from_rgb8(80, 160, 255))
                .color(floem::peniko::Color::WHITE)
                .border_radius(4.0)
                .cursor(floem::style::CursorStyle::Pointer))
            .on_click_stop(move |_| {
                let new_name = query.get_untracked();
                let _old     = target.get_untracked();
                if let Some((path, line, col)) = cursor.get_untracked() {
                    let _ = lsp_cmd2.send(LspCommand::RequestRename {
                        path, line, col, new_name,
                        workspace_root: ws.clone(),
                    });
                }
                open.set(false);
            })
    };

    let cancel = label(|| "Cancel".to_string())
        .style(|s| s.padding_horiz(16.0).padding_vert(6.0)
            .background(floem::peniko::Color::from_rgba8(255, 255, 255, 30))
            .border_radius(4.0)
            .cursor(floem::style::CursorStyle::Pointer))
        .on_click_stop(move |_| { open.set(false); });

    let title = label(move || format!("Rename '{}'", target.get()))
        .style(|s| s.font_size(13.0).color(floem::peniko::Color::from_rgb8(180, 200, 230)).margin_bottom(8.0));

    let dialog = container(
        stack((
            title,
            input,
            stack((confirm, cancel))
                .style(|s| s.flex_row().gap(8.0).margin_top(10.0).justify_end()),
        )).style(|s| s.flex_col().gap(4.0))
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
            s.absolute().inset(0)
             .items_center().justify_center()
             .z_index(420)
             .background(floem::peniko::Color::from_rgba8(0, 0, 0, 130))
             .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

/// Signature-help tooltip (Ctrl+Shift+Space): shows function signature at bottom of editor area.
fn sig_help_overlay(state: IdeState) -> impl IntoView {
    use floem::views::{Decorators, container, label};
    use floem::reactive::SignalGet;

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
        .style(|s| s.font_size(12.0).color(floem::peniko::Color::from_rgb8(200, 220, 255)))
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
            s.absolute().inset(0)
             .items_end().justify_center()
             .padding_bottom(80.0)
             .z_index(380)
             .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| sig_help.set(None))
}

// ── Toast notification overlay ────────────────────────────────────────────────

fn toast_overlay(state: IdeState) -> impl IntoView {
    let toast = state.status_toast;
    let theme  = state.theme;

    container(
        label(move || toast.get().unwrap_or_default())
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(13.0)
                 .color(p.text_primary)
            })
    )
    .style(move |s| {
        let shown = toast.get().is_some();
        let p = theme.get().palette;
        s.absolute()
         .inset_bottom(40.0)
         .inset_left(320.0)
         .z_index(450)
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
    use floem::views::{Decorators, container, dyn_stack, empty, label, scroll, text_input};
    use floem::reactive::{SignalGet, SignalUpdate};

    let open       = state.ws_syms_open;
    let query      = state.ws_syms_query;
    let symbols    = state.workspace_symbols;
    let theme      = state.theme;
    let lsp_cmd    = state.lsp_cmd.clone();
    let goto_line  = state.goto_line;

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
                        label(move || kind.clone())
                            .style(move |s| {
                                let p = row_theme.get().palette;
                                s.font_size(10.0).color(p.accent).width(44.0)
                            }),
                        label(move || name.clone())
                            .style(move |s| {
                                let p = row_theme.get().palette;
                                s.font_size(13.0).color(p.text_primary)
                            }),
                        label(move || format!("  :{line}"))
                            .style(move |s| {
                                let p = row_theme.get().palette;
                                s.font_size(11.0).color(p.text_muted)
                            }),
                    ))
                    .style(|s| s.items_center().padding_vert(2.0))
                )
                .style(move |s| {
                    let p = row_theme.get().palette;
                    s.padding_horiz(12.0).padding_vert(4.0)
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
        .style(|s| s.flex_col().width_full())
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
                    Key::Named(NamedKey::Escape) => { open.set(false); }
                    Key::Named(NamedKey::Enter)  => { open.set(false); }
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
        stack((
            label(|| "Workspace Symbols")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(11.0).color(p.text_muted).padding_horiz(12.0).padding_vert(6.0)
                }),
        ))
        .style(|s| s.width_full()),
        search_box,
        container(empty())
            .style(move |s| s.height(1.0).width_full().background(theme.get().palette.border)),
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
            s.absolute().inset(0)
             .items_start().justify_center()
             .padding_top(80.0)
             .z_index(460)
             .background(floem::peniko::Color::from_rgba8(0, 0, 0, 140))
             .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

// ── Branch picker overlay (click branch in status bar) ───────────────────────

fn branch_picker_overlay(state: IdeState) -> impl IntoView {
    let open         = state.branch_picker_open;
    let branches     = state.branch_list;
    let current      = state.git_branch;
    let theme        = state.theme;
    let workspace    = state.workspace_root;
    let toast        = state.status_toast;

    let rows = scroll(
        dyn_stack(
            move || branches.get(),
            |b| b.clone(),
            move |branch| {
                let b_label   = branch.clone();
                let b_current = branch.clone();
                let b_click   = branch.clone();
                let is_current = move || current.get() == b_current;
                let hov = create_rw_signal(false);
                let ws  = workspace;
                let cur = current;
                let tst = toast;

                container(stack((
                    label(move || if is_current() { "✓ " } else { "  " }.to_string())
                        .style(move |s| s.font_size(12.0).color(theme.get().palette.success).width(20.0)),
                    label(move || b_label.clone())
                        .style(move |s| s.font_size(13.0).color(theme.get().palette.text_primary)),
                )).style(|s| s.items_center()))
                .style(move |s| {
                    let p = theme.get().palette;
                    s.padding_horiz(12.0).padding_vert(6.0)
                     .cursor(floem::style::CursorStyle::Pointer)
                     .background(if hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                })
                .on_click_stop(move |_| {
                    let branch_name = b_click.clone();
                    let toast_name  = b_click.clone();
                    open.set(false);
                    let root = ws.get();
                    let scope = Scope::new();
                    let send = create_ext_action(scope, move |result: Result<String, String>| {
                        match result {
                            Ok(new_branch) => {
                                cur.set(new_branch);
                                show_toast(tst, format!("Switched to {toast_name}"));
                            }
                            Err(e) => {
                                show_toast(tst, format!("Checkout failed: {e}"));
                            }
                        }
                    });
                    std::thread::spawn(move || {
                        let out = std::process::Command::new("git")
                            .args(["checkout", &branch_name])
                            .current_dir(&root)
                            .output();
                        match out {
                            Ok(o) if o.status.success() => send(Ok(branch_name)),
                            Ok(o) => send(Err(String::from_utf8_lossy(&o.stderr).trim().to_string())),
                            Err(e) => send(Err(e.to_string())),
                        }
                    });
                })
                .on_event_stop(EventListener::PointerEnter, move |_| hov.set(true))
                .on_event_stop(EventListener::PointerLeave, move |_| hov.set(false))
            },
        )
        .style(|s| s.flex_col().width_full())
    )
    .style(|s| s.max_height(320.0).width_full());

    let dialog = stack((
        label(|| "Switch Branch")
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0).color(p.text_muted)
                 .padding_horiz(12.0).padding_vert(8.0)
                 .font_weight(floem::text::Weight::BOLD)
            }),
        container(empty())
            .style(move |s| s.height(1.0).width_full().background(theme.get().palette.border)),
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
            s.absolute().inset(0)
             .items_end().justify_start()
             .padding_bottom(30.0)
             .padding_left(8.0)
             .z_index(470)
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
    );

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
                                    let _ = s3.lsp_cmd.send(LspCommand::RequestDefinition { path, line, col });
                                }
                            }))
                            .entry(MenuItem::new("Find All References\tShift+F12").action(move || {
                                if let Some((path, line, col)) = s4.active_cursor.get() {
                                    let _ = s4.lsp_cmd.send(LspCommand::RequestReferences { path, line, col });
                                    s4.show_bottom_panel.set(true);
                                    s4.bottom_panel_tab.set(Tab::References);
                                }
                            }))
                            .entry(MenuItem::new("Rename Symbol\tF2").action(move || {
                                s5.rename_open.set(true);
                            }))
                            .entry(MenuItem::new("Code Actions\tCtrl+.").action(move || {
                                if let Some((path, line, col)) = s6.active_cursor.get() {
                                    let _ = s6.lsp_cmd.send(LspCommand::RequestCodeActions { path, line, col });
                                }
                            }))
                            .separator()
                            .entry(MenuItem::new("Toggle Comment\tCtrl+/").action(move || {
                                s7.comment_toggle_nonce.update(|v| *v += 1);
                            }));
                        show_context_menu(menu, None);
                    }
                }
            })
    };

    let chat = chat_panel(state.theme, state.ai_thinking);

    let chat_wrap = container(chat)
        .style(move |s| {
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
             .apply_if(!state.show_right_panel.get(), |s| s.display(floem::style::Display::None))
        });

    // Drag handle between left panel and editor.
    let divider = {
        let style_s = state.clone();
        let down_s  = state.clone();
        container(empty())
            .style(move |s| {
                let t = style_s.theme.get();
                let active = style_s.panel_drag_active.get();
                let shown  = style_s.show_left_panel.get();
                s.width(4.0)
                 .height_full()
                 .cursor(floem::style::CursorStyle::ColResize)
                 .background(t.palette.glass_border.with_alpha(if active { 0.8 } else { 0.0 }))
                 .apply_if(!shown, |s| s.display(floem::style::Display::None))
            })
            .on_event_stop(EventListener::PointerDown, move |e| {
                if let Event::PointerDown(pe) = e {
                    down_s.panel_drag_active.set(true);
                    down_s.panel_drag_start_x.set(pe.pos.x);
                    down_s.panel_drag_start_width.set(down_s.left_panel_width.get());
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

    // Main content row: activity bar + left panel + resize handle + editor + chat
    let content_row = stack((
        activity_wrap,
        left_wrap,
        divider_wrap,
        editor,
        chat_zen_wrap,
    ))
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    // Bottom panel (terminal etc.)
    let bottom_raw = bottom_panel(state.clone());
    let bottom = container(bottom_raw)
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));

    let state_for_status = state.clone();
    let status_raw = status_bar(state_for_status);
    let status_wrap = container(status_raw)
        .style(move |s| s.apply_if(zen.get(), |s| s.display(floem::style::Display::None)));

    stack((content_row, bottom, status_wrap))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            s.flex_col()
             .width_full()
             .height_full()
             .background(if t.is_cosmic() { floem::peniko::Color::TRANSPARENT } else { p.bg_base })
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
                sty.padding_horiz(12.0)
                    .height(24.0)
                    .items_center()
                    .justify_center()
                    .cursor(floem::style::CursorStyle::Pointer)
                    .font_size(12.0)
                    .color(p.text_secondary)
                    .apply_if(hovered.get(), |s| {
                        s.background(p.bg_elevated).color(p.text_primary)
                    })
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { hovered.set(true); })
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { hovered.set(false); })
    };

    // ── File menu ────────────────────────────────────────────────────────────
    let file_item = {
        let s = state.clone();
        make_item("File", state.theme)
            .on_click_stop(move |_| {
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
        make_item("Edit", state.theme)
            .on_click_stop(move |_| {
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
                    .entry(MenuItem::new("Command Palette\tCtrl+Shift+P").action(move || {
                        s4.command_palette_open.set(true);
                    }));
                show_context_menu(menu, None);
            })
    };

    // ── View menu ────────────────────────────────────────────────────────────
    let view_item = {
        let s = state.clone();
        make_item("View", state.theme)
            .on_click_stop(move |_| {
                let s_exp  = s.clone();
                let s_term = s.clone();
                let s_chat = s.clone();
                let s_zen  = s.clone();
                let s_zin  = s.clone();
                let s_zout = s.clone();
                // Theme submenu
                let theme_menu = Menu::new("Theme")
                    .entry(MenuItem::new("Midnight Blue").action({ let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::MidnightBlue)); } }))
                    .entry(MenuItem::new("Cyberpunk 2077").action({ let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Cyberpunk)); } }))
                    .entry(MenuItem::new("Synthwave '84").action({  let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Synthwave84)); } }))
                    .entry(MenuItem::new("Andromeda").action({      let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Andromeda)); } }))
                    .entry(MenuItem::new("Dark").action({           let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dark)); } }))
                    .entry(MenuItem::new("Dracula").action({        let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dracula)); } }))
                    .entry(MenuItem::new("Tokyo Night").action({    let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::TokyoNight)); } }))
                    .entry(MenuItem::new("Monokai").action({        let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Monokai)); } }))
                    .entry(MenuItem::new("Nord Dark").action({      let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::NordDark)); } }))
                    .entry(MenuItem::new("Matrix Green").action({   let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::MatrixGreen)); } }))
                    .entry(MenuItem::new("Root Shell").action({     let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::RootShell)); } }))
                    .entry(MenuItem::new("Light").action({          let s = s.clone(); move || { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Light)); } }));

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
        make_item("Go", state.theme)
            .on_click_stop(move |_| {
                let s_def = s.clone();
                let s_sym = s.clone();
                let s_fp  = s.clone();
                let menu = Menu::new("Go")
                    .entry(MenuItem::new("Go to Definition\tF12").action(move || {
                        if let Some((path, line, col)) = s_def.active_cursor.get() {
                            let _ = s_def.lsp_cmd.send(LspCommand::RequestDefinition { path, line, col });
                        }
                    }))
                    .entry(MenuItem::new("Find All References\tShift+F12").action(move || {
                        if let Some((path, line, col)) = s_sym.active_cursor.get() {
                            let _ = s_sym.lsp_cmd.send(LspCommand::RequestReferences { path, line, col });
                            s_sym.references_visible.set(true);
                            s_sym.show_bottom_panel.set(true);
                            s_sym.bottom_panel_tab.set(Tab::References);
                        }
                    }))
                    .entry(MenuItem::new("Workspace Symbols\tCtrl+T").action(move || {
                        s_fp.ws_syms_open.set(true);
                        s_fp.ws_syms_query.set(String::new());
                        let _ = s_fp.lsp_cmd.send(LspCommand::RequestWorkspaceSymbols { query: String::new() });
                    }));
                show_context_menu(menu, None);
            })
    };

    // ── Run menu ─────────────────────────────────────────────────────────────
    let run_item = {
        let s = state.clone();
        make_item("Run", state.theme)
            .on_click_stop(move |_| {
                let s_run   = s.clone();
                let s_build = s.clone();
                let s_test  = s.clone();
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
                    .entry(MenuItem::new("Show Problems\tCtrl+Shift+M").action(move || {
                        s_test.show_bottom_panel.set(true);
                        s_test.bottom_panel_tab.set(Tab::Problems);
                    }));
                show_context_menu(menu, None);
            })
    };

    // ── Help menu ────────────────────────────────────────────────────────────
    let help_item = {
        let s = state.clone();
        make_item("Help", state.theme)
            .on_click_stop(move |_| {
                let s2 = s.clone();
                let menu = Menu::new("Help")
                    .entry(MenuItem::new("Command Palette\tCtrl+Shift+P").action(move || {
                        s2.command_palette_open.set(true);
                    }))
                    .separator()
                    .entry(MenuItem::new("About PhazeAI IDE").action(|| {
                        // TODO: about dialog
                    }));
                show_context_menu(menu, None);
            })
    };

    // ── Bar layout ───────────────────────────────────────────────────────────
    let bar_state = state.clone();
    stack((
        file_item,
        edit_item,
        view_item,
        go_item,
        run_item,
        help_item,
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
                let palette    = command_palette(state.clone());
                let picker     = file_picker(state.clone());
                let completions_popup = completion_popup(state.clone());
                let hover_tip  = hover_tooltip(state.clone());
                let inline_edit = inline_edit_overlay(state.clone());
                let code_actions_popup = code_actions_overlay(state.clone());
                let rename_popup = rename_overlay(state.clone());
                let sig_help_popup = sig_help_overlay(state.clone());
                let toast_popup = toast_overlay(state.clone());
                let ws_syms_popup = workspace_symbols_overlay(state.clone());
                let branch_picker_popup = branch_picker_overlay(state.clone());

                // Full-window drag capture overlay — only visible while a panel
                // resize is in progress (panel_drag_active == true).  By covering
                // the entire window it intercepts PointerMove/PointerUp even when
                // the cursor has moved past the divider into the editor area.
                let drag_overlay = {
                    let style_s = state.clone();
                    let move_s  = state.clone();
                    let up_s    = state.clone();
                    container(empty())
                        .style(move |s| {
                            let active = style_s.panel_drag_active.get();
                            s.absolute()
                             .inset(0)
                             .z_index(50)
                             .cursor(floem::style::CursorStyle::ColResize)
                             .apply_if(!active, |s| s.display(floem::style::Display::None))
                        })
                        .on_event_stop(EventListener::PointerMove, move |e| {
                            if let Event::PointerMove(pe) = e {
                                let delta = pe.pos.x - move_s.panel_drag_start_x.get();
                                let new_w = (move_s.panel_drag_start_width.get() + delta)
                                    .max(80.0)
                                    .min(700.0);
                                move_s.left_panel_width.set(new_w);
                                move_s.show_left_panel.set(true);
                            }
                        })
                        .on_event_stop(EventListener::PointerUp, move |_| {
                            up_s.panel_drag_active.set(false);
                        })
                };

                // Root: cosmic canvas + menu bar + IDE + overlays (overlays use z_index)
                let ide_with_menu = stack((
                    menu_bar(state.clone()),
                    ide_root(state.clone()),
                ))
                .style(|s| s.flex_col().width_full().height_full());

                stack((
                    cosmic_bg_canvas(state.theme),
                    ide_with_menu,
                    palette,            // z_index(100)
                    picker,             // z_index(200) — on top of palette
                    hover_tip,          // z_index(250) — LSP hover doc
                    completions_popup,  // z_index(300) — above palette/picker
                    code_actions_popup, // z_index(350) — code actions / quick-fix
                    sig_help_popup,     // z_index(380) — signature help tooltip
                    inline_edit,        // z_index(400) — highest overlay
                    rename_popup,       // z_index(420) — rename dialog
                    toast_popup,        // z_index(450) — toast notifications
                    ws_syms_popup,      // z_index(460) — workspace symbols (Ctrl+T)
                    branch_picker_popup, // z_index(470) — branch switcher
                    drag_overlay,       // z_index(50)  — only shown during resize
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
                            let ctrl  = key_event.modifiers.contains(Modifiers::CONTROL);
                            let shift = key_event.modifiers.contains(Modifiers::SHIFT);
                            let alt   = key_event.modifiers.contains(Modifiers::ALT);

                            // ── Named keys ───────────────────────────────────────
                            if let Key::Named(ref named) = key_event.key.logical_key {
                                match named {
                                    floem::keyboard::NamedKey::Escape => {
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
                                        // Vim: Escape enters Normal mode
                                        if state.vim_mode.get() {
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
                                            let items    = state.completions.get();
                                            let sel      = state.completion_selected.get();
                                            let prefix_b = state.completion_filter_text.get().len();
                                            if let Some(entry) = items.get(sel) {
                                                let text = if entry.insert_text.is_empty() {
                                                    entry.label.clone()
                                                } else {
                                                    entry.insert_text.clone()
                                                };
                                                state.pending_completion.set(Some((text, prefix_b)));
                                            }
                                            state.completion_open.set(false);
                                            state.completion_filter_text.set(String::new());
                                            return;
                                        }
                                    }
                                    floem::keyboard::NamedKey::Enter => {
                                        if state.completion_open.get() {
                                            let items    = state.completions.get();
                                            let sel      = state.completion_selected.get();
                                            let prefix_b = state.completion_filter_text.get().len();
                                            if let Some(entry) = items.get(sel) {
                                                let text = if entry.insert_text.is_empty() {
                                                    entry.label.clone()
                                                } else {
                                                    entry.insert_text.clone()
                                                };
                                                state.pending_completion.set(Some((text, prefix_b)));
                                            }
                                            state.completion_open.set(false);
                                            state.completion_filter_text.set(String::new());
                                            return;
                                        }
                                    }
                                    // F12 — go to definition; Shift+F12 — find all references
                                    floem::keyboard::NamedKey::F12 => {
                                        if let Some((path, line, col)) = state.active_cursor.get() {
                                            if shift {
                                                // Shift+F12: find all references
                                                let _ = state.lsp_cmd.send(
                                                    LspCommand::RequestReferences { path, line, col }
                                                );
                                                state.references_visible.set(true);
                                                state.show_bottom_panel.set(true);
                                                state.bottom_panel_tab.set(Tab::References);
                                            } else {
                                                // F12: go to definition
                                                let _ = state.lsp_cmd.send(
                                                    LspCommand::RequestDefinition { path, line, col }
                                                );
                                            }
                                        }
                                        return;
                                    }
                                    // F1 with Ctrl — show hover documentation
                                    floem::keyboard::NamedKey::F1 => {
                                        if ctrl {
                                            if let Some((path, line, col)) = state.active_cursor.get() {
                                                let _ = state.lsp_cmd.send(
                                                    LspCommand::RequestHover { path, line, col }
                                                );
                                            }
                                            return;
                                        }
                                    }
                                    // F2 — rename symbol at cursor
                                    floem::keyboard::NamedKey::F2 => {
                                        if let Some((path, line, col)) = state.active_cursor.get() {
                                            // Prefill rename box with the word under cursor
                                            let word = std::fs::read_to_string(&path).ok()
                                                .and_then(|content| {
                                                    let target_line = content.lines().nth(line as usize)?.to_string();
                                                    let col = (col as usize).min(target_line.len());
                                                    let start = target_line[..col].char_indices().rev()
                                                        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
                                                        .last().map(|(i, _)| i).unwrap_or(col);
                                                    let end = target_line[col..].char_indices()
                                                        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
                                                        .last().map(|(i, _)| col + i + 1).unwrap_or(col);
                                                    let w = target_line[start..end].to_string();
                                                    if w.is_empty() { None } else { Some(w) }
                                                })
                                                .unwrap_or_default();
                                            state.rename_target.set(word.clone());
                                            state.rename_query.set(word);
                                            state.rename_open.set(true);
                                        }
                                        return;
                                    }
                                    _ => {}
                                }
                            }

                            // Ctrl+Space → request LSP completions and open popup
                            if ctrl && key_event.key.logical_key == Key::Named(floem::keyboard::NamedKey::Space) {
                                if let Some((path, line, col)) = state.active_cursor.get() {
                                    // Compute word before cursor as the filter prefix.
                                    let prefix = std::fs::read_to_string(&path)
                                        .ok()
                                        .and_then(|content| {
                                            let lines: Vec<&str> = content.lines().collect();
                                            let line_str = lines.get(line as usize)?;
                                            let col = (col as usize).min(line_str.len());
                                            let prefix: String = line_str[..col]
                                                .chars().rev()
                                                .take_while(|c| c.is_alphanumeric() || *c == '_')
                                                .collect::<String>()
                                                .chars().rev().collect();
                                            Some(prefix)
                                        })
                                        .unwrap_or_default();
                                    state.completion_filter_text.set(prefix);
                                    let _ = state.lsp_cmd.send(LspCommand::RequestCompletions {
                                        path, line, col,
                                    });
                                }
                                state.completion_selected.set(0);
                                state.completion_open.set(true);
                                return;
                            }

                            // Ctrl+T → workspace symbols overlay
                            if ctrl && !shift && key_event.key.logical_key == Key::Character("t".into()) {
                                let open = state.ws_syms_open.get();
                                state.ws_syms_open.set(!open);
                                if !open {
                                    state.ws_syms_query.set(String::new());
                                    // Kick off an empty-query search to pre-populate list.
                                    let _ = state.lsp_cmd.send(LspCommand::RequestWorkspaceSymbols {
                                        query: String::new(),
                                    });
                                }
                                return;
                            }

                            // Ctrl+. → code actions
                            if ctrl && key_event.key.logical_key == Key::Character(".".into()) {
                                if let Some((path, line, col)) = state.active_cursor.get() {
                                    let _ = state.lsp_cmd.send(
                                        LspCommand::RequestCodeActions { path, line, col }
                                    );
                                }
                                state.code_actions_open.set(true);
                                return;
                            }

                            // Ctrl+Shift+Space → signature help
                            if ctrl && shift && key_event.key.logical_key == Key::Named(floem::keyboard::NamedKey::Space) {
                                if let Some((path, line, col)) = state.active_cursor.get() {
                                    let _ = state.lsp_cmd.send(
                                        LspCommand::RequestSignatureHelp { path, line, col }
                                    );
                                }
                                return;
                            }

                            // Ctrl+P → file picker; Ctrl+Shift+P → command palette
                            if ctrl && key_event.key.logical_key == Key::Character("p".into()) {
                                if shift {
                                    // Ctrl+Shift+P: command palette
                                    let open = state.command_palette_open.get();
                                    state.command_palette_open.set(!open);
                                } else {
                                    // Ctrl+P: file picker
                                    let open = state.file_picker_open.get();
                                    state.file_picker_open.set(!open);
                                    if !open {
                                        state.file_picker_query.set(String::new());
                                    }
                                }
                                return;
                            }

                            // ── Character key combos ─────────────────────────────
                            if let Key::Character(ref ch) = key_event.key.logical_key {
                                let ch = ch.clone();

                                // Alt+Z — toggle word wrap
                                if alt && !ctrl && !shift && ch.as_str() == "z" {
                                    state.word_wrap.update(|v| *v = !*v);
                                    let msg = if state.word_wrap.get() { "Word wrap on" } else { "Word wrap off" };
                                    show_toast(state.status_toast, msg);
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
                                            state.font_size.update(|v| *v = v.saturating_sub(1).max(8));
                                            return;
                                        }
                                        // Ctrl+0 — reset editor font to default
                                        "0" => {
                                            state.font_size.set(14);
                                            return;
                                        }
                                        // Ctrl+D — select next occurrence (multi-cursor)
                                        "d" => {
                                            state.ctrl_d_nonce.update(|v| *v += 1);
                                            return;
                                        }
                                        // Ctrl+B — toggle left sidebar
                                        "b" => {
                                            state.show_left_panel.update(|v| *v = !*v);
                                            let now_open = state.show_left_panel.get();
                                            let new_w = if now_open { 260.0 } else { 0.0 };
                                            state.left_panel_width.set(new_w);
                                            session_save_full(
                                                state.open_file.get().as_ref(),
                                                new_w, state.show_bottom_panel.get(),
                                                state.vim_mode.get(),
                                                state.theme.get().variant.name(),
                                            );
                                        }
                                        // Ctrl+J — toggle bottom terminal panel
                                        "j" => {
                                            state.show_bottom_panel.update(|v| *v = !*v);
                                            session_save_full(
                                                state.open_file.get().as_ref(),
                                                state.left_panel_width.get(),
                                                state.show_bottom_panel.get(),
                                                state.vim_mode.get(),
                                                state.theme.get().variant.name(),
                                            );
                                        }
                                        // Ctrl+\ — toggle right chat panel
                                        "\\" => {
                                            state.show_right_panel.update(|v| *v = !*v);
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
                                }

                                // ── Vim normal-mode keys (no Ctrl) ───────────────
                                if state.vim_mode.get() && state.vim_normal_mode.get()
                                    && !ctrl && !alt
                                {
                                    let pending = state.vim_pending_key.get();
                                    let ch_str = ch.as_str();

                                    // Two-key sequences
                                    if let Some(prev) = pending {
                                        state.vim_pending_key.set(None);
                                        match (prev, ch_str) {
                                            ('d', "d") => { state.vim_motion.set(Some(VimMotion::DeleteLine)); }
                                            ('g', "g") => { state.vim_motion.set(Some(VimMotion::LineStart)); }
                                            ('y', "y") => { state.vim_motion.set(Some(VimMotion::YankLine)); }
                                            _ => {}
                                        }
                                        return;
                                    }

                                    // Single-key normal mode commands
                                    match ch_str {
                                        "h" => { state.vim_motion.set(Some(VimMotion::Left)); return; }
                                        "j" => { state.vim_motion.set(Some(VimMotion::Down)); return; }
                                        "k" => { state.vim_motion.set(Some(VimMotion::Up)); return; }
                                        "l" => { state.vim_motion.set(Some(VimMotion::Right)); return; }
                                        "w" => { state.vim_motion.set(Some(VimMotion::WordForward)); return; }
                                        "b" => { state.vim_motion.set(Some(VimMotion::WordBackward)); return; }
                                        "0" => { state.vim_motion.set(Some(VimMotion::LineStart)); return; }
                                        "$" => { state.vim_motion.set(Some(VimMotion::LineEnd)); return; }
                                        "x" => { state.vim_motion.set(Some(VimMotion::DeleteChar)); return; }
                                        "i" => {
                                            state.vim_normal_mode.set(false);
                                            state.vim_motion.set(Some(VimMotion::EnterInsert));
                                            return;
                                        }
                                        "a" => {
                                            state.vim_normal_mode.set(false);
                                            state.vim_motion.set(Some(VimMotion::EnterInsertAfter));
                                            return;
                                        }
                                        "o" => {
                                            state.vim_normal_mode.set(false);
                                            state.vim_motion.set(Some(VimMotion::EnterInsertNewlineBelow));
                                            return;
                                        }
                                        // p / P — paste from vim register
                                        "p" => { state.vim_motion.set(Some(VimMotion::Paste)); return; }
                                        "P" => { state.vim_motion.set(Some(VimMotion::PasteBefore)); return; }
                                        // d, g, y — set pending key for two-key sequences
                                        "d" | "g" | "y" => {
                                            state.vim_pending_key.set(Some(ch_str.chars().next().unwrap()));
                                            return;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
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
