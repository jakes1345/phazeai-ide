use egui::{self, Color32, FontId, Sense, Vec2, TextFormat, text::LayoutJob};
use ropey::Rope;
use std::path::PathBuf;
use syntect::highlighting::{ThemeSet, Style};
use syntect::parsing::SyntaxSet;
use syntect::easy::HighlightLines;

use crate::themes::ThemeColors;

// ── Undo/Redo ───────────────────────────────────────────────────────────

const MAX_UNDO_HISTORY: usize = 200;

#[derive(Clone)]
struct UndoSnapshot {
    content: String,
    cursor_line: usize,
    cursor_col: usize,
}

struct UndoStack {
    history: Vec<UndoSnapshot>,
    position: usize, // points to the current state (0 = initial)
}

impl UndoStack {
    fn new(initial: &Rope, cursor_line: usize, cursor_col: usize) -> Self {
        Self {
            history: vec![UndoSnapshot {
                content: initial.to_string(),
                cursor_line,
                cursor_col,
            }],
            position: 0,
        }
    }

    fn push(&mut self, rope: &Rope, cursor_line: usize, cursor_col: usize) {
        // Discard any future states beyond current position
        self.history.truncate(self.position + 1);
        self.history.push(UndoSnapshot {
            content: rope.to_string(),
            cursor_line,
            cursor_col,
        });
        self.position = self.history.len() - 1;

        // Cap history size
        if self.history.len() > MAX_UNDO_HISTORY {
            self.history.remove(0);
            self.position = self.history.len() - 1;
        }
    }

    fn undo(&mut self) -> Option<&UndoSnapshot> {
        if self.position > 0 {
            self.position -= 1;
            Some(&self.history[self.position])
        } else {
            None
        }
    }

    fn redo(&mut self) -> Option<&UndoSnapshot> {
        if self.position + 1 < self.history.len() {
            self.position += 1;
            Some(&self.history[self.position])
        } else {
            None
        }
    }
}

// ── Editor Tab ──────────────────────────────────────────────────────────

pub struct EditorTab {
    pub path: Option<PathBuf>,
    pub title: String,
    pub rope: Rope,
    pub modified: bool,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub language: String,
    undo_stack: UndoStack,
    /// Group edits: only push to undo after a pause or explicit save-point
    edit_count_since_snapshot: usize,
}

impl EditorTab {
    pub fn new_untitled() -> Self {
        let rope = Rope::new();
        Self {
            undo_stack: UndoStack::new(&rope, 0, 0),
            path: None,
            title: "Untitled".to_string(),
            rope,
            modified: false,
            cursor_line: 0,
            cursor_col: 0,
            language: String::new(),
            edit_count_since_snapshot: 0,
        }
    }

