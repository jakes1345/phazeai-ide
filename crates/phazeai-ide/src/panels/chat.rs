use egui::{self, Color32, FontId, RichText, TextEdit, Rounding, Margin};
use chrono::Local;

use crate::themes::ThemeColors;

#[derive(Clone, Debug)]
pub enum ChatMessageRole {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatMessageRole,
    pub content: String,
    pub timestamp: String,
    pub tool_calls: Vec<ToolCallInfo>,
}

#[derive(Clone, Debug)]
pub struct ToolCallInfo {
    pub name: String,
    pub success: bool,
    pub summary: String,
}

pub struct ChatPanel {
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub is_streaming: bool,
    pub current_streaming_text: String,
    pub pending_send: Option<String>,
    scroll_to_bottom: bool,
}

impl ChatPanel {
    pub fn new() -> Self {
        Self {
            messages: vec![ChatMessage {
                role: ChatMessageRole::System,
                content: "Welcome to PhazeAI. Type a message to start.".to_string(),
                timestamp: Local::now().format("%H:%M").to_string(),
                tool_calls: Vec::new(),
            }],
            input: String::new(),
            is_streaming: false,
            current_streaming_text: String::new(),
            pending_send: None,
            scroll_to_bottom: true,
        }
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(ChatMessage {
            role: ChatMessageRole::User,
            content: content.to_string(),
            timestamp: Local::now().format("%H:%M").to_string(),
            tool_calls: Vec::new(),
        });
        self.scroll_to_bottom = true;
    }

    pub fn add_assistant_message(&mut self, content: &str) {
        self.messages.push(ChatMessage {
            role: ChatMessageRole::Assistant,
            content: content.to_string(),
            timestamp: Local::now().format("%H:%M").to_string(),
            tool_calls: Vec::new(),
        });
        self.scroll_to_bottom = true;
    }

    pub fn add_tool_call(&mut self, name: &str, success: bool, summary: &str) {
        if let Some(last) = self.messages.last_mut() {
            last.tool_calls.push(ToolCallInfo {
                name: name.to_string(),
                success,
                summary: summary.to_string(),
            });
        }
        self.scroll_to_bottom = true;
    }

    pub fn start_streaming(&mut self) {
        self.is_streaming = true;
        self.current_streaming_text.clear();
    }

    pub fn append_streaming_text(&mut self, text: &str) {
        self.current_streaming_text.push_str(text);
        self.scroll_to_bottom = true;
    }

    pub fn finish_streaming(&mut self) {
        if !self.current_streaming_text.is_empty() {
            self.add_assistant_message(&self.current_streaming_text.clone());
            self.current_streaming_text.clear();
        }
        self.is_streaming = false;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "AI CHAT");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").clicked() {
                    self.messages.clear();
                    self.messages.push(ChatMessage {
                        role: ChatMessageRole::System,
                        content: "Chat cleared.".to_string(),
                        timestamp: Local::now().format("%H:%M").to_string(),
                        tool_calls: Vec::new(),
                    });
                }
            });
        });
        ui.separator();

        // Messages area
        let available_height = ui.available_height() - 80.0;
        egui::ScrollArea::vertical()
            .max_height(available_height)
            .auto_shrink([false, false])
            .stick_to_bottom(self.scroll_to_bottom)
            .show(ui, |ui| {
                for msg in &self.messages {
                    self.render_message(ui, msg, theme);
                    ui.add_space(8.0);
                }

                // Show streaming text
                if self.is_streaming && !self.current_streaming_text.is_empty() {
                    render_assistant_bubble(ui, &self.current_streaming_text, "", theme);
                    ui.add_space(8.0);
                }

                // Show typing indicator
                if self.is_streaming && self.current_streaming_text.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.colored_label(theme.text_muted, "Thinking...");
                    });
                }

                self.scroll_to_bottom = false;
            });

        ui.separator();

        // Input area
        ui.horizontal(|ui| {
            let text_edit = TextEdit::multiline(&mut self.input)
                .desired_width(ui.available_width() - 60.0)
                .desired_rows(2)
                .font(FontId::monospace(13.0))
                .hint_text("Ask PhazeAI...");

            let response = ui.add(text_edit);

            let send_clicked = ui.add_enabled(
                !self.is_streaming && !self.input.trim().is_empty(),
                egui::Button::new("Send"),
            ).clicked();

            let enter_pressed = response.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

            if (send_clicked || enter_pressed) && !self.input.trim().is_empty() && !self.is_streaming {
                let msg = self.input.trim().to_string();
                self.add_user_message(&msg);
                self.pending_send = Some(msg);
                self.input.clear();
            }
        });
    }

    fn render_message(&self, ui: &mut egui::Ui, msg: &ChatMessage, theme: &ThemeColors) {
        match msg.role {
            ChatMessageRole::User => {
                render_user_bubble(ui, &msg.content, &msg.timestamp, theme);
            }
            ChatMessageRole::Assistant => {
                render_assistant_bubble(ui, &msg.content, &msg.timestamp, theme);
                for tc in &msg.tool_calls {
                    render_tool_call(ui, tc, theme);
                }
            }
            ChatMessageRole::System => {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.colored_label(theme.text_muted, &msg.content);
                });
            }
        }
    }
}

