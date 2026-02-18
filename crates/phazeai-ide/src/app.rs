use eframe::egui;
use egui::RichText;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::sync::{mpsc, oneshot};

use phazeai_core::{AgentEvent, Settings};

use crate::keybindings::{self, Action};
use crate::panels::chat::ChatPanel;
use crate::panels::editor::EditorPanel;
use crate::panels::explorer::ExplorerPanel;
use crate::panels::settings::SettingsPanel;
use crate::panels::terminal::TerminalPanel;
use crate::themes::{ThemeColors, ThemePreset};

#[derive(Clone)]
#[allow(dead_code)]
pub struct PendingApproval {
    pub tool_name: String,
    pub params_summary: String,
}

// Shared state for handling approval requests
struct ApprovalState {
    pending: Option<(String, String, oneshot::Sender<bool>)>, // (tool_name, params, response_tx)
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
    OpenSettings,
}

pub struct PhazeApp {
    // Panels
    editor: EditorPanel,
    explorer: ExplorerPanel,
    chat: ChatPanel,
    terminal: TerminalPanel,
    settings_panel: SettingsPanel,

    // Panel visibility
    show_explorer: bool,
    show_chat: bool,
    show_terminal: bool,
    show_about: bool,

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

    // Agent communication
    event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,

    // Tool approval
    pending_approval: Option<PendingApproval>,
    approval_state: Arc<Mutex<ApprovalState>>,

    // Tokio runtime for async operations
    runtime: tokio::runtime::Runtime,

    // Settings
    settings: Settings,

    // File watcher
    file_mod_times: HashMap<PathBuf, SystemTime>,
    last_file_check: std::time::Instant,
}

