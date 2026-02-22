use chrono::Local;
use egui::{self, Color32, FontId, Margin, RichText, Rounding, TextEdit};

use crate::themes::ThemeColors;

// ── AI Mode ───────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub enum AiMode {
    Chat,
    Ask,
    Debug,
    Plan,
    Edit,
}

impl AiMode {
    pub fn label(&self) -> &'static str {
        match self {
            AiMode::Chat => "Chat",
            AiMode::Ask => "Ask",
            AiMode::Debug => "Debug",
            AiMode::Plan => "Plan",
            AiMode::Edit => "Edit",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            AiMode::Chat => "General conversation",
            AiMode::Ask => "Ask about current file",
            AiMode::Debug => "Debug with file + terminal",
            AiMode::Plan => "Plan with project context",
            AiMode::Edit => "Edit current file(s)",
        }
    }

    pub fn system_prompt(&self) -> &'static str {
        match self {
            AiMode::Chat => {
                "You are PhazeAI, an expert AI coding assistant. Help the user with their code, \
                 explain concepts, and answer questions clearly and concisely."
            }
            AiMode::Ask => {
                "You are PhazeAI in Ask mode. The user will share code from their editor. \
                 Answer questions about the code precisely and explain your reasoning. \
                 Focus on the specific code provided, not general advice."
            }
            AiMode::Debug => {
                "You are PhazeAI in Debug mode. You have access to the user's current file \
                 and recent terminal output. Analyze the error or unexpected behavior, identify \
                 the root cause, and provide a concrete fix with explanation."
            }
            AiMode::Plan => {
                "You are PhazeAI in Plan mode. You have access to the project structure and \
                 open files. Create a clear, actionable implementation plan with numbered steps. \
                 Consider dependencies, edge cases, and architectural implications."
            }
            AiMode::Edit => {
                "You are PhazeAI in Edit mode. Use your tools to directly edit the user's files \
                 as instructed. Make precise, minimal changes. Show what you're changing and why. \
                 Prefer editing existing code over rewriting from scratch."
            }
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            AiMode::Chat => "💬",
            AiMode::Ask => "❓",
            AiMode::Debug => "🐛",
            AiMode::Plan => "📋",
            AiMode::Edit => "✏",
        }
    }

    pub fn all() -> &'static [AiMode] {
        &[
            AiMode::Chat,
            AiMode::Ask,
            AiMode::Debug,
            AiMode::Plan,
            AiMode::Edit,
        ]
    }
}

// ── Chat Types ────────────────────────────────────────────────────────────

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
    pub mode: Option<AiMode>,
}

#[derive(Clone, Debug)]
pub struct ToolCallInfo {
    pub name: String,
    pub success: bool,
    pub summary: String,
}

// ── Chat Panel ────────────────────────────────────────────────────────────

pub struct ChatPanel {
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub is_streaming: bool,
    pub current_streaming_text: String,
    /// (raw user message, mode at time of send)
    pub pending_send: Option<(String, AiMode)>,
    pub mode: AiMode,
    /// Set to true when user clicks Stop — consumed by app.rs
    pub pending_cancel: bool,
    /// Whether to include current file in context (even in Chat mode)
    pub include_current_file: bool,
    /// Code block to apply to editor — consumed by app.rs
    pub pending_apply: Option<String>,
    /// Toggle agent history popup — consumed by app.rs
    pub pending_show_history: bool,
    scroll_to_bottom: bool,
}

impl Default for ChatPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatPanel {
    pub fn new() -> Self {
        Self {
            messages: vec![ChatMessage {
                role: ChatMessageRole::System,
                content: "Welcome to PhazeAI. Select a mode and start chatting.".to_string(),
                timestamp: Local::now().format("%H:%M").to_string(),
                tool_calls: Vec::new(),
                mode: None,
            }],
            input: String::new(),
            is_streaming: false,
            current_streaming_text: String::new(),
            pending_send: None,
            mode: AiMode::Chat,
            pending_cancel: false,
            include_current_file: false,
            pending_apply: None,
            pending_show_history: false,
            scroll_to_bottom: true,
        }
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(ChatMessage {
            role: ChatMessageRole::User,
            content: content.to_string(),
            timestamp: Local::now().format("%H:%M").to_string(),
            tool_calls: Vec::new(),
            mode: Some(self.mode.clone()),
        });
        self.scroll_to_bottom = true;
    }