fn make_frame(fill: Color32, rounding_val: f32, margin_val: f32) -> egui::Frame {
    egui::Frame {
        fill,
        rounding: Rounding::same(rounding_val),
        inner_margin: Margin::same(margin_val),
        ..Default::default()
    }
}

fn render_user_bubble(ui: &mut egui::Ui, content: &str, timestamp: &str, theme: &ThemeColors) {
    make_frame(theme.accent, 8.0, 10.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::WHITE, "You");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(Color32::from_white_alpha(150), RichText::new(timestamp).small());
                });
            });
            ui.colored_label(Color32::WHITE, content);
        });
}

fn render_assistant_bubble(ui: &mut egui::Ui, content: &str, timestamp: &str, theme: &ThemeColors) {
    make_frame(theme.surface, 8.0, 10.0)
        .show(ui, |ui| {
            if !timestamp.is_empty() {
                ui.horizontal(|ui| {
                    ui.colored_label(theme.accent, "PhazeAI");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.colored_label(theme.text_muted, RichText::new(timestamp).small());
                    });
                });
            } else {
                ui.colored_label(theme.accent, "PhazeAI");
            }

            // Render content with basic code block support
            let mut in_code_block = false;
            let mut code_buffer = String::new();

            for line in content.lines() {
                if line.starts_with("```") {
                    if in_code_block {
                        // End code block
                        make_frame(theme.background, 4.0, 6.0)
                            .show(ui, |ui| {
                                ui.colored_label(theme.text, RichText::new(&code_buffer).monospace());
                            });
                        code_buffer.clear();
                        in_code_block = false;
                    } else {
                        in_code_block = true;
                    }
                } else if in_code_block {
                    if !code_buffer.is_empty() {
                        code_buffer.push('\n');
                    }
                    code_buffer.push_str(line);
                } else {
                    ui.colored_label(theme.text, line);
                }
            }

            // Handle unclosed code block
            if in_code_block && !code_buffer.is_empty() {
                make_frame(theme.background, 4.0, 6.0)
                    .show(ui, |ui| {
                        ui.colored_label(theme.text, RichText::new(&code_buffer).monospace());
                    });
            }
        });
}

fn render_tool_call(ui: &mut egui::Ui, tc: &ToolCallInfo, theme: &ThemeColors) {
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let icon_color = if tc.success { theme.success } else { theme.error };
        let icon = if tc.success { "+" } else { "x" };
        ui.colored_label(icon_color, icon);
        ui.colored_label(theme.text_secondary, &tc.name);
        if !tc.summary.is_empty() {
            ui.colored_label(theme.text_muted, RichText::new(&tc.summary).small());
        }
    });
}
