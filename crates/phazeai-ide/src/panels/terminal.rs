use egui::{self, text::LayoutJob, Color32, FontId, RichText, TextFormat};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use vte::{Parser, Perform};

use crate::themes::ThemeColors;

// ── Terminal Colors ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TermColor {
    Default,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

impl TermColor {
    fn to_egui(self, default_color: Color32, is_fg: bool) -> Color32 {
        match self {
            TermColor::Default => default_color,
            TermColor::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
            TermColor::Indexed(idx) => indexed_color(idx, is_fg),
        }
    }
}

/// Convert 256-color index to RGB
fn indexed_color(idx: u8, is_fg: bool) -> Color32 {
    let basic = [
        // Normal (0-7)
        Color32::from_rgb(0, 0, 0),       // 0 black
        Color32::from_rgb(194, 54, 33),   // 1 red
        Color32::from_rgb(37, 188, 36),   // 2 green
        Color32::from_rgb(173, 173, 39),  // 3 yellow
        Color32::from_rgb(73, 46, 225),   // 4 blue
        Color32::from_rgb(211, 56, 211),  // 5 magenta
        Color32::from_rgb(51, 187, 200),  // 6 cyan
        Color32::from_rgb(203, 204, 205), // 7 white
        // Bright (8-15)
        Color32::from_rgb(129, 131, 131), // 8 bright black (gray)
        Color32::from_rgb(252, 57, 31),   // 9 bright red
        Color32::from_rgb(49, 231, 34),   // 10 bright green
        Color32::from_rgb(234, 236, 35),  // 11 bright yellow
        Color32::from_rgb(88, 51, 255),   // 12 bright blue
        Color32::from_rgb(249, 53, 248),  // 13 bright magenta
        Color32::from_rgb(20, 240, 240),  // 14 bright cyan
        Color32::from_rgb(233, 235, 235), // 15 bright white
    ];

    if (idx as usize) < basic.len() {
        return basic[idx as usize];
    }

    // 216-color cube (16-231)
    if (16..=231).contains(&idx) {
        let i = idx - 16;
        let b = i % 6;
        let g = (i / 6) % 6;
        let r = i / 36;
        let to_val = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return Color32::from_rgb(to_val(r), to_val(g), to_val(b));
    }

    // Grayscale ramp (232-255)
    if idx >= 232 {
        let v = 8 + (idx - 232) * 10;
        return Color32::from_rgb(v, v, v);
    }

    if is_fg {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
}

// ── Terminal Segment (colored text span) ─────────────────────────────────

#[derive(Clone, Debug)]
pub struct TermSegment {
    pub text: String,
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
}

impl TermSegment {
    fn new(
        fg: TermColor,
        bg: TermColor,
        bold: bool,
        italic: bool,
        underline: bool,
        dim: bool,
    ) -> Self {
        Self {
            text: String::new(),
            fg,
            bg,
            bold,
            italic,
            underline,
            dim,
        }
    }
}

// ── Terminal Line ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct TermLine {
    pub segments: Vec<TermSegment>,
}

impl TermLine {
    fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_char(
        &mut self,
        ch: char,
        fg: TermColor,
        bg: TermColor,
        bold: bool,
        italic: bool,
        underline: bool,
        dim: bool,
    ) {
        if let Some(last) = self.segments.last_mut() {
            if last.fg == fg
                && last.bg == bg
                && last.bold == bold
                && last.italic == italic
                && last.underline == underline
                && last.dim == dim
            {
                last.text.push(ch);
                return;
            }
        }
        let mut seg = TermSegment::new(fg, bg, bold, italic, underline, dim);
        seg.text.push(ch);
        self.segments.push(seg);
    }

    fn is_empty(&self) -> bool {
        self.segments.iter().all(|s| s.text.is_empty())
    }
}

// ── VTE State ─────────────────────────────────────────────────────────────

pub struct TermState {
    pub lines: Vec<TermLine>,
    pub current_line: TermLine,
    pub cur_fg: TermColor,
    pub cur_bg: TermColor,
    pub cur_bold: bool,
    pub cur_italic: bool,
    pub cur_underline: bool,
    pub cur_dim: bool,
}