    pub fn from_file(path: PathBuf) -> Result<Self, std::io::Error> {
        let content = std::fs::read_to_string(&path)?;
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());
        let language = detect_language(&path);
        let rope = Rope::from_str(&content);
        Ok(Self {
            undo_stack: UndoStack::new(&rope, 0, 0),
            path: Some(path),
            title,
            rope,
            modified: false,
            cursor_line: 0,
            cursor_col: 0,
            language,
            edit_count_since_snapshot: 0,
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

    /// Reload file contents from disk (for file watcher integration)
    pub fn reload_from_disk(&mut self) {
        if let Some(ref path) = self.path {
            if let Ok(content) = std::fs::read_to_string(path) {
                self.rope = Rope::from_str(&content);
                self.modified = false;
                self.undo_stack = UndoStack::new(&self.rope, self.cursor_line, self.cursor_col);
                self.edit_count_since_snapshot = 0;
            }
        }
    }

    fn maybe_snapshot(&mut self) {
        self.edit_count_since_snapshot += 1;
        // Snapshot every 10 characters or on newline
        if self.edit_count_since_snapshot >= 10 {
            self.force_snapshot();
        }
    }

    fn force_snapshot(&mut self) {
        self.undo_stack.push(&self.rope, self.cursor_line, self.cursor_col);
        self.edit_count_since_snapshot = 0;
    }

    pub fn insert_char(&mut self, ch: char) {
        let idx = self.cursor_to_char_idx();
        self.rope.insert_char(idx, ch);
        if ch == '\n' {
            self.cursor_line += 1;
            self.cursor_col = 0;
            self.force_snapshot(); // always snapshot on newline
        } else {
            self.cursor_col += 1;
            self.maybe_snapshot();
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
            self.maybe_snapshot();
        }
    }

    pub fn undo(&mut self) {
        // Force a snapshot of the current state before undoing
        if self.edit_count_since_snapshot > 0 {
            self.force_snapshot();
        }
        if let Some(snapshot) = self.undo_stack.undo() {
            self.rope = Rope::from_str(&snapshot.content);
            self.cursor_line = snapshot.cursor_line;
            self.cursor_col = snapshot.cursor_col;
            self.modified = true;
            self.edit_count_since_snapshot = 0;
        }
    }

    pub fn redo(&mut self) {
        if let Some(snapshot) = self.undo_stack.redo() {
            self.rope = Rope::from_str(&snapshot.content);
            self.cursor_line = snapshot.cursor_line;
            self.cursor_col = snapshot.cursor_col;
            self.modified = true;
            self.edit_count_since_snapshot = 0;
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

// ── Find/Replace State ──────────────────────────────────────────────────

pub struct FindReplaceState {
    pub visible: bool,
    pub query: String,
    pub replacement: String,
    pub matches: Vec<(usize, usize)>,  // (line_idx, col_start)
    pub current_match: usize,
    pub case_sensitive: bool,
}

impl FindReplaceState {
    pub fn new() -> Self {
        Self {
            visible: false,
            query: String::new(),
            replacement: String::new(),
            matches: Vec::new(),
            current_match: 0,
            case_sensitive: false,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if !self.visible {
            self.matches.clear();
        }
    }

    pub fn search(&mut self, rope: &Rope) {
        self.matches.clear();
        if self.query.is_empty() {
            return;
        }

        let query = if self.case_sensitive {
            self.query.clone()
        } else {
            self.query.to_lowercase()
        };

        for line_idx in 0..rope.len_lines() {
            let line_str = rope.line(line_idx).to_string();
            let search_str = if self.case_sensitive {
                line_str.clone()
            } else {
                line_str.to_lowercase()
            };

            let mut start = 0;
            while let Some(pos) = search_str[start..].find(&query) {
                self.matches.push((line_idx, start + pos));
                start += pos + query.len();
            }
        }

        if !self.matches.is_empty() && self.current_match >= self.matches.len() {
            self.current_match = 0;
        }
    }

    pub fn next_match(&mut self) {
        if !self.matches.is_empty() {
            self.current_match = (self.current_match + 1) % self.matches.len();
        }
    }

    pub fn prev_match(&mut self) {
        if !self.matches.is_empty() {
            if self.current_match == 0 {
                self.current_match = self.matches.len() - 1;
            } else {
                self.current_match -= 1;
            }
        }
    }

    pub fn replace_current(&mut self, tab: &mut EditorTab) {
        if self.matches.is_empty() || self.query.is_empty() {
            return;
        }

        let (line_idx, col) = self.matches[self.current_match];
        let line_start = tab.rope.line_to_char(line_idx);
        let start_char = line_start + col;
        let end_char = start_char + self.query.len();

        tab.force_snapshot();
        tab.rope.remove(start_char..end_char);
        tab.rope.insert(start_char, &self.replacement);
        tab.modified = true;

        // Re-search
        self.search(&tab.rope);
    }

    pub fn replace_all(&mut self, tab: &mut EditorTab) {
        if self.matches.is_empty() || self.query.is_empty() {
            return;
        }

        tab.force_snapshot();

        // Replace from bottom to top to preserve positions
        let matches: Vec<_> = self.matches.iter().rev().cloned().collect();
        for (line_idx, col) in matches {
            let line_start = tab.rope.line_to_char(line_idx);
            let start_char = line_start + col;
            let end_char = start_char + self.query.len();
            tab.rope.remove(start_char..end_char);
            tab.rope.insert(start_char, &self.replacement);
        }
        tab.modified = true;

        // Re-search (should be empty now if replacement doesn't contain query)
        self.search(&tab.rope);
    }
}

// ── Editor Panel ────────────────────────────────────────────────────────

pub struct EditorPanel {
    pub tabs: Vec<EditorTab>,
    pub active_tab: usize,
    pub show_line_numbers: bool,
    pub font_size: f32,
    pub find_replace: FindReplaceState,
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
            find_replace: FindReplaceState::new(),
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

    pub fn undo(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.undo();
        }
    }

    pub fn redo(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.redo();
        }
    }

    pub fn toggle_find(&mut self) {
        self.find_replace.toggle();
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

        // Find/Replace bar
        if self.find_replace.visible {
            let mut do_search = false;
            let mut do_next = false;
            let mut do_prev = false;
            let mut do_replace = false;
            let mut do_replace_all = false;

            ui.horizontal(|ui| {
                ui.label("Find:");
                let find_resp = ui.add(
                    egui::TextEdit::singleline(&mut self.find_replace.query)
                        .desired_width(200.0)
                        .hint_text("Search...")
                );
                if find_resp.changed() {
                    do_search = true;
                }
                // Enter in search box = next match
                if find_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    do_next = true;
                }

                if ui.small_button("▲").clicked() {
                    do_prev = true;
                }
                if ui.small_button("▼").clicked() {
                    do_next = true;
                }

                let match_count = self.find_replace.matches.len();
                if match_count > 0 {
                    ui.label(format!(
                        "{}/{}",
                        self.find_replace.current_match + 1,
                        match_count
                    ));
                } else if !self.find_replace.query.is_empty() {
                    ui.colored_label(Color32::from_rgb(255, 100, 100), "No results");
                }

                ui.separator();
                ui.label("Replace:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.find_replace.replacement)
                        .desired_width(150.0)
                        .hint_text("Replace...")
                );
                if ui.small_button("Replace").clicked() {
                    do_replace = true;
                }
                if ui.small_button("All").clicked() {
                    do_replace_all = true;
                }

                if ui.small_button("✕").clicked() {
                    self.find_replace.visible = false;
                    self.find_replace.matches.clear();
                }
            });

            // Apply actions
            if do_search {
                if let Some(tab) = self.tabs.get(self.active_tab) {
                    let rope = tab.rope.clone();
                    self.find_replace.search(&rope);
                }
            }
            if do_next {
                self.find_replace.next_match();
                // Jump cursor to match
                if let Some(&(line, col)) = self.find_replace.matches.get(self.find_replace.current_match) {
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        tab.cursor_line = line;
                        tab.cursor_col = col;
                    }
                }
            }
            if do_prev {
                self.find_replace.prev_match();
                if let Some(&(line, col)) = self.find_replace.matches.get(self.find_replace.current_match) {
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        tab.cursor_line = line;
                        tab.cursor_col = col;
                    }
                }
            }
            if do_replace {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    self.find_replace.replace_current(tab);
                }
            }
            if do_replace_all {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    self.find_replace.replace_all(tab);
                }
            }

            ui.separator();
        }

