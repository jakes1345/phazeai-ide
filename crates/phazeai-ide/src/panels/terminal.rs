use egui::{self, Color32, FontId, RichText, TextEdit};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::themes::ThemeColors;

/// A line of terminal output, with basic ANSI-stripped text
struct TerminalLine {
    text: String,
    is_error: bool,
}

/// Real PTY terminal panel
pub struct TerminalPanel {
    /// Current working directory
    cwd: PathBuf,
    /// User input for the terminal
    input: String,
    /// Terminal output lines (shared with reader thread)
    lines: Arc<Mutex<Vec<TerminalLine>>>,
    /// PTY writer (sends input to shell)
    writer: Option<Box<dyn Write + Send>>,
    /// Whether the PTY session is alive
    alive: Arc<Mutex<bool>>,
    /// Auto-scroll flag
    scroll_to_bottom: bool,
}

impl TerminalPanel {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| home_dir());
        let mut panel = Self {
            cwd: cwd.clone(),
            input: String::new(),
            lines: Arc::new(Mutex::new(Vec::new())),
            writer: None,
            alive: Arc::new(Mutex::new(false)),
            scroll_to_bottom: true,
        };
        panel.spawn_shell();
        panel
    }

    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
        // Send a cd command to the existing shell
        self.send_input(&format!("cd {}\n", self.cwd.display()));
    }

    pub fn get_cwd(&self) -> PathBuf {
        self.cwd.clone()
    }

    fn spawn_shell(&mut self) {
        let pty_system = NativePtySystem::default();

        let pair = match pty_system.openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(e) => {
                self.add_line(&format!("Failed to open PTY: {e}"), true);
                return;
            }
        };

        // Determine which shell to use
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(&self.cwd);

        // Spawn the shell process
        let child = match pair.slave.spawn_command(cmd) {
            Ok(child) => child,
            Err(e) => {
                self.add_line(&format!("Failed to spawn shell: {e}"), true);
                return;
            }
        };

        // Get the writer for sending input to the PTY
        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                self.add_line(&format!("Failed to get PTY writer: {e}"), true);
                return;
            }
        };
        self.writer = Some(writer);

        // Get the reader for receiving output from the PTY
        let mut reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                self.add_line(&format!("Failed to clone PTY reader: {e}"), true);
                return;
            }
        };

        *self.alive.lock().unwrap() = true;
        let lines = Arc::clone(&self.lines);
        let alive = Arc::clone(&self.alive);

        // Reader thread: reads PTY output and appends to lines
        thread::spawn(move || {
            let _child = child; // keep child alive
            let mut buf = [0u8; 4096];
            let mut partial_line = String::new();

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        partial_line.push_str(&text);

                        // Split by newlines
                        while let Some(pos) = partial_line.find('\n') {
                            let line = strip_ansi(&partial_line[..pos]);
                            if let Ok(mut lines) = lines.lock() {
                                lines.push(TerminalLine {
                                    text: line,
                                    is_error: false,
                                });
                                // Cap at 5000 lines
                                if lines.len() > 5000 {
                                    lines.drain(0..1000);
                                }
                            }
                            partial_line = partial_line[pos + 1..].to_string();
                        }
                    }
                    Err(_) => break,
                }
            }

            // Flush any remaining partial line
            if !partial_line.is_empty() {
                if let Ok(mut lines) = lines.lock() {
                    lines.push(TerminalLine {
                        text: strip_ansi(&partial_line),
                        is_error: false,
                    });
                }
            }

            *alive.lock().unwrap() = false;
        });
    }

    fn send_input(&mut self, text: &str) {
        if let Some(ref mut writer) = self.writer {
            let _ = writer.write_all(text.as_bytes());
            let _ = writer.flush();
        }
    }

    fn add_line(&self, text: &str, is_error: bool) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(TerminalLine {
                text: text.to_string(),
                is_error,
            });
        }
    }

    pub fn is_running(&self) -> bool {
        *self.alive.lock().unwrap()
    }

    /// Execute a command in the terminal (used by agent integration)
    pub fn execute_command(&mut self, cmd_str: &str) {
        self.send_input(&format!("{}\n", cmd_str));
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        let panel_bg = theme.background_secondary;
        let text_color = theme.text;
        let error_color = Color32::from_rgb(255, 100, 100);
        let input_bg = Color32::from_rgba_premultiplied(0, 0, 0, 200);
        let green = Color32::from_rgb(100, 255, 100);

        egui::Frame::none()
            .fill(panel_bg)
            .inner_margin(6.0)
            .show(ui, |ui: &mut egui::Ui| {
                // Header
                ui.horizontal(|ui: &mut egui::Ui| {
                    ui.colored_label(green, "⬤");
                    ui.label(
                        RichText::new(" Terminal (PTY)")
                            .font(FontId::proportional(13.0))
                            .color(text_color),
                    );

                    // Status indicator
                    if self.is_running() {
                        ui.colored_label(green, "●");
                    } else {
                        ui.colored_label(error_color, "●");
                        if ui.small_button("Restart").clicked() {
                            self.spawn_shell();
                        }
                    }
                });

                ui.separator();

                // Output area
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(self.scroll_to_bottom)
                    .max_height(ui.available_height() - 30.0)
                    .show(ui, |ui| {
                        if let Ok(lines) = self.lines.lock() {
                            for line in lines.iter() {
                                let color = if line.is_error { error_color } else { text_color };
                                ui.label(
                                    RichText::new(&line.text)
                                        .font(FontId::monospace(13.0))
                                        .color(color),
                                );
                            }
                        }
                    });

                // Input line
                ui.horizontal(|ui: &mut egui::Ui| {
                    ui.colored_label(green, "$");
                    let input_resp = ui.add(
                        TextEdit::singleline(&mut self.input)
                            .font(FontId::monospace(13.0))
                            .text_color(text_color)
                            .desired_width(ui.available_width() - 70.0)
                            .hint_text("Enter command...")
                            .frame(true),
                    );

                    // Submit on Enter
                    if input_resp.lost_focus() && ui.input(|i: &egui::InputState| i.key_pressed(egui::Key::Enter)) {
                        let cmd = self.input.clone();
                        if !cmd.is_empty() {
                            self.send_input(&format!("{}\n", cmd));
                            self.input.clear();
                            self.scroll_to_bottom = true;
                        }
                        // Re-focus input
                        input_resp.request_focus();
                    }

                    if ui.small_button("⏎").clicked() {
                        let cmd = self.input.clone();
                        if !cmd.is_empty() {
                            self.send_input(&format!("{}\n", cmd));
                            self.input.clear();
                            self.scroll_to_bottom = true;
                        }
                    }
                });
            });
    }
}

/// Strip ANSI escape sequences from text
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Skip escape sequence
            if let Some(&'[') = chars.peek() {
                chars.next(); // consume '['
                // Read until we hit a letter (the command terminator)
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c.is_ascii_alphabetic() || c == 'm' || c == 'H' || c == 'J' || c == 'K' {
                        break;
                    }
                }
            }
        } else if ch == '\r' {
            // Skip carriage returns
        } else {
            result.push(ch);
        }
    }
    result
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}