impl TermState {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            current_line: TermLine::new(),
            cur_fg: TermColor::Default,
            cur_bg: TermColor::Default,
            cur_bold: false,
            cur_italic: false,
            cur_underline: false,
            cur_dim: false,
        }
    }

    fn commit_line(&mut self) {
        let line = std::mem::replace(&mut self.current_line, TermLine::new());
        self.lines.push(line);
        if self.lines.len() > 10000 {
            self.lines.drain(0..1000);
        }
    }

    fn push_char(&mut self, ch: char) {
        let fg = self.cur_fg;
        let bg = self.cur_bg;
        let bold = self.cur_bold;
        let italic = self.cur_italic;
        let underline = self.cur_underline;
        let dim = self.cur_dim;
        self.current_line
            .push_char(ch, fg, bg, bold, italic, underline, dim);
    }

    fn reset_attrs(&mut self) {
        self.cur_fg = TermColor::Default;
        self.cur_bg = TermColor::Default;
        self.cur_bold = false;
        self.cur_italic = false;
        self.cur_underline = false;
        self.cur_dim = false;
    }

    fn handle_sgr(&mut self, params: &vte::Params) {
        let mut iter = params.iter();
        while let Some(param) = iter.next() {
            let code = param.first().copied().unwrap_or(0);
            match code {
                0 => self.reset_attrs(),
                1 => self.cur_bold = true,
                2 => self.cur_dim = true,
                3 => self.cur_italic = true,
                4 => self.cur_underline = true,
                22 => {
                    self.cur_bold = false;
                    self.cur_dim = false;
                }
                23 => self.cur_italic = false,
                24 => self.cur_underline = false,
                30..=37 => self.cur_fg = TermColor::Indexed(code as u8 - 30),
                38 => {
                    if let Some(mode) = iter.next() {
                        match mode.first().copied().unwrap_or(0) {
                            5 => {
                                if let Some(idx) = iter.next() {
                                    self.cur_fg =
                                        TermColor::Indexed(idx.first().copied().unwrap_or(0) as u8);
                                }
                            }
                            2 => {
                                let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                self.cur_fg = TermColor::Rgb(r as u8, g as u8, b as u8);
                            }
                            _ => {}
                        }
                    }
                }
                39 => self.cur_fg = TermColor::Default,
                40..=47 => self.cur_bg = TermColor::Indexed(code as u8 - 40),
                48 => {
                    if let Some(mode) = iter.next() {
                        match mode.first().copied().unwrap_or(0) {
                            5 => {
                                if let Some(idx) = iter.next() {
                                    self.cur_bg =
                                        TermColor::Indexed(idx.first().copied().unwrap_or(0) as u8);
                                }
                            }
                            2 => {
                                let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                self.cur_bg = TermColor::Rgb(r as u8, g as u8, b as u8);
                            }
                            _ => {}
                        }
                    }
                }
                49 => self.cur_bg = TermColor::Default,
                90..=97 => self.cur_fg = TermColor::Indexed(code as u8 - 90 + 8),
                100..=107 => self.cur_bg = TermColor::Indexed(code as u8 - 100 + 8),
                _ => {}
            }
        }
    }
}

struct VtePerformer {
    state: Arc<Mutex<TermState>>,
}

impl Perform for VtePerformer {
    fn print(&mut self, ch: char) {
        if let Ok(mut state) = self.state.lock() {
            state.push_char(ch);
        }
    }

