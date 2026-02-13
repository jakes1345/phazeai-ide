use eframe::egui;
use egui::RichText;
use std::path::PathBuf;
use tokio::sync::mpsc;

use phazeai_core::{AgentEvent, Settings};

use crate::keybindings::{self, Action};
use crate::panels::chat::ChatPanel;
use crate::panels::editor::EditorPanel;
use crate::panels::explorer::ExplorerPanel;
use crate::panels::settings::SettingsPanel;
use crate::panels::terminal::TerminalPanel;
use crate::themes::{ThemeColors, ThemePreset};

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

    // Tokio runtime for async operations
    runtime: tokio::runtime::Runtime,

    // Settings
    settings: Settings,
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

            theme_preset,
            theme,

            explorer_width: 220.0,
            chat_width: 320.0,
            terminal_height: 200.0,

            event_rx: Some(event_rx),
            event_tx,

            runtime,

            settings,
        }
    }

    fn handle_keybindings(&mut self, ctx: &egui::Context) {
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
                    // TODO: command palette
                }
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

            self.runtime.spawn(async move {
                match settings.build_llm_client() {
                    Ok(llm) => {
                        let agent = phazeai_core::Agent::new(llm)
                            .with_system_prompt(
                                "You are PhazeAI, an AI coding assistant. Help the user with their code."
                            );
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
                    // TODO: about dialog
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

        // Settings window (floating)
        self.settings_panel.show(ctx, &self.theme);

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
