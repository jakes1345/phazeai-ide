use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use arboard;
use std::thread;

use floem::{
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, Modifiers},
    peniko::Color,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    style::{CursorStyle, Display},
    text::{Attrs, AttrsList, FamilyOwned, TextLayout, Weight},
    views::{canvas, container, dyn_stack, label, scroll, stack, Decorators},
    IntoView, Renderer,
};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use vte::{Params, Perform};

use phazeai_core::constants::terminal as term_consts;

use crate::theme::PhazeTheme;

// ── Terminal Colors ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum TermColor {
    Default,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

impl TermColor {
    fn to_floem_color(self, default: Color) -> Color {
        match self {
            TermColor::Default => default,
            TermColor::Rgb(r, g, b) => Color::from_rgb8(r, g, b),
            TermColor::Indexed(idx) => indexed_to_color(idx),
        }
    }
}

/// Convert a 256-color xterm index to a floem Color.
fn indexed_to_color(idx: u8) -> Color {
    const BASIC: [(u8, u8, u8); 16] = [
        (0, 0, 0),       // 0  Black
        (194, 54, 33),   // 1  Red
        (37, 188, 36),   // 2  Green
        (173, 173, 39),  // 3  Yellow
        (73, 46, 225),   // 4  Blue
        (211, 56, 211),  // 5  Magenta
        (51, 187, 200),  // 6  Cyan
        (203, 204, 205), // 7  White
        (129, 131, 131), // 8  Bright Black
        (252, 57, 31),   // 9  Bright Red
        (49, 231, 34),   // 10 Bright Green
        (234, 236, 35),  // 11 Bright Yellow
        (88, 51, 255),   // 12 Bright Blue
        (249, 53, 248),  // 13 Bright Magenta
        (20, 240, 240),  // 14 Bright Cyan
        (233, 235, 235), // 15 Bright White
    ];

    if (idx as usize) < BASIC.len() {
        let (r, g, b) = BASIC[idx as usize];
        return Color::from_rgb8(r, g, b);
    }

    if (16..=231).contains(&idx) {
        let i = idx - 16;
        let b = i % 6;
        let g = (i / 6) % 6;
        let r = i / 36;
        let to_val = |v: u8| if v == 0 { 0u8 } else { 55u8.saturating_add(v.saturating_mul(40)) };
        return Color::from_rgb8(to_val(r), to_val(g), to_val(b));
    }

    let v = 8u8.saturating_add((idx - 232).saturating_mul(10));
    Color::from_rgb8(v, v, v)
}

// ── Terminal Segment ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct TermSegment {
    text: String,
    fg: TermColor,
    bg: TermColor,
    bold: bool,
}

// ── Terminal Line ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct TermLine {
    segments: Vec<TermSegment>,
}

impl TermLine {
    fn new() -> Self {
        Self { segments: Vec::new() }
    }

    fn push_char(&mut self, ch: char, fg: TermColor, bg: TermColor, bold: bool) {
        if let Some(last) = self.segments.last_mut() {
            if last.fg == fg && last.bg == bg && last.bold == bold {
                last.text.push(ch);
                return;
            }
        }
        self.segments.push(TermSegment {
            text: ch.to_string(),
            fg,
            bg,
            bold,
        });
    }

    fn plain_text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }

    fn is_empty(&self) -> bool {
        self.segments.iter().all(|s| s.text.is_empty())
    }
}

// ── Terminal State ────────────────────────────────────────────────────────────

struct TermState {
    lines: Vec<TermLine>,
    current_line: TermLine,
    cur_fg: TermColor,
    cur_bg: TermColor,
    cur_bold: bool,
    cursor_col: usize,
}

