use crate::themes::ThemeColors;
use egui::{Color32, RichText, ScrollArea};
use lsp_types::{Diagnostic, DiagnosticSeverity};
use std::collections::HashMap;
use std::path::PathBuf;

pub struct ProblemsPanel {
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
}

impl Default for ProblemsPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ProblemsPanel {
    pub fn new() -> Self {
        Self {
            diagnostics: HashMap::new(),
        }
    }

    pub fn set_diagnostics(&mut self, path: PathBuf, diags: Vec<Diagnostic>) {
        if diags.is_empty() {
            self.diagnostics.remove(&path);
        } else {
            self.diagnostics.insert(path, diags);
        }
    }

    pub fn clear(&mut self) {
        self.diagnostics.clear();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) -> Option<(PathBuf, usize)> {
        let mut goto_location = None;

        let bg = theme.background_secondary;
        let mut total_errors = 0;
        let mut total_warnings = 0;

        for diags in self.diagnostics.values() {
            for d in diags {
                if d.severity == Some(DiagnosticSeverity::ERROR) {
                    total_errors += 1;
                } else if d.severity == Some(DiagnosticSeverity::WARNING) {
                    total_warnings += 1;
                }
            }
        }

        egui::Frame::none()
            .fill(bg)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Problems")
                            .color(theme.text_secondary)
                            .size(12.0),
                    );
                    ui.separator();
                    let err_color = Color32::from_rgb(255, 100, 100);
                    let warn_color = Color32::from_rgb(255, 200, 100);
                    ui.colored_label(err_color, format!("✖ {}", total_errors));
                    ui.add_space(4.0);
                    ui.colored_label(warn_color, format!("⚠ {}", total_warnings));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Clear").clicked() {
                            self.clear();
                        }
                    });
                });

                ui.separator();

                if self.diagnostics.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.label(
                            RichText::new("No problems found in workspace.")
                                .color(theme.text_muted),
                        );
                    });
                    return;
                }

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut paths: Vec<_> = self.diagnostics.keys().collect();
                        paths.sort();

                        for path in paths {
                            let diags = &self.diagnostics[path];
                            let file_name = path
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "Unknown".to_string());

                            let current_dir = std::env::current_dir().unwrap_or_default();
                            let relative_path = path.strip_prefix(&current_dir).unwrap_or(path);

                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&file_name).color(theme.text).size(13.0));
                                ui.label(
                                    RichText::new(relative_path.to_string_lossy())
                                        .color(theme.text_muted)
                                        .size(11.0),
                                );
                            });

                            for diag in diags {
                                let (icon, color) = match diag.severity {
                                    Some(DiagnosticSeverity::ERROR) => {
                                        ("✖", Color32::from_rgb(255, 100, 100))
                                    }
                                    Some(DiagnosticSeverity::WARNING) => {
                                        ("⚠", Color32::from_rgb(255, 200, 100))
                                    }
                                    Some(DiagnosticSeverity::INFORMATION) => {
                                        ("ℹ", Color32::from_rgb(100, 200, 255))
                                    }
                                    Some(DiagnosticSeverity::HINT) => {
                                        ("💡", Color32::from_rgb(200, 200, 200))
                                    }
                                    _ => ("•", theme.text_secondary),
                                };

                                let line = diag.range.start.line + 1;
                                let col = diag.range.start.character + 1;

                                ui.horizontal(|ui| {
                                    ui.add_space(16.0);
                                    ui.colored_label(color, icon);

                                    let msg = diag.message.lines().next().unwrap_or_default();

                                    if ui.link(format!("{} [{}, {}]", msg, line, col)).clicked() {
                                        goto_location =
                                            Some((path.clone(), diag.range.start.line as usize));
                                    }
                                });
                            }
                            ui.add_space(4.0);
                        }
                    });
            });

        goto_location
    }
}