    pub fn add_assistant_message(&mut self, content: &str) {
        self.messages.push(ChatMessage {
            role: ChatMessageRole::Assistant,
            content: content.to_string(),
            timestamp: Local::now().format("%H:%M").to_string(),
            tool_calls: Vec::new(),
            mode: None,
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
        // ── Header ──────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "PHAZEAI");
            ui.add_space(4.0);
            // Mode badge
            let mode_color = mode_color(&self.mode, theme);
            ui.colored_label(
                mode_color,
                RichText::new(format!("{} {}", self.mode.icon(), self.mode.label()))
                    .size(11.0)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").clicked() {
                    self.messages.clear();
                    self.messages.push(ChatMessage {
                        role: ChatMessageRole::System,
                        content: "Chat cleared.".to_string(),
                        timestamp: Local::now().format("%H:%M").to_string(),
                        tool_calls: Vec::new(),
                        mode: None,
                    });
                }
                if ui.small_button("🕐 History").clicked() {
                    self.pending_show_history = true;
                }
            });
        });

        // ── Mode switcher bar ────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            for mode in AiMode::all() {
                let selected = self.mode == *mode;
                let color = if selected {
                    mode_color(mode, theme)
                } else {
                    theme.text_muted
                };
                let label = RichText::new(format!("{} {}", mode.icon(), mode.label()))
                    .size(10.5)
                    .color(color);
                let resp = ui.selectable_label(selected, label);
                if resp.clicked() {
                    self.mode = mode.clone();
                }
                if resp.hovered() {
                    resp.on_hover_text(mode.description());
                }
                ui.add_space(2.0);
            }
        });

        ui.separator();

        // ── Messages area ────────────────────────────────────────────────
        let available_height = ui.available_height() - 90.0;
        egui::ScrollArea::vertical()
            .max_height(available_height)
            .auto_shrink([false, false])
            .stick_to_bottom(self.scroll_to_bottom)
            .show(ui, |ui| {
                let messages = self.messages.clone();
                for msg in &messages {
                    self.render_message(ui, msg, theme);
                    ui.add_space(6.0);
                }

                if self.is_streaming && !self.current_streaming_text.is_empty() {
                    render_assistant_bubble(
                        ui,
                        &self.current_streaming_text,
                        "",
                        theme,
                        &mut self.pending_apply,
                    );
                    ui.add_space(6.0);
                }

                if self.is_streaming && self.current_streaming_text.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.spinner();
                        ui.colored_label(theme.text_muted, "Thinking...");
                    });
                }

                self.scroll_to_bottom = false;
            });

        ui.separator();

        // ── Input area ───────────────────────────────────────────────────
        ui.horizontal(|ui| {
            let hint = match self.mode {
                AiMode::Chat => "Ask PhazeAI anything...",
                AiMode::Ask => "Ask about the current file...",
                AiMode::Debug => "Describe the bug or error...",
                AiMode::Plan => "What do you want to build?",
                AiMode::Edit => "Describe the changes to make...",
            };

            let text_edit = TextEdit::multiline(&mut self.input)
                .desired_width(ui.available_width() - 70.0)
                .desired_rows(2)
                .font(FontId::monospace(13.0))
                .hint_text(hint);

            let response = ui.add(text_edit);

            if self.is_streaming {
                let stop_btn = egui::Button::new(RichText::new("Stop").color(Color32::WHITE))
                    .fill(Color32::from_rgb(180, 60, 60));
                if ui.add(stop_btn).clicked() {
                    self.pending_cancel = true;
                }
            } else {
                let send_clicked = ui
                    .add_enabled(!self.input.trim().is_empty(), egui::Button::new("Send"))
                    .clicked();

                let enter_pressed = response.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);

                if (send_clicked || enter_pressed) && !self.input.trim().is_empty() {
                    let msg = self.input.trim().to_string();
                    self.add_user_message(&msg);
                    self.pending_send = Some((msg, self.mode.clone()));
                    self.input.clear();
                }
            }
        });

        // ── Options bar ───────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let file_color = if self.include_current_file {
                theme.accent
            } else {
                theme.text_muted
            };
            let file_label = RichText::new("📎 file").color(file_color).size(10.0);
            if ui
                .button(file_label)
                .on_hover_text("Include current file as context")
                .clicked()
            {
                self.include_current_file = !self.include_current_file;
            }
            if self.include_current_file {
                ui.colored_label(theme.success, RichText::new("on").size(9.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(
                    theme.text_muted,
                    RichText::new(self.mode.description()).size(10.0),
                );
            });
        });
    }

    fn render_message(&mut self, ui: &mut egui::Ui, msg: &ChatMessage, theme: &ThemeColors) {
        match msg.role {
            ChatMessageRole::User => {
                render_user_bubble(ui, msg, theme);
            }
            ChatMessageRole::Assistant => {
                render_assistant_bubble(
                    ui,
                    &msg.content,
                    &msg.timestamp,
                    theme,
                    &mut self.pending_apply,
                );
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

// ── Colors ────────────────────────────────────────────────────────────────

fn mode_color(mode: &AiMode, theme: &ThemeColors) -> Color32 {
    match mode {
        AiMode::Chat => theme.accent,
        AiMode::Ask => theme.text_secondary,
        AiMode::Debug => theme.error,
        AiMode::Plan => theme.warning,
        AiMode::Edit => theme.success,
    }
}

// ── Bubble Renderers ──────────────────────────────────────────────────────

fn make_frame(fill: Color32, rounding_val: f32, margin_val: f32) -> egui::Frame {
    egui::Frame {
        fill,
        rounding: Rounding::same(rounding_val),
        inner_margin: Margin::same(margin_val),
        ..Default::default()
    }
}

fn render_user_bubble(ui: &mut egui::Ui, msg: &ChatMessage, theme: &ThemeColors) {
    let frame = make_frame(
        Color32::from(egui::Rgba::from(theme.accent).multiply(0.8)),
        12.0,
        12.0,
    );
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("YOU")
                    .strong()
                    .color(Color32::WHITE)
                    .size(10.0),
            );
            // Show mode badge
            if let Some(ref mode) = msg.mode {
                ui.add_space(6.0);
                ui.colored_label(
                    Color32::from_white_alpha(160),
                    RichText::new(format!("{} {}", mode.icon(), mode.label())).size(9.0),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(&msg.timestamp)
                        .color(Color32::from_white_alpha(140))
                        .small(),
                );
            });
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(&msg.content)
                .color(Color32::WHITE)
                .line_height(Some(18.0)),
        );
    });
}

