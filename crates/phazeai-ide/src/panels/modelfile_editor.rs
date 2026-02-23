use crate::app::IdeEvent;
use crate::themes::ThemeColors;
use egui::{vec2, Align2, FontId, RichText};
use tokio::sync::mpsc;

pub struct ModelfileEditorPanel {
    pub is_open: bool,
    pub model_name: String,
    pub base_model: String,
    pub system_prompt: String,

    // Parameters
    pub temperature: f32,
    pub top_p: f32,
    pub num_ctx: u32,
    pub repeat_penalty: f32,

    pub is_building: bool,
    pub build_status: Option<(String, bool)>, // (Message, IsSuccess)
}

impl Default for ModelfileEditorPanel {
    fn default() -> Self {
        Self {
            is_open: false,
            model_name: "custom-agent".to_string(),
            base_model: "llama3.2:3b".to_string(),
            system_prompt: "You are a helpful AI coding assistant.".to_string(),
            temperature: 0.7,
            top_p: 0.9,
            num_ctx: 8192,
            repeat_penalty: 1.1,
            is_building: false,
            build_status: None,
        }
    }
}

impl ModelfileEditorPanel {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        theme: &ThemeColors,
        ide_tx: &mpsc::UnboundedSender<IdeEvent>,
    ) {
        if !self.is_open {
            return;
        }

        let mut is_open = self.is_open;

        egui::Window::new(RichText::new("Modelfile GUI Editor").strong())
            .open(&mut is_open)
            .collapsible(false)
            .resizable(true)
            .default_size(vec2(600.0, 700.0))
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(theme.panel)
                    .stroke(egui::Stroke::new(1.0, theme.border)),
            )
            .show(ctx, |ui| {
                ui.visuals_mut().override_text_color = Some(theme.text);

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Model Name:").strong());
                    ui.add_sized(
                        [ui.available_width(), 24.0],
                        egui::TextEdit::singleline(&mut self.model_name),
                    );
                });

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Base Model:").strong());
                    ui.add_sized(
                        [ui.available_width(), 24.0],
                        egui::TextEdit::singleline(&mut self.base_model),
                    );
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                ui.label(RichText::new("System Prompt:").strong());
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), 200.0],
                            egui::TextEdit::multiline(&mut self.system_prompt)
                                .font(FontId::monospace(14.0))
                                .desired_rows(10),
                        );
                    });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                ui.label(RichText::new("Hyperparameters").strong());
                ui.add_space(8.0);

                egui::Grid::new("hyperparams_grid")
                    .num_columns(2)
                    .spacing([40.0, 12.0])
                    .show(ui, |ui| {
                        ui.label("Temperature");
                        ui.add(egui::Slider::new(&mut self.temperature, 0.0..=2.0).text(""));
                        ui.end_row();

                        ui.label("Top P");
                        ui.add(egui::Slider::new(&mut self.top_p, 0.0..=1.0).text(""));
                        ui.end_row();

                        ui.label("Context Window (num_ctx)");
                        ui.add(
                            egui::Slider::new(&mut self.num_ctx, 1024..=128000)
                                .logarithmic(true)
                                .text("tokens"),
                        );
                        ui.end_row();

                        ui.label("Repeat Penalty");
                        ui.add(egui::Slider::new(&mut self.repeat_penalty, 0.0..=2.0).text(""));
                        ui.end_row();
                    });

                ui.add_space(20.0);
                ui.separator();
                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    if self.is_building {
                        ui.spinner();
                        ui.label(RichText::new("Building model in Ollama...").color(theme.accent));
                    } else {
                        let btn = ui.button(RichText::new("Build & Register Model").strong());
                        if btn.clicked() {
                            self.trigger_build(ide_tx);
                        }
                    }

                    if let Some((msg, success)) = &self.build_status {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let color = if *success { theme.success } else { theme.error };
                            ui.label(RichText::new(msg).color(color));
                        });
                    }
                });

                ui.add_space(8.0);
            });

        self.is_open = is_open;
    }

    fn trigger_build(&mut self, ide_tx: &mpsc::UnboundedSender<IdeEvent>) {
        if self.model_name.is_empty() || self.base_model.is_empty() {
            self.build_status = Some(("Model name and base model are required".to_string(), false));
            return;
        }

        self.is_building = true;
        self.build_status = None;

        let modelfile = format!(
            "FROM {}\nSYSTEM \"\"\"{}\"\"\"\nPARAMETER temperature {}\nPARAMETER top_p {}\nPARAMETER num_ctx {}\nPARAMETER repeat_penalty {}\n",
            self.base_model,
            self.system_prompt,
            self.temperature,
            self.top_p,
            self.num_ctx,
            self.repeat_penalty
        );

        let _ = ide_tx.send(IdeEvent::BuildOllamaModel {
            name: self.model_name.clone(),
            modelfile_content: modelfile,
        });
    }
}
