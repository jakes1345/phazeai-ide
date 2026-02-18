use egui::{self, RichText};
use phazeai_core::config::{LlmProvider, Settings};

use crate::themes::{ThemeColors, ThemePreset};

pub struct SettingsPanel {
    pub visible: bool,
    pub settings: Settings,
    pub theme_preset: ThemePreset,
    pub settings_changed: bool,
    provider_idx: usize,
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
            .default_size([500.0, 600.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.show_appearance(ui, theme);
                    ui.add_space(16.0);
                    self.show_llm(ui, theme);
                    ui.add_space(16.0);
                    self.show_editor(ui, theme);
                    ui.add_space(16.0);
                    self.show_sidecar(ui, theme);
                    ui.add_space(16.0);

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
                        if ui.selectable_value(&mut self.theme_preset, preset, &name).changed() {
                            self.settings.editor.theme = name;
                            self.settings_changed = true;
                        }
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Font size:");
            if ui
                .add(egui::Slider::new(&mut self.settings.editor.font_size, 10.0..=24.0))
                .changed()
            {
                self.settings_changed = true;
            }
        });
    }

    fn show_llm(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.colored_label(theme.text, RichText::new("LLM Provider").strong().size(16.0));
        ui.separator();

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "Provider:");
            let providers = ["Claude", "OpenAI", "Ollama", "Groq", "Together.ai", "OpenRouter", "LM Studio"];
            egui::ComboBox::from_id_source("provider_selector")
                .selected_text(providers[self.provider_idx])
                .show_ui(ui, |ui: &mut egui::Ui| {
                    for (i, name) in providers.iter().enumerate() {
                        if ui.selectable_value(&mut self.provider_idx, i, *name).changed() {
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
            if ui.text_edit_singleline(&mut self.settings.llm.model).changed() {
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
            .checkbox(&mut self.settings.editor.show_line_numbers, "Show line numbers")
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
        ui.colored_label(theme.text, RichText::new("Python Sidecar").strong().size(16.0));
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
}
