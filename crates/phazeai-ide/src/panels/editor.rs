use egui::{self, Color32, FontId, Sense, Vec2, TextFormat, text::LayoutJob};
use ropey::Rope;
use std::path::PathBuf;
use syntect::highlighting::{ThemeSet, Style};
use syntect::parsing::SyntaxSet;
use syntect::easy::HighlightLines;

use crate::themes::ThemeColors;

pub struct EditorTab {
    pub path: Option<PathBuf>,
    pub title: String,
    pub rope: Rope,
    pub modified: bool,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub scroll_offset: f32,
    pub language: String,
}

impl EditorTab {
    pub fn new_untitled() -> Self {
        Self {
            path: None,
            title: "Untitled".to_string(),
            rope: Rope::new(),
            modified: false,
            cursor_line: 0,
            cursor_col: 0,
            scroll_offset: 0.0,
            language: String::new(),
        }
    }

    pub fn from_file(path: PathBuf) -> Result<Self, std::io::Error> {
        let content = std::fs::read_to_string(&path)?;
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());
        let language = detect_language(&path);
        Ok(Self {
            path: Some(path),
            title,
            rope: Rope::from_str(&content),
            modified: false,
            cursor_line: 0,
            cursor_col: 0,
            scroll_offset: 0.0,
            language,
        })
    }

    pub fn save(&mut self) -> Result<(), std::io::Error> {
        if let Some(ref path) = self.path {
            let content: String = self.rope.to_string();
            std::fs::write(path, content)?;
            self.modified = false;
        }
        Ok(())
    }

    pub fn content(&self) -> String {
        self.rope.to_string()
    }

    pub fn insert_char(&mut self, ch: char) {
        let idx = self.cursor_to_char_idx();
        self.rope.insert_char(idx, ch);
        if ch == '\n' {
            self.cursor_line += 1;
            self.cursor_col = 0;
        } else {
            self.cursor_col += 1;
        }
        self.modified = true;
    }

    pub fn delete_back(&mut self) {
        let idx = self.cursor_to_char_idx();
        if idx > 0 {
            if self.cursor_col == 0 && self.cursor_line > 0 {
                self.cursor_line -= 1;
                self.cursor_col = self.rope.line(self.cursor_line).len_chars();
                if self.cursor_col > 0
                    && self.rope.line(self.cursor_line).char(self.cursor_col - 1) == '\n'
                {
                    self.cursor_col -= 1;
                }
            } else if self.cursor_col > 0 {
                self.cursor_col -= 1;
            }
            self.rope.remove(idx - 1..idx);
            self.modified = true;
        }
    }

    fn cursor_to_char_idx(&self) -> usize {
        if self.cursor_line >= self.rope.len_lines() {
            return self.rope.len_chars();
        }
        let line_start = self.rope.line_to_char(self.cursor_line);
        let line_len = self.rope.line(self.cursor_line).len_chars();
        line_start + self.cursor_col.min(line_len)
    }
}

pub struct EditorPanel {
    pub tabs: Vec<EditorTab>,
    pub active_tab: usize,
    pub show_line_numbers: bool,
    pub font_size: f32,
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

impl EditorPanel {
    pub fn new(font_size: f32) -> Self {
        Self {
            tabs: vec![EditorTab::new_untitled()],
            active_tab: 0,
            show_line_numbers: true,
            font_size,
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }

    pub fn open_file(&mut self, path: PathBuf) {
        // Check if already open
        for (i, tab) in self.tabs.iter().enumerate() {
            if tab.path.as_ref() == Some(&path) {
                self.active_tab = i;
                return;
            }
        }
        match EditorTab::from_file(path) {
            Ok(tab) => {
                self.tabs.push(tab);
                self.active_tab = self.tabs.len() - 1;
            }
            Err(e) => {
                tracing::error!("Failed to open file: {e}");
            }
        }
    }

    pub fn close_tab(&mut self, index: usize) {
        if self.tabs.len() > 1 {
            self.tabs.remove(index);
            if self.active_tab >= self.tabs.len() {
                self.active_tab = self.tabs.len() - 1;
            }
        }
    }

    pub fn save_active(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Err(e) = tab.save() {
                tracing::error!("Failed to save: {e}");
            }
        }
    }

    pub fn new_tab(&mut self) {
        self.tabs.push(EditorTab::new_untitled());
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        // Tab bar
        ui.horizontal(|ui| {
            let mut close_idx = None;
            for (i, tab) in self.tabs.iter().enumerate() {
                let label = if tab.modified {
                    format!("{} *", tab.title)
                } else {
                    tab.title.clone()
                };

                let is_active = i == self.active_tab;
                let bg = if is_active {
                    theme.background
                } else {
                    theme.background_secondary
                };
                let fg = if is_active {
                    theme.text
                } else {
                    theme.text_secondary
                };

                let response = ui.allocate_ui_with_layout(
                    Vec2::new(0.0, 28.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.painter()
                                .rect_filled(ui.available_rect_before_wrap(), 0.0, bg);
                            ui.add_space(8.0);
                            ui.colored_label(fg, &label);
                            // Close button
                            if self.tabs.len() > 1 {
                                let close_resp = ui.small_button("x");
                                if close_resp.clicked() {
                                    close_idx = Some(i);
                                }
                            }
                            ui.add_space(8.0);
                        })
                        .response
                    },
                );

                if response.response.clicked() {
                    self.active_tab = i;
                }
            }

            // New tab button
            if ui.small_button("+").clicked() {
                self.new_tab();
            }

            if let Some(idx) = close_idx {
                self.close_tab(idx);
            }
        });