        // Editor content
        let active_tab = self.active_tab;
        let font_size = self.font_size;
        let show_line_numbers = self.show_line_numbers;

        if active_tab < self.tabs.len() {
            let syntax_set = &self.syntax_set;
            let theme_set = &self.theme_set;
            let find_matches = if self.find_replace.visible {
                Some((&self.find_replace.matches, self.find_replace.current_match, self.find_replace.query.len()))
            } else {
                None
            };
            let tab = &mut self.tabs[active_tab];
            render_editor_content(ui, tab, theme, font_size, show_line_numbers, syntax_set, theme_set, find_matches);
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
    find_matches: Option<(&Vec<(usize, usize)>, usize, usize)>,
) {
    let line_height = font_size + 4.0;
    let total_lines = tab.rope.len_lines().max(1);
    let gutter_width = if show_line_numbers {
        let digits = format!("{}", total_lines).len();
        (digits as f32) * (font_size * 0.6) + 16.0
    } else {
        0.0
    };

    // Handle keyboard input
    let mut cursor_changed = false;
    ui.input(|i| {
        for event in &i.events {
            match event {
                egui::Event::Text(text) => {
                    for ch in text.chars() {
                        tab.insert_char(ch);
                    }
                    cursor_changed = true;
                }
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command => {
                    tab.insert_char('\n');
                    cursor_changed = true;
                }
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    pressed: true,
                    ..
                } => {
                    tab.delete_back();
                    cursor_changed = true;
                }
                egui::Event::Key {
                    key: egui::Key::Tab,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.shift => {
                    // Insert 4 spaces for tab
                    for _ in 0..4 {
                        tab.insert_char(' ');
                    }
                    cursor_changed = true;
                }
                egui::Event::Key {
                    key: egui::Key::ArrowUp,
                    pressed: true,
                    ..
                } => {
                    if tab.cursor_line > 0 {
                        tab.cursor_line -= 1;
                        cursor_changed = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    pressed: true,
                    ..
                } => {
                    if tab.cursor_line + 1 < tab.rope.len_lines() {
                        tab.cursor_line += 1;
                        cursor_changed = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowLeft,
                    pressed: true,
                    ..
                } => {
                    if tab.cursor_col > 0 {
                        tab.cursor_col -= 1;
                        cursor_changed = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowRight,
                    pressed: true,
                    ..
                } => {
                    tab.cursor_col += 1;
                    cursor_changed = true;
                }
                egui::Event::Key {
                    key: egui::Key::PageUp,
                    pressed: true,
                    ..
                } => {
                    tab.cursor_line = tab.cursor_line.saturating_sub(30);
                    cursor_changed = true;
                }
                egui::Event::Key {
                    key: egui::Key::PageDown,
                    pressed: true,
                    ..
                } => {
                    tab.cursor_line = (tab.cursor_line + 30).min(tab.rope.len_lines().saturating_sub(1));
                    cursor_changed = true;
                }
                egui::Event::Key {
                    key: egui::Key::Home,
                    pressed: true,
                    ..
                } => {
                    tab.cursor_col = 0;
                    cursor_changed = true;
                }
                egui::Event::Key {
                    key: egui::Key::End,
                    pressed: true,
                    ..
                } => {
                    if tab.cursor_line < tab.rope.len_lines() {
                        let line = tab.rope.line(tab.cursor_line);
                        let mut line_len = line.len_chars();
                        if line_len > 0 && line.char(line_len - 1) == '\n' {
                            line_len -= 1;
                        }
                        tab.cursor_col = line_len;
                        cursor_changed = true;
                    }
                }
                _ => {}
            }
        }
    });

    // Build set of lines that have search matches for quick lookup
    let match_lines: std::collections::HashSet<usize> = find_matches
        .map(|(matches, _, _)| matches.iter().map(|(l, _)| *l).collect())
        .unwrap_or_default();

    let scroll_output = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available_width = ui.available_width();
            let mut cursor_rect = None;

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
                        cursor_rect = Some(rect);
                    }

                    // Search match highlights
                    if match_lines.contains(&line_idx) {
                        if let Some((matches, current, query_len)) = &find_matches {
                            let char_width = font_size * 0.6;
                            for (i, (ml, mc)) in matches.iter().enumerate() {
                                if *ml == line_idx {
                                    let x_start = rect.left() + (*mc as f32) * char_width;
                                    let x_end = x_start + (*query_len as f32) * char_width;
                                    let highlight_color = if i == *current {
                                        Color32::from_rgba_premultiplied(255, 180, 0, 80) // current match = orange
                                    } else {
                                        Color32::from_rgba_premultiplied(255, 255, 0, 40) // other matches = faint yellow
                                    };
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(x_start, rect.top()),
                                            egui::pos2(x_end, rect.bottom()),
                                        ),
                                        0.0,
                                        highlight_color,
                                    );
                                }
                            }
                        }
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

            cursor_rect
        });

    // Auto-scroll to cursor if it changed
    if cursor_changed {
        if let Some(cursor_rect) = scroll_output.inner {
            ui.scroll_to_rect(cursor_rect, Some(egui::Align::Center));
        }
    }
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