impl TermState {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            current_line: TermLine::new(),
            cur_fg: TermColor::Default,
            cur_bg: TermColor::Default,
            cur_bold: false,
            cursor_col: 0,
        }
    }

    fn commit_line(&mut self) {
        let line = std::mem::replace(&mut self.current_line, TermLine::new());
        self.lines.push(line);
        if self.lines.len() > term_consts::SCROLLBACK_LIMIT {
            self.lines.drain(0..term_consts::SCROLLBACK_DRAIN);
        }
        self.cursor_col = 0;
    }

    fn push_char(&mut self, ch: char) {
        let fg = self.cur_fg;
        let bg = self.cur_bg;
        let bold = self.cur_bold;
        self.current_line.push_char(ch, fg, bg, bold);
        self.cursor_col += 1;
    }

    fn reset_attrs(&mut self) {
        self.cur_fg = TermColor::Default;
        self.cur_bg = TermColor::Default;
        self.cur_bold = false;
    }

    fn handle_sgr(&mut self, params: &Params) {
        let mut iter = params.iter();
        while let Some(param) = iter.next() {
            let code = param.first().copied().unwrap_or(0);
            match code {
                0 => self.reset_attrs(),
                1 => self.cur_bold = true,
                2 | 3 | 4 => {}
                22 => self.cur_bold = false,
                30..=37 => self.cur_fg = TermColor::Indexed(code as u8 - 30),
                38 => {
                    if let Some(mode) = iter.next() {
                        match mode.first().copied().unwrap_or(0) {
                            5 => {
                                if let Some(idx) = iter.next() {
                                    self.cur_fg = TermColor::Indexed(
                                        idx.first().copied().unwrap_or(0) as u8,
                                    );
                                }
                            }
                            2 => {
                                let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                self.cur_fg = TermColor::Rgb(r, g, b);
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
                                    self.cur_bg = TermColor::Indexed(
                                        idx.first().copied().unwrap_or(0) as u8,
                                    );
                                }
                            }
                            2 => {
                                let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                self.cur_bg = TermColor::Rgb(r, g, b);
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

// ── VTE Performer ─────────────────────────────────────────────────────────────

struct VtePerformer {
    state: Arc<Mutex<TermState>>,
}

impl Perform for VtePerformer {
    fn print(&mut self, c: char) {
        if let Ok(mut state) = self.state.lock() {
            state.push_char(c);
        }
    }

    fn execute(&mut self, byte: u8) {
        if let Ok(mut state) = self.state.lock() {
            match byte {
                b'\n' | 0x0B | 0x0C => state.commit_line(),
                b'\r' => state.cursor_col = 0,
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

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        if let Ok(mut state) = self.state.lock() {
            match action {
                'm' => state.handle_sgr(params),
                'J' => {
                    let p = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
                    if p == 2 || p == 3 {
                        if !state.current_line.is_empty() {
                            let line = std::mem::replace(&mut state.current_line, TermLine::new());
                            state.lines.push(line);
                        }
                    }
                }
                'K' => {
                    let p = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
                    if p == 0 {
                        state.current_line = TermLine::new();
                    }
                }
                'A' => {
                    let n = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(1).max(1) as usize;
                    if !state.current_line.is_empty() {
                        let line = std::mem::replace(&mut state.current_line, TermLine::new());
                        state.lines.push(line);
                    }
                    let len = state.lines.len();
                    state.lines.truncate(len.saturating_sub(n));
                }
                'H' | 'f' => {
                    let row = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(1);
                    if row == 1 && !state.current_line.is_empty() {
                        let line = std::mem::replace(&mut state.current_line, TermLine::new());
                        state.lines.push(line);
                    }
                }
                _ => {}
            }
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        if byte == b'c' {
            if let Ok(mut state) = self.state.lock() {
                state.reset_attrs();
            }
        }
    }
}

// ── Key → PTY byte encoding ───────────────────────────────────────────────────

/// Encode a floem KeyDown event into the bytes that should be sent to the PTY.
/// Returns an empty Vec if the key should not be forwarded (e.g. pure modifiers).
fn key_to_pty_bytes(event: &floem::keyboard::KeyEvent) -> Vec<u8> {
    use floem::keyboard::NamedKey::*;

    let mods = event.modifiers;
    let ctrl = mods.contains(Modifiers::CONTROL);
    let shift = mods.contains(Modifiers::SHIFT);
    let alt = mods.contains(Modifiers::ALT);

    match &event.key.logical_key {
        // ── Named keys ────────────────────────────────────────────────────────
        Key::Named(named) => match named {
            Enter => b"\r".to_vec(),
            Backspace => b"\x7f".to_vec(),
            Tab => {
                if shift {
                    b"\x1b[Z".to_vec() // Shift+Tab (reverse tab)
                } else {
                    b"\t".to_vec()
                }
            }
            Escape => b"\x1b".to_vec(),
            Delete => b"\x1b[3~".to_vec(),
            Insert => b"\x1b[2~".to_vec(),
            Home => {
                if ctrl { b"\x1b[1;5H".to_vec() } else { b"\x1b[H".to_vec() }
            }
            End => {
                if ctrl { b"\x1b[1;5F".to_vec() } else { b"\x1b[F".to_vec() }
            }
            ArrowUp => {
                if ctrl { b"\x1b[1;5A".to_vec() } else { b"\x1b[A".to_vec() }
            }
            ArrowDown => {
                if ctrl { b"\x1b[1;5B".to_vec() } else { b"\x1b[B".to_vec() }
            }
            ArrowRight => {
                if ctrl { b"\x1b[1;5C".to_vec() } else { b"\x1b[C".to_vec() }
            }
            ArrowLeft => {
                if ctrl { b"\x1b[1;5D".to_vec() } else { b"\x1b[D".to_vec() }
            }
            PageUp => b"\x1b[5~".to_vec(),
            PageDown => b"\x1b[6~".to_vec(),
            F1 =>  b"\x1bOP".to_vec(),
            F2 =>  b"\x1bOQ".to_vec(),
            F3 =>  b"\x1bOR".to_vec(),
            F4 =>  b"\x1bOS".to_vec(),
            F5 =>  b"\x1b[15~".to_vec(),
            F6 =>  b"\x1b[17~".to_vec(),
            F7 =>  b"\x1b[18~".to_vec(),
            F8 =>  b"\x1b[19~".to_vec(),
            F9 =>  b"\x1b[20~".to_vec(),
            F10 => b"\x1b[21~".to_vec(),
            F11 => b"\x1b[23~".to_vec(),
            F12 => b"\x1b[24~".to_vec(),
            _ => vec![],
        },

        // ── Character keys ────────────────────────────────────────────────────
        Key::Character(ch) => {
            let s = ch.as_str();
            if ctrl {
                // Ctrl+letter → control character (Ctrl+A = 0x01, Ctrl+Z = 0x1A)
                if let Some(c) = s.chars().next() {
                    if c.is_ascii_alphabetic() {
                        let ctrl_byte = (c.to_ascii_uppercase() as u8).wrapping_sub(b'A').wrapping_add(1);
                        return vec![ctrl_byte];
                    }
                    // Special Ctrl combos
                    match c {
                        '[' => return b"\x1b".to_vec(),   // Ctrl+[ = ESC
                        '\\' => return b"\x1c".to_vec(),
                        ']' => return b"\x1d".to_vec(),
                        '^' | '6' => return b"\x1e".to_vec(),
                        '_' | '-' => return b"\x1f".to_vec(),
                        _ => return vec![],
                    }
                }
                vec![]
            } else if alt {
                // Alt+key → ESC prefix
                let mut out = vec![b'\x1b'];
                out.extend_from_slice(s.as_bytes());
                out
            } else {
                // Regular printable character
                s.as_bytes().to_vec()
            }
        }

        _ => vec![],
    }
}

// ── Panel ─────────────────────────────────────────────────────────────────────

const FONT_SIZE: f32 = 13.0;
/// Maximum lines rendered at once — keeps the dyn_stack fast.
const MAX_RENDER_LINES: usize = 500;

fn build_line_layout(line: &TermLine, default_fg: Color, _default_bg: Color) -> TextLayout {
    let plain = line.plain_text();
    let fonts = [
        FamilyOwned::Name("JetBrains Mono".to_string()),
        FamilyOwned::Name("Fira Code".to_string()),
        FamilyOwned::Name("Cascadia Code".to_string()),
        FamilyOwned::Monospace,
    ];
    let default_attrs = Attrs::new().font_size(FONT_SIZE).color(default_fg).family(&fonts);
    let mut attrs_list = AttrsList::new(default_attrs);

    let mut byte_offset: usize = 0;
    for seg in &line.segments {
        let seg_len = seg.text.len();
        if seg_len == 0 { continue; }
        let start = byte_offset;
        let end = byte_offset + seg_len;
        byte_offset = end;

        let fg = seg.fg.to_floem_color(default_fg);
        let mut span_attrs = Attrs::new().font_size(FONT_SIZE).color(fg).family(&fonts);
        if seg.bold {
            span_attrs = span_attrs.weight(Weight::BOLD);
        }
        attrs_list.add_span(start..end, span_attrs);
    }

    let mut layout = TextLayout::new();
    layout.set_text(&plain, attrs_list, None);
    layout
}

/// One independent PTY terminal session rendered into a Floem view.
/// Each terminal tab gets its own call to `single_terminal()`.
fn single_terminal(theme: RwSignal<PhazeTheme>) -> impl IntoView {
    // ── Shared VTE state ──────────────────────────────────────────────────
    let term_state: Arc<Mutex<TermState>> = Arc::new(Mutex::new(TermState::new()));
    let pty_writer: Arc<Mutex<Option<Box<dyn Write + Send>>>> = Arc::new(Mutex::new(None));

    // ── Update channel: reader thread → reactive signal ───────────────────
    let (update_tx, update_rx) = std::sync::mpsc::channel::<()>();
    let update_signal = create_signal_from_channel(update_rx);

    // ── Reactive line buffer ──────────────────────────────────────────────
    let lines: RwSignal<Vec<TermLine>> = create_rw_signal(vec![]);
    // Increment on every update so auto-scroll can detect new output
    let line_version: RwSignal<u64> = create_rw_signal(0);
    // Cursor column position for rendering the cursor block
    let cursor_col_sig: RwSignal<usize> = create_rw_signal(0usize);

    // ── Spawn PTY thread ──────────────────────────────────────────────────
    {
        let term_state_t = Arc::clone(&term_state);
        let pty_writer_t = Arc::clone(&pty_writer);

        thread::spawn(move || {
            let pty_system = NativePtySystem::default();
            let pair = match pty_system.openpty(PtySize {
                rows: 40,
                cols: 220,
                pixel_width: 0,
                pixel_height: 0,
            }) {
                Ok(p) => p,
                Err(e) => {
                    if let Ok(mut s) = term_state_t.lock() {
                        let mut err = TermLine::new();
                        for ch in format!("PTY open error: {e}").chars() {
                            err.push_char(ch, TermColor::Indexed(1), TermColor::Default, true);
                        }
                        s.lines.push(err);
                    }
                    let _ = update_tx.send(());
                    return;
                }
            };

            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
            let mut cmd = CommandBuilder::new(&shell);
            cmd.env("TERM", term_consts::TERM_TYPE);
            cmd.env("COLORTERM", term_consts::COLOR_TERM);

            let child = match pair.slave.spawn_command(cmd) {
                Ok(c) => c,
                Err(e) => {
                    if let Ok(mut s) = term_state_t.lock() {
                        let mut err = TermLine::new();
                        for ch in format!("Shell spawn error: {e}").chars() {
                            err.push_char(ch, TermColor::Indexed(1), TermColor::Default, true);
                        }
                        s.lines.push(err);
                    }
                    let _ = update_tx.send(());
                    return;
                }
            };

            match pair.master.take_writer() {
                Ok(w) => {
                    if let Ok(mut guard) = pty_writer_t.lock() {
                        *guard = Some(w);
                    }
                }
                Err(e) => eprintln!("PTY take_writer error: {e}"),
            }

            let mut reader = match pair.master.try_clone_reader() {
                Ok(r) => r,
                Err(e) => { eprintln!("PTY clone_reader error: {e}"); return; }
            };

            let _child = child;
            let mut parser = vte::Parser::new();
            let mut performer = VtePerformer { state: Arc::clone(&term_state_t) };
            let mut buf = [0u8; term_consts::READ_BUFFER_SIZE];

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        for &byte in &buf[..n] {
                            parser.advance(&mut performer, byte);
                        }
                        let _ = update_tx.send(());
                    }
                    Err(_) => break,
                }
            }

            if let Ok(mut s) = term_state_t.lock() {
                if !s.current_line.is_empty() {
                    let line = std::mem::replace(&mut s.current_line, TermLine::new());
                    s.lines.push(line);
                }
            }
            let _ = update_tx.send(());
        });
    }

    // ── Sync thread state → reactive signals ─────────────────────────────
    {
        let term_state_e = Arc::clone(&term_state);
        create_effect(move |_| {
            update_signal.get();
            if let Ok(state) = term_state_e.lock() {
                let mut all_lines = state.lines.clone();
                if !state.current_line.is_empty() {
                    all_lines.push(state.current_line.clone());
                }
                lines.set(all_lines);
                line_version.update(|v| *v += 1);
                cursor_col_sig.set(state.cursor_col);
            }
        });
    }

    // ── PTY send helper ───────────────────────────────────────────────────
    let pty_writer_send = Arc::clone(&pty_writer);
    let send_to_pty = move |data: Vec<u8>| {
        if data.is_empty() { return; }
        if let Ok(mut guard) = pty_writer_send.lock() {
            if let Some(ref mut w) = *guard {
                let _ = w.write_all(&data);
                let _ = w.flush();
            }
        }
    };

    // ── Focus state ───────────────────────────────────────────────────────
    let is_focused = create_rw_signal(false);

    // ── Output list ───────────────────────────────────────────────────────
    let output_list = dyn_stack(
        move || {
            let all = lines.get();
            let total = all.len();
            let start = total.saturating_sub(MAX_RENDER_LINES);
            all.into_iter().enumerate().skip(start).collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, line)| {
            let segments = line.segments.clone();

            let initial_layout = {
                let t = theme.get();
                let p = &t.palette;
                if segments.is_empty() {
                    let mut layout = TextLayout::new();
                    let attrs = Attrs::new()
                        .font_size(FONT_SIZE)
                        .color(p.text_primary)
                        .family(&[FamilyOwned::Monospace]);
                    layout.set_text(" ", AttrsList::new(attrs), None);
                    layout
                } else {
                    build_line_layout(&line, p.text_primary, p.bg_base)
                }
            };

            let layout_signal: RwSignal<TextLayout> = create_rw_signal(initial_layout);

            create_effect(move |_| {
                let t = theme.get();
                let p = &t.palette;
                let new_layout = if segments.is_empty() {
                    let mut layout = TextLayout::new();
                    let attrs = Attrs::new()
                        .font_size(FONT_SIZE)
                        .color(p.text_primary)
                        .family(&[FamilyOwned::Monospace]);
                    layout.set_text(" ", AttrsList::new(attrs), None);
                    layout
                } else {
                    let reconstructed = TermLine { segments: segments.clone() };
                    build_line_layout(&reconstructed, p.text_primary, p.bg_base)
                };
                layout_signal.set(new_layout);
            });

            container(
                floem::views::rich_text(move || layout_signal.get())
                    .style(|s| s.width_full()),
            )
            .style(|s| s.padding_horiz(8.0).padding_vert(1.0).width_full())
        },
    )
    .style(|s| s.flex_col().width_full().padding_vert(4.0));

    // ── Scroll area — also the keyboard target ────────────────────────────
    let output_scroll = scroll(output_list).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        // Subtle accent border when focused so user knows keys are going to PTY
        let border_color = if is_focused.get() {
            p.accent.with_alpha(0.35)
        } else {
            p.bg_base // invisible border when unfocused
        };
        s.flex_grow(1.0)
         .min_height(0.0)
         .width_full()
         .background(p.bg_base)
         .border(1.0)
         .border_color(border_color)
    });

    // ── Cursor overlay ────────────────────────────────────────────────────
    // Renders a semi-transparent block cursor at the current column position
    // on the last (current) line. Uses 8.4 px per character as approximation.
    let cursor_view = canvas(move |cx, _size| {
        let col = cursor_col_sig.get() as f64;
        let x = 8.0 + col * 8.4;
        let y = 0.0;
        cx.fill(
            &floem::kurbo::Rect::new(x, y, x + 7.0, y + 16.0),
            floem::peniko::Color::from_rgba8(200, 200, 200, 180),
            0.0,
        );
    })
    .style(|s| {
        s.absolute()
         .width_full()
         .height(16.0)
         .inset_bottom(0.0)
         .inset_left(0.0)
         .z_index(5)
         .pointer_events_none()
    });

    let terminal_with_cursor = stack((output_scroll, cursor_view))
        .style(|s| s.flex_col().flex_grow(1.0).min_height(0.0).width_full());

    // Clones for the clipboard key handler closure.
    let pty_writer_c = Arc::clone(&pty_writer);
    let term_state_c = Arc::clone(&term_state);

    // Wrap scroll in a keyboard-navigable container so it can receive focus
    // and capture all key events to forward to the PTY.
    let output_area = container(terminal_with_cursor)
        .keyboard_navigable()
        .on_event_stop(EventListener::FocusGained, move |_| {
            is_focused.set(true);
        })
        .on_event_stop(EventListener::FocusLost, move |_| {
            is_focused.set(false);
        })
        .on_event_stop(EventListener::KeyDown, move |event| {
            if let Event::KeyDown(e) = event {
                let ctrl  = e.modifiers.contains(Modifiers::CONTROL);
                let shift = e.modifiers.contains(Modifiers::SHIFT);

                // Ctrl+Shift+V — paste clipboard text into terminal
                if ctrl && shift {
                    if let Key::Character(ref ch) = e.key.logical_key {
                        if ch.as_str() == "v" || ch.as_str() == "V" {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                if let Ok(text) = clipboard.get_text() {
                                    if let Ok(mut w) = pty_writer_c.lock() {
                                        if let Some(writer) = w.as_mut() {
                                            let _ = writer.write_all(text.as_bytes());
                                            let _ = writer.flush();
                                        }
                                    }
                                }
                            }
                            return;
                        }

                        // Ctrl+Shift+C — copy all visible terminal text to clipboard
                        if ch.as_str() == "c" || ch.as_str() == "C" {
                            if let Ok(state) = term_state_c.lock() {
                                let mut parts: Vec<String> = state.lines.iter()
                                    .map(|line| {
                                        line.segments.iter()
                                            .map(|seg| seg.text.as_str())
                                            .collect::<String>()
                                    })
                                    .collect();
                                if !state.current_line.is_empty() {
                                    parts.push(
                                        state.current_line.segments.iter()
                                            .map(|seg| seg.text.as_str())
                                            .collect::<String>()
                                    );
                                }
                                let text = parts.join("\n");
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    let _ = clipboard.set_text(text);
                                }
                            }
                            return;
                        }
                    }
                }

                let bytes = key_to_pty_bytes(e);
                send_to_pty(bytes);
            }
        })
        .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    // ── Assemble — just the output area (no header; tabs manage the title) ──
    output_area
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.flex_grow(1.0)
             .min_height(0.0)
             .width_full()
             .background(p.bg_base)
        })
}