    fn execute(&mut self, byte: u8) {
        if let Ok(mut state) = self.state.lock() {
            match byte {
                b'\n' => state.commit_line(),
                b'\r' => {}
                b'\x08' => {
                    if let Some(last) = state.current_line.segments.last_mut() {
                        last.text.pop();
                    }
                }
                b'\x07' => {}
                _ => {}
            }
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        if let Ok(mut state) = self.state.lock() {
            match action {
                'm' => state.handle_sgr(params),
                'J' => {
                    let param = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(0);
                    if param == 2 || param == 3 {
                        state.commit_line();
                    }
                }
                'K' => {
                    let param = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(0);
                    if param == 0 {
                        state.current_line = TermLine::new();
                    }
                }
                'A' => {
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(1)
                        .max(1);
                    if !state.current_line.is_empty() {
                        let line = std::mem::replace(&mut state.current_line, TermLine::new());
                        state.lines.push(line);
                    }
                    let len = state.lines.len();
                    let trim = (n as usize).min(len);
                    state.lines.truncate(len - trim);
                }
                'H' | 'f' => {
                    let row = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(1);
                    if row == 1 && !state.current_line.is_empty() {
                        let line = std::mem::replace(&mut state.current_line, TermLine::new());
                        state.lines.push(line);
                    }
                }
                _ => {}
            }
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        if byte == b'c' {
            if let Ok(mut state) = self.state.lock() {
                state.reset_attrs();
            }
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {
    }
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
}

// ── Terminal Session ───────────────────────────────────────────────────────

/// A single terminal session (PTY + state). Multiple sessions form the tab list.
pub struct TerminalSession {
    pub title: String,
    pub cwd: PathBuf,
    term_state: Arc<Mutex<TermState>>,
    writer: Option<Box<dyn Write + Send>>,
    /// Master PTY handle — kept alive for PTY resize.
    pty_master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    alive: Arc<Mutex<bool>>,
    input: String,
    input_history: Vec<String>,
    history_pos: Option<usize>,
    scroll_to_bottom: bool,
    /// Last measured size (cols, rows) — used to detect resize.
    last_panel_size: Option<(u16, u16)>,
}

impl TerminalSession {
    pub fn new(cwd: PathBuf, title: String) -> Self {
        let mut session = Self {
            title,
            cwd: cwd.clone(),
            term_state: Arc::new(Mutex::new(TermState::new())),
            writer: None,
            pty_master: None,
            alive: Arc::new(Mutex::new(false)),
            input: String::new(),
            input_history: Vec::new(),
            history_pos: None,
            scroll_to_bottom: true,
            last_panel_size: None,
        };
        session.spawn_shell();
        session
    }

    pub fn is_running(&self) -> bool {
        *self.alive.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn inject_output(&self, text: &str) {
        let mut state = self.term_state.lock().unwrap_or_else(|e| e.into_inner());
        let mut header = TermLine::new();
        for ch in "\u{2500}\u{2500}\u{2500} agent ".chars() {
            header.push_char(
                ch,
                TermColor::Indexed(8),
                TermColor::Default,
                false,
                false,
                false,
                true,
            );
        }
        state.lines.push(header);
        for output_line in text.lines() {
            let mut line = TermLine::new();
            for ch in output_line.chars() {
                line.push_char(
                    ch,
                    TermColor::Default,
                    TermColor::Default,
                    false,
                    false,
                    false,
                    false,
                );
            }
            state.lines.push(line);
        }
        if state.lines.len() > 10000 {
            let drain_count = state.lines.len() - 10000;
            state.lines.drain(0..drain_count);
        }
    }

    pub fn recent_output(&self, n: usize) -> String {
        let state = self.term_state.lock().unwrap_or_else(|e| e.into_inner());
        let total = state.lines.len();
        let start = total.saturating_sub(n);
        state.lines[start..]
            .iter()
            .map(|line| {
                line.segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn execute_command(&mut self, cmd_str: &str) {
        self.send_raw(&format!("{}\n", cmd_str));
    }

    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd.clone();
        self.send_raw(&format!("cd {:?}\n", cwd));
    }

    fn send_raw(&mut self, text: &str) {
        if let Some(ref mut writer) = self.writer {
            let _ = writer.write_all(text.as_bytes());
            let _ = writer.flush();
        }
    }

    /// Resize the PTY to match the given character dimensions.
    fn try_resize(&mut self, cols: u16, rows: u16) {
        if let Some(ref master) = self.pty_master {
            let _ = master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        self.last_panel_size = Some((cols, rows));
    }

    fn push_history(&mut self, cmd: String) {
        if !cmd.is_empty() && self.input_history.last() != Some(&cmd) {
            self.input_history.push(cmd);
        }
        self.history_pos = None;
    }

    fn history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            None => self.input_history.len() - 1,
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.history_pos = Some(pos);
        self.input = self.input_history[pos].clone();
    }

    fn history_next(&mut self) {
        match self.history_pos {
            None => {}
            Some(pos) => {
                if pos + 1 >= self.input_history.len() {
                    self.history_pos = None;
                    self.input.clear();
                } else {
                    self.history_pos = Some(pos + 1);
                    self.input = self.input_history[pos + 1].clone();
                }
            }
        }
    }

    fn spawn_shell(&mut self) {
        let pty_system = NativePtySystem::default();
        let pair = match pty_system.openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(p) => p,
            Err(e) => {
                if let Ok(mut state) = self.term_state.lock() {
                    let mut line = TermLine::new();
                    for ch in format!("! PTY error: {e}").chars() {
                        line.push_char(
                            ch,
                            TermColor::Indexed(1),
                            TermColor::Default,
                            false,
                            false,
                            false,
                            false,
                        );
                    }
                    state.lines.push(line);
                }
                return;
            }
        };

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(&self.cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => {
                if let Ok(mut state) = self.term_state.lock() {
                    let mut line = TermLine::new();
                    for ch in format!("Shell spawn error: {e}").chars() {
                        line.push_char(
                            ch,
                            TermColor::Indexed(1),
                            TermColor::Default,
                            false,
                            false,
                            false,
                            false,
                        );
                    }
                    state.lines.push(line);
                }
                return;
            }
        };

        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("PTY writer error: {e}");
                return;
            }
        };
        self.writer = Some(writer);

        let mut reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("PTY reader error: {e}");
                return;
            }
        };

        // Store master so we can resize the PTY when the panel is resized.
        self.pty_master = Some(pair.master);

        *self.alive.lock().unwrap() = true;
        let term_state = Arc::clone(&self.term_state);
        let alive = Arc::clone(&self.alive);

        thread::spawn(move || {
            let _child = child;
            let state_arc = Arc::clone(&term_state);
            let mut parser = Parser::new();
            let mut performer = VtePerformer { state: state_arc };
            let mut buf = [0u8; 8192];

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        for &byte in &buf[..n] {
                            parser.advance(&mut performer, byte);
                        }
                    }
                    Err(_) => break,
                }
            }

            if let Ok(mut state) = term_state.lock() {
                if !state.current_line.is_empty() {
                    let line = std::mem::replace(&mut state.current_line, TermLine::new());
                    state.lines.push(line);
                }
            }

            *alive.lock().unwrap_or_else(|e| e.into_inner()) = false;
        });
    }

    /// Render the session content (output + input bar) inside an already-measured area.
    /// Returns whether the input field was focused this frame.
    fn show_content(&mut self, ui: &mut egui::Ui, theme: &ThemeColors, font_size: f32) {
        let bg = theme.background_secondary;
        let green = Color32::from_rgb(80, 200, 100);
        let red = Color32::from_rgb(220, 80, 80);

        // Status + controls row
        ui.horizontal(|ui| {
            let status_color = if self.is_running() { green } else { red };
            ui.colored_label(status_color, "●");
            ui.label(
                RichText::new(&self.title)
                    .font(FontId::proportional(12.0))
                    .color(theme.text_secondary),
            );
            if !self.is_running() {
                ui.separator();
                if ui.small_button("⟳ Restart").clicked() {
                    self.spawn_shell();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Clear").clicked() {
                    if let Ok(mut state) = self.term_state.lock() {
                        state.lines.clear();
                        state.current_line = TermLine::new();
                    }
                }
            });
        });

        ui.separator();

        let default_fg = theme.text;
        let default_bg = bg;

        // ── PTY resize: measure available area and resize if changed ─────────
        let available = ui.available_size();
        // Reserve space for input bar (~30px) and header (~24px)
        let content_height = (available.y - 54.0).max(60.0);
        let char_width = font_size * 0.6; // approx monospace char width
        let cols = ((available.x / char_width) as u16).max(10);
        let rows = ((content_height / (font_size + 4.0)) as u16).max(4);

        if self.last_panel_size != Some((cols, rows)) {
            self.try_resize(cols, rows);
        }

        // Output scroll area
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(self.scroll_to_bottom)
            .max_height(content_height)
            .show(ui, |ui| {
                if let Ok(state) = self.term_state.lock() {
                    for term_line in &state.lines {
                        render_term_line(ui, term_line, font_size, default_fg, default_bg);
                    }
                    if !state.current_line.is_empty() {
                        render_term_line(
                            ui,
                            &state.current_line,
                            font_size,
                            default_fg,
                            default_bg,
                        );
                    }
                }
            });

        self.scroll_to_bottom = false;

        // Input bar
        ui.separator();
        ui.horizontal(|ui| {
            ui.colored_label(green, "›");

            let input_resp = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .font(FontId::monospace(font_size))
                    .text_color(theme.text)
                    .desired_width(ui.available_width() - 60.0)
                    .hint_text("command...")
                    .frame(false),
            );

            if input_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let cmd = self.input.clone();
                self.push_history(cmd.clone());
                self.send_raw(&format!("{}\n", cmd));
                self.input.clear();
                self.scroll_to_bottom = true;
                input_resp.request_focus();
            }

            if input_resp.has_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    self.history_prev();
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    self.history_next();
                }
                if ui.input(|i| i.key_pressed(egui::Key::C) && i.modifiers.ctrl) {
                    self.send_raw("\x03");
                }
                if ui.input(|i| i.key_pressed(egui::Key::D) && i.modifiers.ctrl) {
                    self.send_raw("\x04");
                }
                if ui.input(|i| i.key_pressed(egui::Key::L) && i.modifiers.ctrl) {
                    self.send_raw("\x0c");
                    if let Ok(mut state) = self.term_state.lock() {
                        state.lines.clear();
                        state.current_line = TermLine::new();
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Tab)) {
                    self.send_raw("\t");
                    self.scroll_to_bottom = true;
                }
            }

            if ui.button("↵").clicked() {
                let cmd = self.input.clone();
                self.push_history(cmd.clone());
                self.send_raw(&format!("{}\n", cmd));
                self.input.clear();
                self.scroll_to_bottom = true;
            }
        });
    }
}