        ui.separator();

        // Editor content - extract needed data to avoid borrow conflict
        let active_tab = self.active_tab;
        let font_size = self.font_size;
        let show_line_numbers = self.show_line_numbers;

        if active_tab < self.tabs.len() {
            let syntax_set = &self.syntax_set;
            let theme_set = &self.theme_set;
            let tab = &mut self.tabs[active_tab];
            render_editor_content(ui, tab, theme, font_size, show_line_numbers, syntax_set, theme_set);
        }
    }
}

fn render_editor_content(
    ui: &mut egui::Ui,
    tab: &mut EditorTab,
    theme: &ThemeColors,
    font_size: f32,
    show_line_numbers: bool,
    syntax_set: &SyntaxSet,
    theme_set: &ThemeSet,
) {
    let line_height = font_size + 4.0;
    let total_lines = tab.rope.len_lines().max(1);
    let gutter_width = if show_line_numbers {
        let digits = format!("{}", total_lines).len();
        (digits as f32) * (font_size * 0.6) + 16.0
    } else {
        0.0
    };

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available_width = ui.available_width();

            for line_idx in 0..total_lines {
                ui.horizontal(|ui| {
                    // Line number gutter
                    if show_line_numbers {
                        let gutter_rect = ui.allocate_space(Vec2::new(gutter_width, line_height));
                        ui.painter().text(
                            gutter_rect.1.right_center() - Vec2::new(8.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            format!("{}", line_idx + 1),
                            FontId::monospace(font_size - 2.0),
                            theme.text_muted,
                        );
                    }

                    // Line content
                    let line_text = if line_idx < tab.rope.len_lines() {
                        let line = tab.rope.line(line_idx);
                        let s = line.to_string();
                        s.trim_end_matches('\n').to_string()
                    } else {
                        String::new()
                    };

                    let layout_job = highlight_line(&line_text, &tab.language, theme, font_size, syntax_set, theme_set);
                    let galley = ui.fonts(|f| f.layout_job(layout_job));

                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(
                            (available_width - gutter_width).max(galley.size().x + 20.0),
                            line_height,
                        ),
                        Sense::click(),
                    );

                    // Cursor highlight
                    if line_idx == tab.cursor_line {
                        ui.painter().rect_filled(
                            rect,
                            0.0,
                            Color32::from_rgba_premultiplied(255, 255, 255, 8),
                        );
                    }

                    // Draw text
                    ui.painter().galley(
                        rect.left_center() - Vec2::new(0.0, galley.size().y / 2.0),
                        galley,
                        theme.text,
                    );

                    if response.clicked() {
                        tab.cursor_line = line_idx;
                        if let Some(pos) = response.interact_pointer_pos() {
                            let x_offset = pos.x - rect.left();
                            tab.cursor_col =
                                (x_offset / (font_size * 0.6)).max(0.0) as usize;
                            let line_len = line_text.len();
                            tab.cursor_col = tab.cursor_col.min(line_len);
                        }
                    }
                });
            }
        });

    // Handle keyboard input
    ui.input(|i| {
        for event in &i.events {
            match event {
                egui::Event::Text(text) => {
                    for ch in text.chars() {
                        tab.insert_char(ch);
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command => {
                    tab.insert_char('\n');
                }
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    pressed: true,
                    ..
                } => {
                    tab.delete_back();
                }
                egui::Event::Key {
                    key: egui::Key::ArrowUp,
                    pressed: true,
                    ..
                } => {
                    if tab.cursor_line > 0 {
                        tab.cursor_line -= 1;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    pressed: true,
                    ..
                } => {
                    if tab.cursor_line + 1 < tab.rope.len_lines() {
                        tab.cursor_line += 1;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowLeft,
                    pressed: true,
                    ..
                } => {
                    if tab.cursor_col > 0 {
                        tab.cursor_col -= 1;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowRight,
                    pressed: true,
                    ..
                } => {
                    tab.cursor_col += 1;
                }
                _ => {}
            }
        }
    });
}

fn highlight_line(
    line: &str,
    language: &str,
    theme: &ThemeColors,
    font_size: f32,
    syntax_set: &SyntaxSet,
    theme_set: &ThemeSet,
) -> LayoutJob {
    let mut job = LayoutJob::default();

    if line.is_empty() {
        job.append(" ", 0.0, TextFormat {
            font_id: FontId::monospace(font_size),
            color: theme.text,
            ..Default::default()
        });
        return job;
    }

    // Try syntect highlighting
    let syntax = if !language.is_empty() {
        syntax_set.find_syntax_by_extension(language)
    } else {
        None
    };

    if let Some(syntax) = syntax {
        let highlight_theme = &theme_set.themes["base16-ocean.dark"];
        let mut h = HighlightLines::new(syntax, highlight_theme);
        if let Ok(ranges) = h.highlight_line(line, syntax_set) {
            for (style, text) in ranges {
                let color = syntect_to_egui_color(style);
                job.append(text, 0.0, TextFormat {
                    font_id: FontId::monospace(font_size),
                    color,
                    ..Default::default()
                });
            }
            return job;
        }
    }

    // Fallback: plain text
    job.append(line, 0.0, TextFormat {
        font_id: FontId::monospace(font_size),
        color: theme.text,
        ..Default::default()
    });
    job
}

fn syntect_to_egui_color(style: Style) -> Color32 {
    Color32::from_rgba_premultiplied(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
        style.foreground.a,
    )
}

fn detect_language(path: &std::path::Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string()
}
