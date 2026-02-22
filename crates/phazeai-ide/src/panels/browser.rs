use crate::themes::ThemeColors;
use eframe::egui;
use egui::{Margin, RichText, Rounding, Stroke};

const QUICK_LINKS: &[(&str, &str)] = &[
    ("std", "https://doc.rust-lang.org/std/"),
    ("crates.io", "https://crates.io"),
    ("docs.rs", "https://docs.rs"),
    ("Rust Book", "https://doc.rust-lang.org/book/"),
    ("egui", "https://docs.rs/egui/latest/egui/"),
    ("tokio", "https://docs.rs/tokio/latest/tokio/"),
    ("MDN", "https://developer.mozilla.org"),
];

pub struct BrowserPanel {
    pub url: String,
    pub title: String,
    pub content: String,
    pub history: Vec<String>,
    pub history_index: usize,
    pub loading: bool,
    pub show_history: bool,
}

impl Default for BrowserPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserPanel {
    pub fn new() -> Self {
        Self {
            url: String::new(),
            title: "Documentation".to_string(),
            content: String::new(),
            history: Vec::new(),
            history_index: 0,
            loading: false,
            show_history: false,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.vertical(|ui| {
            // Quick links bar
            self.render_quick_links(ui, theme);
            ui.add_space(4.0);

            // URL / navigation bar
            self.render_navbar(ui, theme);

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // Content area
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.loading {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.spinner();
                            ui.add_space(8.0);
                            ui.colored_label(theme.text_muted, "Loading...");
                        });
                    } else if self.content.is_empty() {
                        self.render_landing(ui, theme);
                    } else {
                        self.render_content(ui, theme);
                    }
                });
        });
    }

    fn render_quick_links(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.horizontal(|ui| {
            ui.colored_label(theme.text_muted, RichText::new("Quick:").size(11.0));
            ui.add_space(4.0);
            for (label, url) in QUICK_LINKS {
                let btn = egui::Button::new(RichText::new(*label).size(11.0).color(theme.accent))
                    .fill(theme.background_secondary)
                    .frame(true);
                if ui.add(btn).clicked() {
                    self.navigate_to(url.to_string());
                }
            }
        });
    }

    fn render_navbar(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.horizontal(|ui| {
            let back_enabled = self.history_index > 0;
            if ui
                .add_enabled(back_enabled, egui::Button::new("⬅").frame(false))
                .clicked()
            {
                self.go_back();
            }
            let forward_enabled = self.history_index + 1 < self.history.len();
            if ui
                .add_enabled(forward_enabled, egui::Button::new("➡").frame(false))
                .clicked()
            {
                self.go_forward();
            }
            if ui.button("🔄").clicked() && !self.url.is_empty() {
                self.loading = true;
            }
            ui.add_space(8.0);

            let url_stroke = if self.loading {
                Stroke::new(1.0, theme.accent)
            } else {
                Stroke::new(1.0, theme.border)
            };

            egui::Frame::none()
                .fill(theme.background_secondary)
                .rounding(Rounding::same(8.0))
                .stroke(url_stroke)
                .inner_margin(Margin::symmetric(12.0, 6.0))
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let text_edit = egui::TextEdit::singleline(&mut self.url)
                            .frame(false)
                            .hint_text("Search docs or enter URL (e.g. docs.rs/serde)...")
                            .text_color(theme.text)
                            .desired_width(ui.available_width() - 40.0);
                        let response = ui.add(text_edit);
                        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.navigate_to(self.url.clone());
                        }
                        if self.loading {
                            ui.spinner();
                        }
                    });
                });
        });
    }

    fn render_landing(&self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("📖 Documentation Viewer")
                    .size(22.0)
                    .color(theme.accent)
                    .strong(),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new("Use the quick links above or enter a URL to browse docs.")
                    .color(theme.text_muted),
            );
            ui.add_space(24.0);

            egui::Grid::new("landing_grid")
                .num_columns(2)
                .spacing([24.0, 8.0])
                .show(ui, |ui| {
                    for (label, url) in QUICK_LINKS {
                        ui.colored_label(theme.accent, "→");
                        ui.vertical(|ui| {
                            ui.colored_label(theme.text, RichText::new(*label).strong());
                            ui.colored_label(theme.text_muted, RichText::new(*url).size(10.0));
                        });
                        ui.end_row();
                    }
                });
        });
    }

    fn render_content(&self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.vertical(|ui| {
            if !self.title.is_empty() && self.title != "Documentation" {
                ui.label(
                    RichText::new(&self.title)
                        .heading()
                        .color(theme.accent)
                        .strong(),
                );
                ui.add_space(12.0);
            }

            let mut in_code_block = false;
            for line in self.content.lines() {
                if line.starts_with("```") {
                    in_code_block = !in_code_block;
                    continue;
                }
                if in_code_block {
                    egui::Frame::none()
                        .fill(theme.background)
                        .inner_margin(Margin::symmetric(8.0, 2.0))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(line)
                                    .monospace()
                                    .size(12.0)
                                    .color(theme.text_secondary),
                            );
                        });
                } else if let Some(rest) = line.strip_prefix("### ") {
                    ui.label(RichText::new(rest).size(15.0).color(theme.text).strong());
                } else if let Some(rest) = line.strip_prefix("## ") {
                    ui.label(RichText::new(rest).size(18.0).color(theme.text).strong());
                } else if let Some(rest) = line.strip_prefix("# ") {
                    ui.label(RichText::new(rest).heading().color(theme.text).strong());
                } else if line.is_empty() {
                    ui.add_space(8.0);
                } else {
                    ui.label(
                        RichText::new(line)
                            .color(theme.text_muted)
                            .line_height(Some(20.0)),
                    );
                }
            }
        });
    }

    pub fn navigate_to(&mut self, url: String) {
        self.url = url.clone();
        if self.history_index + 1 < self.history.len() {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(url);
        self.history_index = self.history.len() - 1;
        self.loading = true;
    }

    pub fn go_back(&mut self) {
        if self.history_index > 0 {
            self.history_index -= 1;
            self.url = self.history[self.history_index].clone();
            self.loading = true;
        }
    }

    pub fn go_forward(&mut self) {
        if self.history_index + 1 < self.history.len() {
            self.history_index += 1;
            self.url = self.history[self.history_index].clone();
            self.loading = true;
        }
    }

    pub fn set_content(&mut self, url: String, title: String, content: String) {
        if url == self.url {
            self.title = title;
            self.content = content;
            self.loading = false;
        }
    }

    pub fn set_error(&mut self, _url: String, error: String) {
        self.title = "Error".to_string();
        self.content = format!("## Could not load page\n\n{}", error);
        self.loading = false;
    }
}
