use egui::{self, RichText};
use phazeai_core::config::{LlmProvider, Settings};

use crate::keybindings::{binding_label, default_keybindings};
use crate::themes::{ThemeColors, ThemePreset};

pub struct SettingsPanel {
    pub visible: bool,
    pub settings: Settings,
    pub theme_preset: ThemePreset,
    pub settings_changed: bool,
    provider_idx: usize,
    /// Filter query for the settings search bar.
    search_query: String,
}

impl SettingsPanel {
    pub fn new(settings: Settings) -> Self {
        let provider_idx = match settings.llm.provider {
            LlmProvider::Claude => 0,
            LlmProvider::OpenAI => 1,
            LlmProvider::Ollama => 2,
            LlmProvider::Groq => 3,
            LlmProvider::Together => 4,
            LlmProvider::OpenRouter => 5,
            LlmProvider::LmStudio => 6,
        };
        Self {
            visible: false,
            settings,
            theme_preset: ThemePreset::Dark,
            settings_changed: false,
            provider_idx,
            search_query: String::new(),
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn show(&mut self, ctx: &egui::Context, theme: &ThemeColors) {
        if !self.visible {
            return;
        }

        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(true)
            .default_size([520.0, 640.0])
            .show(ctx, |ui| {
                // Search bar
                ui.horizontal(|ui| {
                    ui.colored_label(theme.text_muted, "🔍");
                    ui.add_space(4.0);
                    let search = egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("Filter settings…")
                        .desired_width(ui.available_width());
                    ui.add(search);
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                egui::ScrollArea::vertical().show(ui, |ui| {
                    let q = self.search_query.to_lowercase();

                    if self.section_matches(&q, &["appearance", "theme", "font", "size", "color"]) {
                        self.show_appearance(ui, theme);
                        ui.add_space(16.0);
                    }
                    if self.section_matches(
                        &q,
                        &[
                            "llm", "provider", "model", "api", "key", "base", "url", "tokens",
                            "claude", "openai", "ollama", "groq",
                        ],
                    ) {
                        self.show_llm(ui, theme);
                        ui.add_space(16.0);
                    }
                    if self.section_matches(
                        &q,
                        &["editor", "tab", "line", "numbers", "auto", "save", "indent"],
                    ) {
                        self.show_editor(ui, theme);
                        ui.add_space(16.0);
                    }
                    if self.section_matches(&q, &["sidecar", "python", "semantic", "search"]) {
                        self.show_sidecar(ui, theme);
                        ui.add_space(16.0);
                    }
                    if self
                        .section_matches(&q, &["keybind", "shortcut", "hotkey", "key", "binding"])
                    {
                        self.show_keybindings(ui, theme);
                        ui.add_space(16.0);
                    }

                    ui.separator();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            if let Err(e) = self.settings.save() {
                                tracing::error!("Failed to save settings: {e}");
                            }
                            self.settings_changed = true;
                        }
                        if ui.button("Close").clicked() {
                            self.visible = false;
                        }
                    });
                });
            });
    }

    fn section_matches(&self, query: &str, keywords: &[&str]) -> bool {
        query.is_empty() || keywords.iter().any(|k| query.contains(k))
    }

    fn show_appearance(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.colored_label(theme.text, RichText::new("Appearance").strong().size(16.0));
        ui.separator();

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Theme:");
            egui::ComboBox::from_id_source("theme_selector")
                .selected_text(self.theme_preset.name())
                .show_ui(ui, |ui: &mut egui::Ui| {
                    for preset in ThemePreset::all() {
                        let name = preset.name().to_string();
                        if ui
                            .selectable_value(&mut self.theme_preset, preset, &name)
                            .changed()
                        {
                            self.settings.editor.theme = name;
                            self.settings_changed = true;
                        }
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Font size:");
            if ui
                .add(egui::Slider::new(
                    &mut self.settings.editor.font_size,
                    10.0..=24.0,
                ))
                .changed()
            {
                self.settings_changed = true;
            }
        });
    }

    fn show_llm(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.colored_label(
            theme.text,
            RichText::new("LLM Provider").strong().size(16.0),
        );
        ui.separator();

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Provider:");
            let providers = [
                "Claude",
                "OpenAI",
                "Ollama",
                "Groq",
                "Together.ai",
                "OpenRouter",
                "LM Studio",
            ];
            egui::ComboBox::from_id_source("provider_selector")
                .selected_text(providers[self.provider_idx])
                .show_ui(ui, |ui: &mut egui::Ui| {
                    for (i, name) in providers.iter().enumerate() {
                        if ui
                            .selectable_value(&mut self.provider_idx, i, *name)
                            .changed()
                        {
                            self.settings.llm.provider = match i {
                                0 => LlmProvider::Claude,
                                1 => LlmProvider::OpenAI,
                                2 => LlmProvider::Ollama,
                                3 => LlmProvider::Groq,
                                4 => LlmProvider::Together,
                                5 => LlmProvider::OpenRouter,
                                _ => LlmProvider::LmStudio,
                            };
                            self.settings_changed = true;
                        }
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Model:");
            if ui
                .text_edit_singleline(&mut self.settings.llm.model)
                .changed()
            {
                self.settings_changed = true;
            }
        });

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "API key env:");
            if ui
                .text_edit_singleline(&mut self.settings.llm.api_key_env)
                .changed()
            {
                self.settings_changed = true;
            }
        });

        let mut base_url = self.settings.llm.base_url.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Base URL:");
            if ui.text_edit_singleline(&mut base_url).changed() {
                self.settings.llm.base_url = if base_url.is_empty() {
                    None
                } else {
                    Some(base_url.clone())
                };
                self.settings_changed = true;
            }
        });

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Max tokens:");
            let mut max_tokens = self.settings.llm.max_tokens as f32;
            if ui
                .add(egui::Slider::new(&mut max_tokens, 256.0..=32768.0).logarithmic(true))
                .changed()
            {
                self.settings.llm.max_tokens = max_tokens as u32;
                self.settings_changed = true;
            }
        });
    }

    fn show_editor(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.colored_label(theme.text, RichText::new("Editor").strong().size(16.0));
        ui.separator();

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Tab size:");
            let mut tab_size = self.settings.editor.tab_size as f32;
            if ui
                .add(egui::Slider::new(&mut tab_size, 2.0..=8.0).step_by(1.0))
                .changed()
            {
                self.settings.editor.tab_size = tab_size as u32;
                self.settings_changed = true;
            }
        });

        if ui
            .checkbox(
                &mut self.settings.editor.show_line_numbers,
                "Show line numbers",
            )
            .changed()
        {
            self.settings_changed = true;
        }

        if ui
            .checkbox(&mut self.settings.editor.auto_save, "Auto save")
            .changed()
        {
            self.settings_changed = true;
        }
    }

    fn show_sidecar(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.colored_label(
            theme.text,
            RichText::new("Python Sidecar").strong().size(16.0),
        );
        ui.separator();

        if ui
            .checkbox(&mut self.settings.sidecar.enabled, "Enable sidecar")
            .changed()
        {
            self.settings_changed = true;
        }

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Python path:");
            if ui
                .text_edit_singleline(&mut self.settings.sidecar.python_path)
                .changed()
            {
                self.settings_changed = true;
            }
        });

        if ui
            .checkbox(&mut self.settings.sidecar.auto_start, "Auto start")
            .changed()
        {
            self.settings_changed = true;
        }
    }

    fn show_keybindings(&self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.colored_label(
            theme.text,
            RichText::new("Keyboard Shortcuts").strong().size(16.0),
        );
        ui.separator();
        ui.add_space(4.0);

        let bindings = default_keybindings();
        egui::Grid::new("keybindings_grid")
            .num_columns(2)
            .spacing([16.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                for binding in &bindings {
                    ui.colored_label(theme.text_secondary, binding.action.label());
                    let label = binding_label(binding);
                    egui::Frame::none()
                        .fill(theme.background)
                        .rounding(egui::Rounding::same(4.0))
                        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                        .show(ui, |ui| {
                            ui.colored_label(
                                theme.accent,
                                RichText::new(&label).monospace().size(11.0),
                            );
                        });
                    ui.end_row();
                }
            });

        ui.add_space(4.0);
        ui.colored_label(
            theme.text_muted,
            RichText::new("Custom keybinding editor coming soon.")
                .size(11.0)
                .italics(),
        );
    }
}
