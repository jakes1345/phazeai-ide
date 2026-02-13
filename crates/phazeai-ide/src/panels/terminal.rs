use egui::{self, FontId, RichText, TextEdit};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;

use crate::themes::ThemeColors;

struct TerminalLine {
    text: String,
    is_error: bool,
    is_command: bool,
}

pub struct TerminalPanel {
    lines: Arc<Mutex<Vec<TerminalLine>>>,
    input: String,
    cwd: PathBuf,
    is_running: bool,
    scroll_to_bottom: bool,
}

impl TerminalPanel {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut panel = Self {
            lines: Arc::new(Mutex::new(Vec::new())),
            input: String::new(),
            cwd: cwd.clone(),
            is_running: false,
            scroll_to_bottom: true,
        };
        panel.add_line(&format!("Terminal ready. cwd: {}", cwd.display()), false, false);
        panel
    }

    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    fn add_line(&mut self, text: &str, is_error: bool, is_command: bool) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(TerminalLine {
                text: text.to_string(),
                is_error,
                is_command,
            });
            // Keep last 5000 lines
            if lines.len() > 5000 {
                let drain_count = lines.len() - 5000;
                lines.drain(..drain_count);
            }
        }
        self.scroll_to_bottom = true;
    }

    fn execute_command(&mut self, cmd_str: &str) {
        self.add_line(&format!("$ {}", cmd_str), false, true);

        // Handle cd specially
        let trimmed = cmd_str.trim();
        if trimmed == "cd" || trimmed.starts_with("cd ") {
            let dir = if trimmed == "cd" {
                home_dir()
            } else {
                let path_str = trimmed.strip_prefix("cd ").unwrap().trim();
                if path_str.starts_with('/') || (cfg!(windows) && path_str.len() >= 2 && path_str.as_bytes()[1] == b':') {
                    PathBuf::from(path_str)
                } else if path_str.starts_with('~') {
                    let home = home_dir();
                    home.join(path_str.strip_prefix("~/").unwrap_or(""))
                } else {
                    self.cwd.join(path_str)
                }
            };

            if dir.is_dir() {
                self.cwd = dir.canonicalize().unwrap_or(dir);
                self.add_line(&format!("Changed directory to: {}", self.cwd.display()), false, false);
            } else {
                self.add_line(&format!("cd: no such directory: {}", dir.display()), true, false);
            }
            return;
        }

        if trimmed == "clear" {
            if let Ok(mut lines) = self.lines.lock() {
                lines.clear();
            }
            return;
        }

        if trimmed == "pwd" {
            self.add_line(&self.cwd.display().to_string(), false, false);
            return;
        }

        // Execute via shell
        let (shell, flag) = if cfg!(windows) {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };

        match Command::new(shell)
            .arg(flag)
            .arg(cmd_str)
            .current_dir(&self.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => {
                self.is_running = true;

                // Read stdout
                if let Some(stdout) = child.stdout {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        match line {
                            Ok(text) => self.add_line(&text, false, false),
                            Err(e) => self.add_line(&format!("Read error: {e}"), true, false),
                        }
                    }
                }

                // Read stderr
                if let Some(stderr) = child.stderr {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines() {
                        match line {
                            Ok(text) => self.add_line(&text, true, false),
                            Err(e) => self.add_line(&format!("Read error: {e}"), true, false),
                        }
                    }
                }

                self.is_running = false;
            }
            Err(e) => {
                self.add_line(&format!("Failed to execute: {e}"), true, false);
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "TERMINAL");
            ui.colored_label(theme.text_muted, RichText::new(&format!(" {}", self.cwd.display())).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").clicked() {
                    if let Ok(mut lines) = self.lines.lock() {
                        lines.clear();
                    }
                }
            });
        });
        ui.separator();

        // Output area
        let available_height = ui.available_height() - 32.0;
        egui::ScrollArea::vertical()
            .max_height(available_height)
            .auto_shrink([false, false])
            .stick_to_bottom(self.scroll_to_bottom)
            .show(ui, |ui| {
                if let Ok(lines) = self.lines.lock() {
                    for line in lines.iter() {
                        let color = if line.is_command {
                            theme.accent
                        } else if line.is_error {
                            theme.error
                        } else {
                            theme.text
                        };
                        ui.colored_label(color, RichText::new(&line.text).monospace().size(12.0));
                    }
                }
                self.scroll_to_bottom = false;
            });

        // Input line
        ui.horizontal(|ui| {
            ui.colored_label(theme.accent, "$");
            let response = ui.add(
                TextEdit::singleline(&mut self.input)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(12.0))
                    .hint_text("Enter command..."),
            );

            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if !self.input.trim().is_empty() {
                    let cmd = self.input.clone();
                    self.input.clear();
                    self.execute_command(&cmd);
                }
                response.request_focus();
            }
        });
    }
}

fn home_dir() -> PathBuf {
    // Cross-platform home directory without external dep
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}