// ── Multi-tab terminal panel ───────────────────────────────────────────────────

/// Real PTY terminal panel with multiple tab support.
///
/// A "+" button spawns a new shell session. Each session is independent —
/// closing a tab kills that PTY (the OS will reap it) and removes the tab.
pub fn terminal_panel(theme: RwSignal<PhazeTheme>) -> impl IntoView {
    // Tab list: each entry is a unique numeric ID.  Stable IDs let dyn_stack
    // keep existing terminal instances alive when new tabs are added.
    let tab_ids: RwSignal<Vec<usize>> = create_rw_signal(vec![1]);
    let active_tab: RwSignal<usize>   = create_rw_signal(1);
    let next_id:    RwSignal<usize>   = create_rw_signal(2);

    // ── Tab bar ───────────────────────────────────────────────────────────
    let tab_bar = stack((
        // Scrollable row of tabs
        dyn_stack(
            move || tab_ids.get().into_iter().enumerate().collect::<Vec<_>>(),
            |(_, id)| *id,
            move |(_, id)| {
                let is_active = move || active_tab.get() == id;
                let hovered   = create_rw_signal(false);

                let title = label(move || format!("Terminal {id}"))
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        let active = is_active();
                        let hov    = hovered.get();
                        s.font_size(11.0)
                         .color(if active { p.text_primary } else if hov { p.text_secondary } else { p.text_muted })
                         .padding_horiz(10.0)
                         .padding_vert(5.0)
                         .border_bottom(if active { 2.0 } else { 0.0 })
                         .border_color(p.accent)
                         .cursor(CursorStyle::Pointer)
                    })
                    .on_click_stop(move |_| active_tab.set(id));

                // × close button — only show when hovered and there is more than 1 tab
                let close_btn = container(label(|| "×"))
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        let show = hovered.get() && tab_ids.get().len() > 1;
                        s.font_size(11.0)
                         .color(p.text_muted)
                         .padding_horiz(4.0)
                         .padding_vert(4.0)
                         .border_radius(3.0)
                         .cursor(CursorStyle::Pointer)
                         .hover(|s| s.background(p.error.with_alpha(0.25)))
                         .apply_if(!show, |s| s.display(Display::None))
                    })
                    .on_click_stop(move |_| {
                        tab_ids.update(|ids| ids.retain(|&x| x != id));
                        // Switch to last remaining tab if we closed the active one
                        if active_tab.get_untracked() == id {
                            if let Some(&last) = tab_ids.get_untracked().last() {
                                active_tab.set(last);
                            }
                        }
                    });

                container(stack((title, close_btn)).style(|s| s.items_center()))
                    .on_event_stop(EventListener::PointerEnter, move |_| hovered.set(true))
                    .on_event_stop(EventListener::PointerLeave, move |_| hovered.set(false))
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        s.items_center()
                         .border_right(1.0)
                         .border_color(p.border)
                         .apply_if(is_active(), |s| s.background(p.bg_elevated))
                    })
            },
        )
        .style(|s| s.flex_row()),

        // "+" new terminal button
        container(label(|| "+"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(10.0)
                 .padding_vert(6.0)
                 .font_size(16.0)
                 .color(p.text_muted)
                 .cursor(CursorStyle::Pointer)
                 .hover(|s| s.background(p.bg_elevated).color(p.accent))
            })
            .on_click_stop(move |_| {
                let id = next_id.get();
                next_id.set(id + 1);
                tab_ids.update(|ids| ids.push(id));
                active_tab.set(id);
            }),
    ))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.flex_row()
         .items_stretch()
         .border_bottom(1.0)
         .border_color(p.border)
         .background(p.bg_panel)
         .width_full()
         .min_height(30.0)
    });

    // ── Terminal instances (one per tab, hidden when not active) ──────────
    let instances = dyn_stack(
        move || tab_ids.get().into_iter().collect::<Vec<_>>(),
        |id| *id,
        move |id| {
            single_terminal(theme)
                .style(move |s| {
                    s.size_full()
                     .apply_if(active_tab.get() != id, |s| s.display(Display::None))
                })
        },
    )
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    stack((tab_bar, instances))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.flex_col()
             .width_full()
             .height_full()
             .background(p.bg_base)
             .border_top(1.0)
             .border_color(p.border)
        })
}