// ── Terminal Panel (multi-tab) ────────────────────────────────────────────

pub struct TerminalPanel {
    sessions: Vec<TerminalSession>,
    active: usize,
    /// Public flag — set from outside to trigger scroll-to-bottom on the active session.
    pub scroll_to_bottom: bool,
}

impl Default for TerminalPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalPanel {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| home_dir());
        let session = TerminalSession::new(cwd, "1".to_string());
        Self {
            sessions: vec![session],
            active: 0,
            scroll_to_bottom: false,
        }
    }

    fn active_session(&self) -> &TerminalSession {
        &self.sessions[self.active]
    }

    fn active_session_mut(&mut self) -> &mut TerminalSession {
        &mut self.sessions[self.active]
    }

    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.active_session_mut().set_cwd(cwd);
    }

    pub fn get_cwd(&self) -> PathBuf {
        self.active_session().cwd.clone()
    }

    pub fn inject_output(&self, text: &str) {
        self.active_session().inject_output(text);
    }

    pub fn recent_output(&self, n: usize) -> String {
        self.active_session().recent_output(n)
    }

    pub fn execute_command(&mut self, cmd: &str) {
        self.active_session_mut().execute_command(cmd);
    }

    pub fn is_running(&self) -> bool {
        self.active_session().is_running()
    }

    pub fn clear_output(&mut self) {
        if let Some(session) = self.sessions.get_mut(self.active) {
            if let Ok(mut state) = session.term_state.lock() {
                state.lines.clear();
                state.current_line = TermLine::new();
            }
        }
    }

    pub fn new_session(&mut self) {
        let cwd = self.active_session().cwd.clone();
        let idx = self.sessions.len() + 1;
        let session = TerminalSession::new(cwd, idx.to_string());
        self.sessions.push(session);
        self.active = self.sessions.len() - 1;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        // Apply scroll flag from outside
        if self.scroll_to_bottom {
            if let Some(s) = self.sessions.get_mut(self.active) {
                s.scroll_to_bottom = true;
            }
            self.scroll_to_bottom = false;
        }

        let bg = theme.background_secondary;
        let font_size = 13.0_f32;

        egui::Frame::none()
            .fill(bg)
            .inner_margin(6.0)
            .show(ui, |ui| {
                // ── Tab bar ───────────────────────────────────────────────
                ui.horizontal(|ui| {
                    // "Terminal" label
                    ui.label(
                        RichText::new("Terminal")
                            .font(FontId::proportional(12.0))
                            .color(theme.text_secondary),
                    );
                    ui.separator();

                    // Session tabs
                    let mut close_idx: Option<usize> = None;
                    let n = self.sessions.len();
                    for i in 0..n {
                        let is_active = i == self.active;
                        let tab_label = self.sessions[i].title.clone();
                        let alive = self.sessions[i].is_running();

                        let dot_color = if alive {
                            Color32::from_rgb(80, 200, 100)
                        } else {
                            Color32::from_rgb(220, 80, 80)
                        };

                        let tab_bg = if is_active {
                            theme.background
                        } else {
                            theme.background_secondary
                        };
                        let text_color = if is_active {
                            theme.text
                        } else {
                            theme.text_secondary
                        };

                        egui::Frame::none()
                            .fill(tab_bg)
                            .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                            .rounding(egui::Rounding::same(3.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.colored_label(dot_color, "●");
                                    let resp = ui.selectable_label(
                                        is_active,
                                        RichText::new(&tab_label).color(text_color).size(12.0),
                                    );
                                    if resp.clicked() && !is_active {
                                        self.active = i;
                                    }
                                    // Close button (only show if more than one tab)
                                    if n > 1
                                        && ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new("×")
                                                        .color(theme.text_secondary)
                                                        .size(11.0),
                                                )
                                                .frame(false),
                                            )
                                            .clicked()
                                    {
                                        close_idx = Some(i);
                                    }
                                });
                            });
                    }

                    // "+" button — new terminal tab
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("+").color(theme.text_secondary).size(14.0),
                            )
                            .frame(false),
                        )
                        .on_hover_text("New terminal")
                        .clicked()
                    {
                        let cwd = self.active_session().cwd.clone();
                        let idx = self.sessions.len() + 1;
                        let session = TerminalSession::new(cwd, idx.to_string());
                        self.sessions.push(session);
                        self.active = self.sessions.len() - 1;
                    }

                    // Handle tab close
                    if let Some(idx) = close_idx {
                        self.sessions.remove(idx);
                        if self.active >= self.sessions.len() {
                            self.active = self.sessions.len() - 1;
                        }
                    }
                });

                ui.separator();

                // ── Active session content ────────────────────────────────
                if let Some(session) = self.sessions.get_mut(self.active) {
                    session.show_content(ui, theme, font_size);
                }
            });
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

fn render_term_line(
    ui: &mut egui::Ui,
    line: &TermLine,
    font_size: f32,
    default_fg: Color32,
    default_bg: Color32,
) {
    if line.segments.is_empty() {
        ui.add_space(font_size + 2.0);
        return;
    }

    let mut job = LayoutJob::default();
    for seg in &line.segments {
        if seg.text.is_empty() {
            continue;
        }
        let fg = seg.fg.to_egui(default_fg, true);
        let mut format = TextFormat {
            font_id: FontId::monospace(font_size),
            color: if seg.dim {
                Color32::from_rgba_premultiplied(fg.r(), fg.g(), fg.b(), 150)
            } else {
                fg
            },
            ..Default::default()
        };
        if seg.underline {
            format.underline = egui::Stroke::new(1.0, fg);
        }
        if seg.bg != TermColor::Default {
            format.background = seg.bg.to_egui(default_bg, false);
        }
        job.append(seg.text.as_str(), 0.0, format);
    }

    if !job.sections.is_empty() {
        ui.label(job);
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}