impl PhazeApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, settings: Settings) -> Self {
        let theme_preset = resolve_theme_preset(&settings.editor.theme);
        let theme = ThemeColors::from_preset(&theme_preset);

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let runtime = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");

        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));

        let mut explorer = ExplorerPanel::new();
        explorer.set_root(cwd.clone());

        let mut terminal = TerminalPanel::new();
        terminal.set_cwd(cwd);

        Self {
            editor: EditorPanel::new(settings.editor.font_size),
            explorer,
            chat: ChatPanel::new(),
            terminal,
            settings_panel: SettingsPanel::new(settings.clone()),

            show_explorer: true,
            show_chat: true,
            show_terminal: true,
            show_about: false,

            show_palette: false,
            palette_query: String::new(),
            palette_selected: 0,
            palette_files: Vec::new(),
            palette_just_opened: false,

            theme_preset,
            theme,

            explorer_width: 220.0,
            chat_width: 320.0,
            terminal_height: 200.0,

            event_rx: Some(event_rx),
            event_tx,

            pending_approval: None,
            approval_state: Arc::new(Mutex::new(ApprovalState { pending: None })),

            runtime,

            settings,

            file_mod_times: HashMap::new(),
            last_file_check: std::time::Instant::now(),
        }
    }

    /// Check open editor files for disk modifications (e.g., from agent edits)
    fn check_file_changes(&mut self) {
        // Only check every 2 seconds to avoid excessive I/O
        if self.last_file_check.elapsed().as_secs() < 2 {
            return;
        }
        self.last_file_check = std::time::Instant::now();

        for tab in &mut self.editor.tabs {
            if let Some(ref path) = tab.path {
                if let Ok(metadata) = std::fs::metadata(path) {
                    if let Ok(modified) = metadata.modified() {
                        let prev = self.file_mod_times.get(path).cloned();
                        self.file_mod_times.insert(path.clone(), modified);

                        // If we've seen this file before and it changed, and the tab
                        // isn't dirty (user hasn't made unsaved changes), reload it
                        if let Some(prev_time) = prev {
                            if modified > prev_time && !tab.modified {
                                tracing::info!("Reloading changed file: {}", path.display());
                                tab.reload_from_disk();
                            }
                        }
                    }
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
            }
        }
    }

    fn open_file_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
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
                        // Approval is now handled in the callback via approval_state
                        // This event is just for logging/display purposes
                    }
                    AgentEvent::ToolStart { name } => {
                        self.chat.add_tool_call(&name, true, "Running...");
                    }
                    AgentEvent::ToolResult {
                        name,
                        success,
                        summary,
                    } => {
                        self.chat.add_tool_call(&name, success, &summary);
                    }
                    AgentEvent::Complete { .. } => {
                        self.chat.finish_streaming();
                    }
                    AgentEvent::Error(err) => {
                        self.chat.finish_streaming();
                        self.chat.add_assistant_message(&format!("Error: {}", err));
                    }
                }
            }
        }
    }

    fn process_chat_messages(&mut self) {
        if let Some(msg) = self.chat.pending_send.take() {
            let tx = self.event_tx.clone();
            let settings = self.settings.clone();
            let approval_state = self.approval_state.clone();

            self.runtime.spawn(async move {
                match settings.build_llm_client() {
                    Ok(llm) => {
                        // Create approval callback that uses the shared state
                        let approval_fn: phazeai_core::agent::ApprovalFn = Box::new(move |tool_name, params| {
                            let approval_state = approval_state.clone();
                            Box::pin(async move {
                                // Format params for display
                                let params_summary = if params.is_null() {
                                    String::new()
                                } else {
                                    match serde_json::to_string_pretty(&params) {
                                        Ok(s) => {
                                            if s.len() > 200 {
                                                format!("{}...", &s[..200])
                                            } else {
                                                s
                                            }
                                        }
                                        Err(_) => format!("{:?}", params),
                                    }
                                };

                                // Create a oneshot channel for the response
                                let (response_tx, response_rx) = oneshot::channel();

                                // Store the approval request in shared state
                                {
                                    let mut state = approval_state.lock().unwrap();
                                    state.pending = Some((tool_name, params_summary, response_tx));
                                }

                                // Wait for the UI to respond (with timeout)
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(300), // 5 minute timeout
                                    response_rx
                                ).await {
                                    Ok(Ok(approved)) => approved,
                                    _ => false, // Timeout or channel closed = deny
                                }
                            })
                        });

                        let agent = phazeai_core::Agent::new(llm)
                            .with_system_prompt(
                                "You are PhazeAI, an AI coding assistant. Help the user with their code."
                            )
                            .with_approval(approval_fn);

                        if let Err(e) = agent.run_with_events(&msg, tx.clone()).await {
                            let _ = tx.send(AgentEvent::Error(e.to_string()));
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(AgentEvent::Error(format!("LLM client error: {e}")));
                    }
                }
            });
        }
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
        }
    }

    fn handle_approval_response(&mut self, approved: bool) {
        // Clear the UI state
        self.pending_approval = None;

        // Send the approval response through the shared state
        if let Ok(mut state) = self.approval_state.lock() {
            if let Some((_, _, tx)) = state.pending.take() {
                let _ = tx.send(approved);
            }
        }
    }

    fn check_approval_requests(&mut self) {
        // Check if there's a new approval request from the agent
        if let Ok(state) = self.approval_state.lock() {
            if let Some((ref tool_name, ref params, _)) = state.pending {
                // Only update UI if we don't already have a pending approval
                if self.pending_approval.is_none() {
                    self.pending_approval = Some(PendingApproval {
                        tool_name: tool_name.clone(),
                        params_summary: params.clone(),
                    });
                }
            }
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
                ("Settings", PaletteAction::OpenSettings),
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
            PaletteAction::OpenSettings => self.settings_panel.toggle(),
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
                        self.terminal.set_cwd(path);
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
                    .checkbox(&mut self.show_explorer, "Explorer (Ctrl+E)")
                    .clicked()
                {
                    ui.close_menu();
                }
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

    fn render_status_bar(&self, ui: &mut egui::Ui) {
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
                    ui.colored_label(self.theme.warning, RichText::new("Modified").small());
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(
                    self.theme.text_muted,
                    RichText::new("PhazeAI IDE v0.1.0").small(),
                );
            });
        });
    }
}

impl eframe::App for PhazeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme
        self.theme.apply(ctx);

        // Handle keybindings
        self.handle_keybindings(ctx);

        // Process events
        self.process_agent_events();
        self.process_chat_messages();
        self.check_settings_changes();
        self.check_explorer_file_open();
        self.check_approval_requests();
        self.check_file_changes();

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

        // Explorer panel on the left
        if self.show_explorer {
            egui::SidePanel::left("explorer_panel")
                .resizable(true)
                .default_width(self.explorer_width)
                .min_width(150.0)
                .max_width(400.0)
                .show(ctx, |ui| {
                    let theme = self.theme.clone();
                    self.explorer.show(ui, &theme);
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

        // Central panel - editor
        egui::CentralPanel::default().show(ctx, |ui| {
            let theme = self.theme.clone();
            self.editor.show(ui, &theme);
        });

        // Request repaint for streaming updates
        if self.chat.is_streaming {
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