fn render_assistant_bubble(
    ui: &mut egui::Ui,
    content: &str,
    timestamp: &str,
    theme: &ThemeColors,
    apply_out: &mut Option<String>,
) {
    let frame = make_frame(theme.surface, 12.0, 12.0);
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("PHAZEAI")
                    .strong()
                    .color(theme.accent)
                    .size(10.0),
            );
            if !timestamp.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(timestamp).color(theme.text_muted).small());
                });
            }
        });
        ui.add_space(4.0);

        let mut in_code_block = false;
        let mut code_lang = String::new();
        let mut code_buffer = String::new();

        for line in content.lines() {
            if line.starts_with("```") {
                if in_code_block {
                    let captured_code = code_buffer.clone();
                    let header_color = if code_lang.is_empty() {
                        theme.text_muted
                    } else {
                        theme.accent
                    };
                    make_frame(theme.background, 4.0, 8.0).show(ui, |ui| {
                        // Code block header with lang + apply button
                        ui.horizontal(|ui| {
                            if !code_lang.is_empty() {
                                ui.colored_label(
                                    header_color,
                                    RichText::new(&code_lang).size(10.0),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let apply_btn = egui::Button::new(
                                        RichText::new("Apply to editor")
                                            .size(9.0)
                                            .color(theme.accent),
                                    )
                                    .frame(false);
                                    if ui.add(apply_btn).clicked() {
                                        *apply_out = Some(captured_code.clone());
                                    }
                                },
                            );
                        });
                        ui.colored_label(
                            theme.text,
                            RichText::new(&code_buffer).monospace().size(12.0),
                        );
                    });
                    code_buffer.clear();
                    code_lang.clear();
                    in_code_block = false;
                } else {
                    code_lang = line.trim_start_matches('`').trim().to_string();
                    in_code_block = true;
                }
            } else if in_code_block {
                if !code_buffer.is_empty() {
                    code_buffer.push('\n');
                }
                code_buffer.push_str(line);
            } else if let Some(rest) = line.strip_prefix("# ") {
                ui.colored_label(theme.text, RichText::new(rest).strong().size(15.0));
            } else if let Some(rest) = line.strip_prefix("## ") {
                ui.colored_label(theme.text, RichText::new(rest).strong().size(13.0));
            } else if let Some(rest) = line.strip_prefix("### ") {
                ui.colored_label(theme.text, RichText::new(rest).strong().size(12.0));
            } else if line.starts_with("- [ ] ")
                || line.starts_with("- [x] ")
                || line.starts_with("- [X] ")
            {
                // Checklist items (Plan mode)
                let checked = !line.contains("- [ ] ");
                let label_text = &line[6..];
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let color = if checked {
                        theme.success
                    } else {
                        theme.text_secondary
                    };
                    let icon = if checked { "☑" } else { "☐" };
                    ui.colored_label(color, RichText::new(icon).size(13.0));
                    ui.add_space(4.0);
                    let txt_color = if checked {
                        theme.text_muted
                    } else {
                        theme.text_secondary
                    };
                    let rich = if checked {
                        RichText::new(label_text).size(12.5).strikethrough()
                    } else {
                        RichText::new(label_text).size(12.5)
                    };
                    ui.colored_label(txt_color, rich);
                });
            } else if let Some(rest) = line.strip_prefix("- ") {
                // Bullet list item
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.colored_label(theme.accent, "•");
                    ui.add_space(4.0);
                    ui.colored_label(theme.text_secondary, rest);
                });
            } else if line.starts_with("**") && line.ends_with("**") {
                let inner = line.trim_matches('*');
                ui.colored_label(theme.text, RichText::new(inner).strong());
            } else if line.is_empty() {
                ui.add_space(4.0);
            } else if line.starts_with("> ") {
                // Blockquote
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(ui.cursor().min, egui::vec2(3.0, 16.0)),
                        1.0,
                        theme.text_muted,
                    );
                    ui.add_space(8.0);
                    ui.colored_label(theme.text_muted, &line[2..]);
                });
            } else {
                // Detect numbered list: "1. ", "2. ", etc.
                let is_numbered = line
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
                    && line.find(". ").map(|i| i < 4).unwrap_or(false);
                if is_numbered {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.colored_label(theme.text_secondary, line);
                    });
                } else {
                    ui.colored_label(theme.text_secondary, line);
                }
            }
        }

        // Handle unclosed code block (streaming)
        if in_code_block && !code_buffer.is_empty() {
            make_frame(theme.background, 4.0, 8.0).show(ui, |ui| {
                if !code_lang.is_empty() {
                    ui.colored_label(theme.accent, RichText::new(&code_lang).size(10.0));
                }
                ui.colored_label(
                    theme.text,
                    RichText::new(&code_buffer).monospace().size(12.0),
                );
            });
        }
    });
}

fn render_tool_call(ui: &mut egui::Ui, tc: &ToolCallInfo, theme: &ThemeColors) {
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let (icon_color, icon) = if tc.success {
            (theme.success, "✓")
        } else {
            (theme.error, "✗")
        };
        ui.colored_label(icon_color, icon);
        ui.colored_label(theme.text_secondary, &tc.name);
        if !tc.summary.is_empty() {
            ui.colored_label(theme.text_muted, RichText::new(&tc.summary).small());
        }
    });
}
