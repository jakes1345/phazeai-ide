//! Integrated terminal panel: tabs, split, find, AI command bar and the
//! command-block list around one or more terminal sessions.
//!
//! Each session runs a shell in a PTY (`portable-pty`) and keeps its screen in
//! a [`TermCore`] (`terminal_emu.rs`, built on `alacritty_terminal`'s cell
//! grid). A reader thread feeds PTY output to the core and writes the
//! emulator's replies back; the view paints the core's [`GridSnapshot`] on one
//! canvas and turns keys, the mouse and paste into PTY input.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use alacritty_terminal::index::Side;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{CursorShape, Rgb};
use floem::{
    action::exec_after,
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, Modifiers, NamedKey},
    kurbo::{Point, Rect},
    peniko::Color,
    pointer::{MouseButton, PointerButton},
    reactive::{create_effect, create_memo, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    style::{CursorStyle, Display},
    text::{Attrs, AttrsList, FamilyOwned, Style as FontStyle, TextLayout, Weight},
    views::{canvas, container, dyn_stack, empty, label, scroll, stack, text_input, Decorators},
    IntoView, Renderer, View,
};
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtyPair, PtySize, PtySystem};

use super::terminal_emu::{
    key_bytes, mouse_report, paste_bytes, process_pty_output, text_key_bytes, FindHit, FindMark,
    GridSnapshot, Mods, MouseCode, SnapCell, TermCore, TermKey, TermPalette, TermSize, ANSI_BASE,
};
use crate::commands::{execute_command, match_global_shortcut};
use crate::domain_state::IdeState;
use crate::theme::{PhazePalette, PhazeTheme};
use crate::util::safe_get;
use phazeai_core::constants::terminal as term_consts;
use phazeai_core::Settings;

// ── PTY defaults ──────────────────────────────────────────────────────────────

/// Size of a new PTY before the panel reports its real size. Wide enough for
/// side-by-side editors (two editors need ~180 columns) and never smaller than
/// a classic 80×24 terminal.
pub const DEFAULT_PTY_COLS: u16 = 220;
pub const DEFAULT_PTY_ROWS: u16 = 40;
const _: () = assert!(DEFAULT_PTY_COLS >= 180 && DEFAULT_PTY_ROWS >= 24);

const SHELLS: &[&str] = &["bash", "zsh", "fish", "sh"];

/// Left margin holding the prompt marks, in pixels.
const GUTTER_W: f64 = 10.0;
const PAD_TOP: f64 = 2.0;
const PAD_RIGHT: f64 = 6.0;
/// Below this the view is hidden (an inactive tab): do not resize the PTY.
const MIN_COLS: usize = 4;
const MIN_ROWS: usize = 2;

/// Shell integration typed into the shell once it starts: OSC 133;D;$? (exit
/// code of the last command), OSC 133;A (prompt start), OSC 7 (cwd). D comes
/// before A so each block gets its own code.
const PROMPT_COMMAND_INJECT: &str = "export PROMPT_COMMAND='printf \"\\033]133;D;$?\\007\\033]133;A\\007\\033]7;file://${HOSTNAME}${PWD}\\007\"'\n";

/// Shared PTY writer — `Arc` so callers (Run in Terminal, the AI command bar)
/// can write to the shell once the PTY is ready.
type SharedPtyWriter = Arc<Mutex<Option<Box<dyn Write + Send>>>>;
type SharedMaster = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;

/// `Write` adapter over the shared PTY writer (drops bytes until it is set).
struct SharedWriterRef<'a>(&'a SharedPtyWriter);

impl Write for SharedWriterRef<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock() {
            Ok(mut guard) => match guard.as_mut() {
                Some(w) => w.write(buf),
                None => Ok(buf.len()),
            },
            Err(_) => Err(std::io::Error::other("pty writer lock poisoned")),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock() {
            Ok(mut guard) => match guard.as_mut() {
                Some(w) => w.flush(),
                None => Ok(()),
            },
            Err(_) => Ok(()),
        }
    }
}

fn write_shared(writer: &SharedPtyWriter, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let mut w = SharedWriterRef(writer);
    let _ = w.write_all(data);
    let _ = w.flush();
}

// ── Colours and fonts ─────────────────────────────────────────────────────────

fn to_color(c: Rgb) -> Color {
    Color::from_rgb8(c.r, c.g, c.b)
}

fn to_rgb(c: Color) -> Rgb {
    let x = c.to_rgba8();
    Rgb {
        r: x.r,
        g: x.g,
        b: x.b,
    }
}

/// Terminal colours from the IDE theme: text and background follow the
/// palette, the 16 ANSI colours keep the terminal's table.
fn palette_from_theme(p: &PhazePalette) -> TermPalette {
    TermPalette {
        fg: to_rgb(p.text_primary),
        bg: to_rgb(p.bg_base),
        cursor: to_rgb(p.text_primary),
        ansi: ANSI_BASE,
    }
}

fn term_fonts() -> [FamilyOwned; 4] {
    [
        FamilyOwned::Name("JetBrains Mono".to_string()),
        FamilyOwned::Name("Fira Code".to_string()),
        FamilyOwned::Name("Cascadia Code".to_string()),
        FamilyOwned::Monospace,
    ]
}

/// Size of one terminal cell in pixels for a font size, measured from the
/// font the grid is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CellMetrics {
    w: f64,
    h: f64,
    font_size: f32,
}

fn measure_cell(font_size: f32) -> CellMetrics {
    let fonts = term_fonts();
    let attrs = Attrs::new().font_size(font_size).family(&fonts);
    let mut layout = TextLayout::new();
    layout.set_text("MMMMMMMMMM", AttrsList::new(attrs), None);
    let size = layout.size();
    CellMetrics {
        w: (size.width / 10.0).max(1.0),
        h: size.height.ceil().max(1.0),
        font_size,
    }
}

/// Grid size that fits `px` (width, height) pixels.
fn grid_size_for(px: (f64, f64), m: CellMetrics) -> (usize, usize) {
    let cols = ((px.0 - GUTTER_W - PAD_RIGHT) / m.w).floor().max(0.0) as usize;
    let rows = ((px.1 - PAD_TOP) / m.h).floor().max(0.0) as usize;
    (cols, rows)
}

// ── Painting ──────────────────────────────────────────────────────────────────

/// Text runs of one row, keyed by a hash of what affects them.
struct RowLayouts {
    key: u64,
    runs: Vec<(usize, TextLayout)>,
}

fn row_key(row: &[SnapCell], font_size: f32) -> u64 {
    let mut h = DefaultHasher::new();
    font_size.to_bits().hash(&mut h);
    for cell in row {
        cell.c.hash(&mut h);
        cell.zerowidth.hash(&mut h);
        (cell.style.fg.r, cell.style.fg.g, cell.style.fg.b).hash(&mut h);
        (cell.style.bold, cell.style.italic, cell.wide, cell.spacer).hash(&mut h);
    }
    h.finish()
}

/// A cell drawn as part of a run of ASCII text.
fn is_run_cell(cell: &SnapCell) -> bool {
    !cell.spacer
        && !cell.wide
        && cell.zerowidth.is_empty()
        && (cell.c == ' ' || cell.c.is_ascii_graphic())
}

fn text_attrs<'a>(
    fonts: &'a [FamilyOwned],
    font_size: f32,
    fg: Rgb,
    bold: bool,
    italic: bool,
) -> Attrs<'a> {
    let mut a = Attrs::new()
        .font_size(font_size)
        .family(fonts)
        .color(to_color(fg));
    if bold {
        a = a.weight(Weight::BOLD);
    }
    if italic {
        a = a.style(FontStyle::Italic);
    }
    a
}

/// Lay out one row: runs of ASCII cells become one layout each (a monospace
/// font keeps them on the grid); any other glyph gets its own layout placed
/// at its cell, so a wide or fallback glyph cannot shift the rest of the row.
fn build_row_runs(row: &[SnapCell], font_size: f32) -> Vec<(usize, TextLayout)> {
    let fonts = term_fonts();
    let mut runs = Vec::new();
    let mut col = 0;
    while col < row.len() {
        let cell = &row[col];
        if cell.spacer || cell.c == ' ' && is_run_cell(cell) {
            col += 1;
            continue;
        }
        if is_run_cell(cell) {
            let start = col;
            let mut end = col;
            let mut j = col;
            while j < row.len() && is_run_cell(&row[j]) {
                if row[j].c != ' ' {
                    end = j + 1;
                }
                j += 1;
            }
            let first = &row[start].style;
            let mut list = AttrsList::new(text_attrs(
                &fonts,
                font_size,
                first.fg,
                first.bold,
                first.italic,
            ));
            let mut text = String::with_capacity(end - start);
            let mut span_start = 0;
            let mut span_style = (first.fg, first.bold, first.italic);
            for c in &row[start..end] {
                let style = (c.style.fg, c.style.bold, c.style.italic);
                if style != span_style {
                    let (fg, b, i) = span_style;
                    list.add_span(
                        span_start..text.len(),
                        text_attrs(&fonts, font_size, fg, b, i),
                    );
                    span_start = text.len();
                    span_style = style;
                }
                text.push(c.c);
            }
            let (fg, b, i) = span_style;
            list.add_span(
                span_start..text.len(),
                text_attrs(&fonts, font_size, fg, b, i),
            );
            let mut layout = TextLayout::new();
            layout.set_text(&text, list, None);
            runs.push((start, layout));
            col = j;
        } else {
            let mut text = String::new();
            text.push(cell.c);
            text.extend(cell.zerowidth.iter());
            let s = &cell.style;
            let mut layout = TextLayout::new();
            layout.set_text(
                &text,
                AttrsList::new(text_attrs(&fonts, font_size, s.fg, s.bold, s.italic)),
                None,
            );
            runs.push((col, layout));
            col += 1;
        }
    }
    runs
}

/// Theme colours the painter needs besides the snapshot.
struct PaintColors {
    selection: Color,
    find_match: Color,
    find_current: Color,
    ok: Color,
    error: Color,
    muted: Color,
    accent: Color,
    scrollbar: Color,
}

impl PaintColors {
    fn from_palette(p: &PhazePalette) -> Self {
        Self {
            selection: p.selection.with_alpha(0.45),
            find_match: p.warning.with_alpha(0.35),
            find_current: p.accent.with_alpha(0.6),
            ok: Color::from_rgb8(80, 200, 80),
            error: p.error,
            muted: p.text_muted,
            accent: p.accent,
            scrollbar: p.text_muted.with_alpha(0.35),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_grid(
    cx: &mut floem::context::PaintCx,
    size: floem::kurbo::Size,
    mut snap: GridSnapshot,
    history: usize,
    m: CellMetrics,
    colors: &PaintColors,
    focused: bool,
    cache: &RefCell<Vec<Option<RowLayouts>>>,
) {
    let (w, h) = (m.w, m.h);
    let x0 = GUTTER_W;
    cx.fill(
        &Rect::new(0.0, 0.0, size.width, size.height),
        to_color(snap.default_bg),
        0.0,
    );

    // A focused block cursor shows its character in the background colour.
    if let Some(cur) = snap.cursor {
        if focused && cur.shape == CursorShape::Block {
            if let Some(cell) = snap.cells.get_mut(cur.row).and_then(|r| r.get_mut(cur.col)) {
                cell.style.fg = cell.style.bg;
            }
        }
    }

    let cell_rect = |row: usize, c0: usize, c1: usize| {
        let y = PAD_TOP + row as f64 * h;
        Rect::new(x0 + c0 as f64 * w, y, x0 + c1 as f64 * w, y + h)
    };

    // Backgrounds, selection and find highlights.
    for (r, row) in snap.cells.iter().enumerate() {
        let mut c = 0;
        while c < row.len() {
            let bg = row[c].style.bg;
            let mut e = c + 1;
            while e < row.len() && row[e].style.bg == bg {
                e += 1;
            }
            if bg != snap.default_bg {
                cx.fill(&cell_rect(r, c, e), to_color(bg), 0.0);
            }
            c = e;
        }
        let mut c = 0;
        while c < row.len() {
            let key = (row[c].selected, row[c].find);
            let mut e = c + 1;
            while e < row.len() && (row[e].selected, row[e].find) == key {
                e += 1;
            }
            match key.1 {
                FindMark::Current => cx.fill(&cell_rect(r, c, e), colors.find_current, 0.0),
                FindMark::Match => cx.fill(&cell_rect(r, c, e), colors.find_match, 0.0),
                FindMark::None => {}
            }
            if key.0 {
                cx.fill(&cell_rect(r, c, e), colors.selection, 0.0);
            }
            c = e;
        }
    }

    // Block cursor fill (under the text).
    let cursor_color = to_color(snap.cursor_color);
    if let Some(cur) = snap.cursor {
        let span = if cur.wide { 2 } else { 1 };
        if focused && cur.shape == CursorShape::Block {
            cx.fill(
                &cell_rect(cur.row, cur.col, cur.col + span),
                cursor_color,
                0.0,
            );
        }
    }

    // Text, from the per-row layout cache.
    {
        let mut cache = cache.borrow_mut();
        cache.resize_with(snap.rows, || None);
        for (r, row) in snap.cells.iter().enumerate() {
            let key = row_key(row, m.font_size);
            let fresh = !matches!(&cache[r], Some(entry) if entry.key == key);
            if fresh {
                cache[r] = Some(RowLayouts {
                    key,
                    runs: build_row_runs(row, m.font_size),
                });
            }
            if let Some(entry) = &cache[r] {
                let y = PAD_TOP + r as f64 * h;
                for (col, layout) in &entry.runs {
                    cx.draw_text(layout, Point::new(x0 + *col as f64 * w, y));
                }
            }
        }
    }

    // Underline and strikeout.
    for (r, row) in snap.cells.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let s = &cell.style;
            if !(s.underline || s.strikeout) || cell.spacer {
                continue;
            }
            let span = if cell.wide { 2.0 } else { 1.0 };
            let x = x0 + c as f64 * w;
            let y = PAD_TOP + r as f64 * h;
            if s.underline {
                cx.fill(
                    &Rect::new(x, y + h - 1.5, x + w * span, y + h - 0.5),
                    to_color(s.fg),
                    0.0,
                );
            }
            if s.strikeout {
                cx.fill(
                    &Rect::new(x, y + h * 0.5 - 0.5, x + w * span, y + h * 0.5 + 0.5),
                    to_color(s.fg),
                    0.0,
                );
            }
        }
    }

    // Other cursor shapes (and the unfocused outline).
    if let Some(cur) = snap.cursor {
        let span = if cur.wide { 2 } else { 1 };
        let r = cell_rect(cur.row, cur.col, cur.col + span);
        let outline = |cx: &mut floem::context::PaintCx| {
            for edge in [
                Rect::new(r.x0, r.y0, r.x1, r.y0 + 1.0),
                Rect::new(r.x0, r.y1 - 1.0, r.x1, r.y1),
                Rect::new(r.x0, r.y0, r.x0 + 1.0, r.y1),
                Rect::new(r.x1 - 1.0, r.y0, r.x1, r.y1),
            ] {
                cx.fill(&edge, cursor_color, 0.0);
            }
        };
        match cur.shape {
            CursorShape::Block if !focused => outline(cx),
            CursorShape::HollowBlock => outline(cx),
            CursorShape::Beam => {
                cx.fill(&Rect::new(r.x0, r.y0, r.x0 + 2.0, r.y1), cursor_color, 0.0)
            }
            CursorShape::Underline => {
                cx.fill(&Rect::new(r.x0, r.y1 - 2.0, r.x1, r.y1), cursor_color, 0.0)
            }
            _ => {}
        }
    }

    // Prompt marks in the gutter, coloured by the command's exit code.
    for (r, mark) in snap.marks.iter().enumerate() {
        let Some(mark) = mark else { continue };
        let color = match (mark.finished, mark.exit_code) {
            (true, Some(0)) => colors.ok,
            (true, Some(_)) => colors.error,
            (true, None) => colors.muted,
            (false, _) => colors.accent,
        };
        let y = PAD_TOP + r as f64 * h;
        cx.fill(&Rect::new(3.0, y + 1.0, 6.0, y + h - 1.0), color, 1.5);
    }

    // Scroll position, while scrolled back.
    if snap.display_offset > 0 && history > 0 {
        let total = (history + snap.rows) as f64;
        let track = size.height;
        let thumb = (snap.rows as f64 / total * track).max(12.0);
        let top = (history - snap.display_offset) as f64 / total * track;
        cx.fill(
            &Rect::new(size.width - 4.0, top, size.width - 1.0, top + thumb),
            colors.scrollbar,
            1.5,
        );
    }
}

// ── Keys ──────────────────────────────────────────────────────────────────────

fn mods_of(m: &Modifiers) -> Mods {
    Mods {
        shift: m.contains(Modifiers::SHIFT),
        alt: m.contains(Modifiers::ALT),
        ctrl: m.contains(Modifiers::CONTROL),
    }
}

/// Encode a key press into the bytes the PTY should get (empty for keys that
/// send nothing, such as bare modifiers). `app_cursor` is the terminal's
/// application-cursor mode.
fn key_to_pty_bytes(event: &floem::keyboard::KeyEvent, app_cursor: bool) -> Vec<u8> {
    let mods = mods_of(&event.modifiers);
    match &event.key.logical_key {
        Key::Named(named) => {
            let key = match named {
                NamedKey::Enter => TermKey::Enter,
                NamedKey::Backspace => TermKey::Backspace,
                NamedKey::Tab => TermKey::Tab,
                NamedKey::Escape => TermKey::Escape,
                NamedKey::Delete => TermKey::Delete,
                NamedKey::Insert => TermKey::Insert,
                NamedKey::Home => TermKey::Home,
                NamedKey::End => TermKey::End,
                NamedKey::ArrowUp => TermKey::Up,
                NamedKey::ArrowDown => TermKey::Down,
                NamedKey::ArrowRight => TermKey::Right,
                NamedKey::ArrowLeft => TermKey::Left,
                NamedKey::PageUp => TermKey::PageUp,
                NamedKey::PageDown => TermKey::PageDown,
                NamedKey::Space => return text_key_bytes(" ", mods),
                NamedKey::F1 => TermKey::F(1),
                NamedKey::F2 => TermKey::F(2),
                NamedKey::F3 => TermKey::F(3),
                NamedKey::F4 => TermKey::F(4),
                NamedKey::F5 => TermKey::F(5),
                NamedKey::F6 => TermKey::F(6),
                NamedKey::F7 => TermKey::F(7),
                NamedKey::F8 => TermKey::F(8),
                NamedKey::F9 => TermKey::F(9),
                NamedKey::F10 => TermKey::F(10),
                NamedKey::F11 => TermKey::F(11),
                NamedKey::F12 => TermKey::F(12),
                _ => return Vec::new(),
            };
            key_bytes(key, mods, app_cursor)
        }
        Key::Character(ch) => text_key_bytes(ch.as_str(), mods),
        _ => Vec::new(),
    }
}

// ── One terminal session ──────────────────────────────────────────────────────

/// A finished command, as shown in the block list.
#[derive(Clone, Debug, PartialEq)]
struct BlockInfo {
    id: u64,
    prompt_row: u64,
    exit_code: Option<i32>,
    command: String,
    output: String,
}

fn write_notice(core: &Mutex<TermCore>, text: &str) {
    if let Ok(mut c) = core.lock() {
        c.feed(format!("\r\n\x1b[1;31m{text}\x1b[0m\r\n").as_bytes());
    }
}

/// Spawn the shell in a PTY and run the reader loop (on its own thread).
fn spawn_pty(
    shell: String,
    core: Arc<Mutex<TermCore>>,
    pty_writer: SharedPtyWriter,
    pty_master: SharedMaster,
    update_tx: std::sync::mpsc::SyncSender<()>,
) {
    thread::spawn(move || {
        let size = core.lock().map(|c| c.size()).unwrap_or(TermSize::new(
            DEFAULT_PTY_COLS as usize,
            DEFAULT_PTY_ROWS as usize,
        ));
        let pty_size = |s: TermSize| PtySize {
            rows: s.rows as u16,
            cols: s.cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        };
        let PtyPair { master, slave } = match NativePtySystem::default().openpty(pty_size(size)) {
            Ok(p) => p,
            Err(e) => {
                write_notice(&core, &format!("PTY open error: {e}"));
                let _ = update_tx.try_send(());
                return;
            }
        };

        let mut cmd = CommandBuilder::new(&shell);
        cmd.env("TERM", term_consts::TERM_TYPE);
        cmd.env("COLORTERM", term_consts::COLOR_TERM);
        let mut child = match slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => {
                write_notice(&core, &format!("Shell spawn error: {e}"));
                let _ = update_tx.try_send(());
                return;
            }
        };
        // Only the child holds the slave now, so reads end when it exits.
        drop(slave);

        match master.take_writer() {
            Ok(w) => {
                if let Ok(mut guard) = pty_writer.lock() {
                    *guard = Some(w);
                }
            }
            Err(e) => {
                tracing::error!(target: "phazeai_ui::terminal", "PTY take_writer error: {e}")
            }
        }

        // Shell integration: OSC 7 (cwd) and OSC 133 (blocks, exit codes).
        {
            let w = Arc::clone(&pty_writer);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                write_shared(&w, PROMPT_COMMAND_INJECT.as_bytes());
            });
        }

        let mut reader = match master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(target: "phazeai_ui::terminal", "PTY clone_reader error: {e}");
                write_notice(&core, &format!("PTY read error: {e}"));
                let _ = update_tx.try_send(());
                return;
            }
        };

        // Keep the master for resizes from the UI thread; catch up with a
        // resize that happened while the PTY was starting.
        if let Ok(mut guard) = pty_master.lock() {
            let now = core.lock().map(|c| c.size()).unwrap_or(size);
            if now != size {
                let _ = master.resize(pty_size(now));
            }
            *guard = Some(master);
        }

        let mut buf = vec![0u8; term_consts::READ_BUFFER_SIZE];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let outcome =
                        process_pty_output(&core, &buf[..n], &mut SharedWriterRef(&pty_writer));
                    if let Some(text) = outcome.clipboard {
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            let _ = clipboard.set_text(text);
                        }
                    }
                    let _ = update_tx.try_send(());
                }
            }
        }
        let status = child.wait();
        if let Ok(mut c) = core.lock() {
            let msg = match status {
                Ok(s) => format!("[process exited with code {}]", s.exit_code()),
                Err(e) => format!("[process ended: {e}]"),
            };
            c.feed(format!("\r\n\x1b[2m{msg}\x1b[0m\r\n").as_bytes());
        }
        let _ = update_tx.try_send(());
    });
}

/// One independent PTY terminal session.
/// - `clear_nonce`: when incremented, sends Ctrl+L to the shell.
/// - `cwd_out`: receives the working directory reported by OSC 7.
/// - `pty_writer_out`: if `Some`, receives this terminal's PTY writer so the
///   panel can write to the shell (Run in Terminal, AI command bar).
/// - `prompt_positions_out`: if `Some`, receives the absolute rows of the
///   prompts (OSC 133;A) still in the scrollback.
/// - `prompt_jump`: if `Some`, a `(nonce, direction)` request to scroll to the
///   previous (-1) or next (+1) prompt.
#[allow(clippy::too_many_arguments)]
fn single_terminal(
    theme: RwSignal<PhazeTheme>,
    clear_nonce: RwSignal<u64>,
    shell: String,
    cwd_out: RwSignal<String>,
    state: IdeState,
    term_font_size: RwSignal<u32>,
    find_open: RwSignal<bool>,
    find_query: RwSignal<String>,
    ai_cmd_open: RwSignal<bool>,
    blocks_open: RwSignal<bool>,
    pty_writer_out: Option<RwSignal<Option<SharedPtyWriter>>>,
    prompt_positions_out: Option<RwSignal<Vec<usize>>>,
    prompt_jump: Option<RwSignal<(u64, i32)>>,
) -> impl IntoView {
    let core: Arc<Mutex<TermCore>> = Arc::new(Mutex::new(TermCore::new(
        DEFAULT_PTY_COLS as usize,
        DEFAULT_PTY_ROWS as usize,
    )));
    let pty_writer: SharedPtyWriter = Arc::new(Mutex::new(None));
    let pty_master: SharedMaster = Arc::new(Mutex::new(None));

    if let Some(out_sig) = pty_writer_out {
        out_sig.set(Some(Arc::clone(&pty_writer)));
    }

    // Reader thread → UI: at most one pending wake-up at a time.
    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let update_signal = create_signal_from_channel(update_rx);
    spawn_pty(
        shell,
        Arc::clone(&core),
        Arc::clone(&pty_writer),
        Arc::clone(&pty_master),
        update_tx,
    );

    // Bumped whenever the grid should be repainted.
    let repaint: RwSignal<u64> = create_rw_signal(0);
    let bump = move || repaint.update(|v| *v = v.wrapping_add(1));
    let is_focused = create_rw_signal(false);
    let blocks_sig: RwSignal<Vec<BlockInfo>> = create_rw_signal(Vec::new());
    let find_hits: RwSignal<Vec<FindHit>> = create_rw_signal(Vec::new());
    let find_idx: RwSignal<Option<usize>> = create_rw_signal(None);
    let find_pending = create_rw_signal(false);

    let recompute_find = {
        let core = Arc::clone(&core);
        move || {
            let query = find_query.get_untracked();
            let hits = if find_open.get_untracked() && !query.is_empty() {
                core.lock().map(|c| c.find(&query)).unwrap_or_default()
            } else {
                Vec::new()
            };
            let idx = if hits.is_empty() {
                None
            } else {
                Some(
                    find_idx
                        .get_untracked()
                        .unwrap_or(hits.len() - 1)
                        .min(hits.len() - 1),
                )
            };
            find_hits.set(hits);
            find_idx.set(idx);
        }
    };

    // ── Reader updates → repaint, cwd, blocks, find ──────────────────────
    {
        let core = Arc::clone(&core);
        let last_blocks_version = create_rw_signal(u64::MAX);
        let recompute_find = recompute_find.clone();
        create_effect(move |_| {
            update_signal.get();
            bump();
            let Ok(c) = core.lock() else { return };
            if let Some(cwd) = c.cwd() {
                if cwd_out.get_untracked() != cwd {
                    cwd_out.set(cwd.to_string());
                }
            }
            let version = c.blocks_version();
            if version != last_blocks_version.get_untracked() {
                last_blocks_version.set(version);
                blocks_sig.set(
                    c.blocks()
                        .iter()
                        .filter(|b| b.finished)
                        .map(|b| BlockInfo {
                            id: b.id,
                            prompt_row: b.prompt_row,
                            exit_code: b.exit_code,
                            command: b.command.clone(),
                            output: b.output.clone(),
                        })
                        .collect(),
                );
                if let Some(pp) = prompt_positions_out {
                    pp.set(c.prompt_rows().into_iter().map(|r| r as usize).collect());
                }
            }
            drop(c);
            // Keep find results current while output arrives (debounced).
            if find_open.get_untracked()
                && !find_query.get_untracked().is_empty()
                && !find_pending.get_untracked()
            {
                find_pending.set(true);
                let recompute_find = recompute_find.clone();
                exec_after(Duration::from_millis(300), move |_| {
                    find_pending.set(false);
                    recompute_find();
                });
            }
        });
    }

    // Find query or bar visibility changed.
    {
        let recompute_find = recompute_find.clone();
        create_effect(move |_| {
            find_query.get();
            find_open.get();
            find_idx.set(None);
            recompute_find();
            bump();
        });
    }

    // ── Theme → emulator colours ─────────────────────────────────────────
    {
        let core = Arc::clone(&core);
        create_effect(move |_| {
            let palette = palette_from_theme(&theme.get().palette);
            if let Ok(mut c) = core.lock() {
                c.set_palette(palette);
            }
            bump();
        });
    }

    // ── Clear (Ctrl+L to the shell) ──────────────────────────────────────
    {
        let pty_w = Arc::clone(&pty_writer);
        create_effect(move |prev: Option<u64>| {
            let n = clear_nonce.get();
            if prev.is_some_and(|p| p != n) {
                write_shared(&pty_w, b"\x0c");
            }
            n
        });
    }

    // ── Previous/next prompt requests from the tab bar ───────────────────
    if let Some(jump) = prompt_jump {
        let core = Arc::clone(&core);
        create_effect(move |prev: Option<u64>| {
            let (n, dir) = jump.get();
            if prev.is_some_and(|p| p != n) {
                if let Ok(mut c) = core.lock() {
                    c.jump_prompt(dir);
                }
                bump();
            }
            n
        });
    }

    // ── Sizing ───────────────────────────────────────────────────────────
    let metrics = create_memo(move |_| measure_cell(term_font_size.get() as f32));
    let last_px: RwSignal<(f64, f64)> = create_rw_signal((0.0, 0.0));
    let apply_size = {
        let core = Arc::clone(&core);
        let master = Arc::clone(&pty_master);
        move |px: (f64, f64), m: CellMetrics| {
            let (cols, rows) = grid_size_for(px, m);
            if cols < MIN_COLS || rows < MIN_ROWS {
                return; // hidden (inactive tab or collapsed panel)
            }
            let changed = match core.lock() {
                Ok(mut c) => {
                    let before = c.size();
                    c.resize(cols, rows, (m.w.round() as u16, m.h.round() as u16));
                    before != c.size()
                }
                Err(_) => false,
            };
            if changed {
                if let Ok(guard) = master.lock() {
                    if let Some(ref master) = *guard {
                        let _ = master.resize(PtySize {
                            rows: rows as u16,
                            cols: cols as u16,
                            pixel_width: (cols as f64 * m.w) as u16,
                            pixel_height: (rows as f64 * m.h) as u16,
                        });
                    }
                }
                bump();
            }
        }
    };
    {
        let apply_size = apply_size.clone();
        create_effect(move |_| {
            let m = metrics.get();
            let px = last_px.get_untracked();
            if px.0 > 0.0 && px.1 > 0.0 {
                apply_size(px, m);
            }
        });
    }

    // ── Grid canvas ──────────────────────────────────────────────────────
    let layout_cache: Rc<RefCell<Vec<Option<RowLayouts>>>> = Rc::new(RefCell::new(Vec::new()));
    let grid_canvas = {
        let core = Arc::clone(&core);
        canvas(move |cx, size| {
            repaint.get();
            let t = theme.get();
            let m = metrics.get();
            let focused = is_focused.get();
            let open = find_open.get();
            let query = find_query.get();
            let current = find_idx.get().and_then(|i| find_hits.get().get(i).copied());
            let colors = PaintColors::from_palette(&t.palette);
            let (snap, history) = match core.lock() {
                Ok(c) => (
                    c.snapshot(
                        (open && !query.is_empty()).then_some(query.as_str()),
                        current,
                    ),
                    c.history_size(),
                ),
                Err(_) => return,
            };
            paint_grid(cx, size, snap, history, m, &colors, focused, &layout_cache);
        })
        .style(|s| s.size_full())
    };

    // ── Mouse ────────────────────────────────────────────────────────────
    let selecting = create_rw_signal(false);
    let mouse_down: RwSignal<Option<MouseCode>> = create_rw_signal(None);
    let last_mouse_cell: RwSignal<(usize, usize)> = create_rw_signal((usize::MAX, usize::MAX));
    let wheel_acc = create_rw_signal(0.0f64);

    // Viewport cell under a point, and which half of the cell it is in.
    let cell_at = {
        let core = Arc::clone(&core);
        move |pos: Point| -> (usize, usize, Side) {
            let m = metrics.get_untracked();
            let size = core.lock().map(|c| c.size()).unwrap_or(TermSize::new(2, 1));
            let x = (pos.x - GUTTER_W).max(0.0);
            let y = (pos.y - PAD_TOP).max(0.0);
            let col = ((x / m.w) as usize).min(size.cols - 1);
            let row = ((y / m.h) as usize).min(size.rows - 1);
            let side = if x - col as f64 * m.w > m.w / 2.0 {
                Side::Right
            } else {
                Side::Left
            };
            (row, col, side)
        }
    };
    let term_mode = {
        let core = Arc::clone(&core);
        move || core.lock().map(|c| c.mode()).unwrap_or(TermMode::empty())
    };
    let send_to_pty = {
        let w = Arc::clone(&pty_writer);
        move |data: &[u8]| write_shared(&w, data)
    };

    let output_area = container(grid_canvas);
    let area_id = output_area.id();
    let output_area = output_area
        .keyboard_navigable()
        .on_event_stop(EventListener::FocusGained, move |_| is_focused.set(true))
        .on_event_stop(EventListener::FocusLost, move |_| is_focused.set(false))
        .on_event_stop(EventListener::PointerDown, {
            let core = Arc::clone(&core);
            let cell_at = cell_at.clone();
            let term_mode = term_mode.clone();
            let send_to_pty = send_to_pty.clone();
            move |e| {
                let Event::PointerDown(pe) = e else { return };
                area_id.request_focus();
                let (row, col, side) = cell_at(pe.pos);
                let mods = mods_of(&pe.modifiers);
                let mode = term_mode();
                let code = match pe.button {
                    PointerButton::Mouse(MouseButton::Primary) => MouseCode::Left,
                    PointerButton::Mouse(MouseButton::Auxiliary) => MouseCode::Middle,
                    PointerButton::Mouse(MouseButton::Secondary) => MouseCode::Right,
                    _ => return,
                };
                if mode.intersects(TermMode::MOUSE_MODE) && !mods.shift {
                    mouse_down.set(Some(code));
                    last_mouse_cell.set((row, col));
                    send_to_pty(&mouse_report(code, true, false, col, row, mods, mode));
                    return;
                }
                if code != MouseCode::Left {
                    return;
                }
                let ty = match pe.count {
                    2 => SelectionType::Semantic,
                    n if n >= 3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                if let Ok(mut c) = core.lock() {
                    c.start_selection(ty, row, col, side);
                }
                selecting.set(true);
                bump();
            }
        })
        .on_event_stop(EventListener::PointerMove, {
            let core = Arc::clone(&core);
            let cell_at = cell_at.clone();
            let term_mode = term_mode.clone();
            let send_to_pty = send_to_pty.clone();
            move |e| {
                let Event::PointerMove(pe) = e else { return };
                let (row, col, side) = cell_at(pe.pos);
                if selecting.get_untracked() {
                    if let Ok(mut c) = core.lock() {
                        // Dragging past the top or bottom scrolls.
                        if pe.pos.y < 0.0 {
                            c.scroll_lines(1);
                        } else if pe.pos.y
                            > PAD_TOP + c.size().rows as f64 * metrics.get_untracked().h
                        {
                            c.scroll_lines(-1);
                        }
                        c.update_selection(row, col, side);
                    }
                    bump();
                    return;
                }
                let mode = term_mode();
                let mods = mods_of(&pe.modifiers);
                if !mode.intersects(TermMode::MOUSE_MODE) || mods.shift {
                    return;
                }
                if last_mouse_cell.get_untracked() == (row, col) {
                    return;
                }
                let held = mouse_down.get_untracked();
                let report = match held {
                    Some(code)
                        if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) =>
                    {
                        Some(code)
                    }
                    None if mode.contains(TermMode::MOUSE_MOTION) => Some(MouseCode::NoButton),
                    _ => None,
                };
                if let Some(code) = report {
                    last_mouse_cell.set((row, col));
                    send_to_pty(&mouse_report(code, true, true, col, row, mods, mode));
                }
            }
        })
        .on_event_stop(EventListener::PointerUp, {
            let core = Arc::clone(&core);
            let cell_at = cell_at.clone();
            let term_mode = term_mode.clone();
            let send_to_pty = send_to_pty.clone();
            move |e| {
                let Event::PointerUp(pe) = e else { return };
                if let Some(code) = mouse_down.get_untracked() {
                    mouse_down.set(None);
                    let (row, col, _) = cell_at(pe.pos);
                    let mode = term_mode();
                    if mode.intersects(TermMode::MOUSE_MODE) {
                        let mods = mods_of(&pe.modifiers);
                        send_to_pty(&mouse_report(code, false, false, col, row, mods, mode));
                    }
                    return;
                }
                if selecting.get_untracked() {
                    selecting.set(false);
                    if let Ok(mut c) = core.lock() {
                        if !c.has_selection() {
                            c.clear_selection();
                        }
                    }
                    bump();
                }
            }
        })
        .on_event_stop(EventListener::PointerWheel, {
            let core = Arc::clone(&core);
            let cell_at = cell_at.clone();
            let term_mode = term_mode.clone();
            let send_to_pty = send_to_pty.clone();
            move |e| {
                let Event::PointerWheel(we) = e else { return };
                let h = metrics.get_untracked().h;
                let acc = wheel_acc.get_untracked() + we.delta.y;
                let lines = (acc / h).trunc() as i32;
                wheel_acc.set(acc - lines as f64 * h);
                if lines == 0 {
                    return;
                }
                let mode = term_mode();
                let mods = mods_of(&we.modifiers);
                let down = lines > 0;
                let count = lines.unsigned_abs().min(10) as usize;
                if mode.intersects(TermMode::MOUSE_MODE) && !mods.shift {
                    let (row, col, _) = cell_at(we.pos);
                    let code = if down {
                        MouseCode::WheelDown
                    } else {
                        MouseCode::WheelUp
                    };
                    let one = mouse_report(code, true, false, col, row, mods, mode);
                    send_to_pty(&one.repeat(count));
                } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
                    && !mods.shift
                {
                    // Programs on the alternate screen (less, man) scroll
                    // with the arrow keys.
                    let key = if down { TermKey::Down } else { TermKey::Up };
                    let one = key_bytes(key, Mods::default(), mode.contains(TermMode::APP_CURSOR));
                    send_to_pty(&one.repeat(count));
                } else {
                    if let Ok(mut c) = core.lock() {
                        c.scroll_lines(-lines);
                    }
                    bump();
                }
            }
        })
        .on_event_stop(EventListener::KeyDown, {
            let core = Arc::clone(&core);
            let state = state.clone();
            let term_mode = term_mode.clone();
            let send_to_pty = send_to_pty.clone();
            move |event| {
                let Event::KeyDown(e) = event else { return };
                let ctrl = e.modifiers.contains(Modifiers::CONTROL);
                let shift = e.modifiers.contains(Modifiers::SHIFT);

                // Global shortcuts route through the same execute_command as the root
                // key handler in app.rs — single dispatch, no drift.
                if let Some(cmd) = match_global_shortcut(&e.key.logical_key, &e.modifiers) {
                    execute_command(&cmd, &state);
                    return;
                }

                if ctrl && !shift {
                    if let Key::Character(ref ch) = e.key.logical_key {
                        match ch.as_str() {
                            "f" | "F" => {
                                find_open.update(|v| *v = !*v);
                                return;
                            }
                            "k" | "K" => {
                                ai_cmd_open.update(|v| *v = !*v);
                                return;
                            }
                            _ => {}
                        }
                    }
                }

                if ctrl && shift {
                    if let Key::Character(ref ch) = e.key.logical_key {
                        match ch.as_str() {
                            // Paste (bracketed when the program asked for it).
                            "v" | "V" => {
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    if let Ok(text) = clipboard.get_text() {
                                        let bracketed =
                                            term_mode().contains(TermMode::BRACKETED_PASTE);
                                        send_to_pty(&paste_bytes(&text, bracketed));
                                        if let Ok(mut c) = core.lock() {
                                            c.scroll_to_bottom();
                                        }
                                        bump();
                                    }
                                }
                                return;
                            }
                            // Copy the selection, or everything when nothing
                            // is selected.
                            "c" | "C" => {
                                let text = core
                                    .lock()
                                    .ok()
                                    .map(|c| c.selection_text().unwrap_or_else(|| c.all_text()));
                                if let Some(text) = text {
                                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                        let _ = clipboard.set_text(text);
                                    }
                                }
                                return;
                            }
                            "=" | "+" => {
                                term_font_size.update(|v| *v = (*v + 1).min(32));
                                return;
                            }
                            "-" | "_" => {
                                term_font_size.update(|v| *v = (*v).saturating_sub(1).max(8));
                                return;
                            }
                            _ => {}
                        }
                    }
                }

                // Shift+PageUp/PageDown scroll the view instead of the program.
                if shift && !ctrl {
                    if let Key::Named(named @ (NamedKey::PageUp | NamedKey::PageDown)) =
                        e.key.logical_key
                    {
                        if let Ok(mut c) = core.lock() {
                            c.scroll_page(named == NamedKey::PageUp);
                        }
                        bump();
                        return;
                    }
                }

                let app_cursor = term_mode().contains(TermMode::APP_CURSOR);
                let bytes = key_to_pty_bytes(e, app_cursor);
                if !bytes.is_empty() {
                    send_to_pty(&bytes);
                    if let Ok(mut c) = core.lock() {
                        c.scroll_to_bottom();
                    }
                    bump();
                }
            }
        })
        .on_resize(move |rect| {
            // The canvas sits inside a 1px border on each side.
            let px = (rect.width() - 2.0, rect.height() - 2.0);
            last_px.set(px);
            apply_size(px, metrics.get_untracked());
        })
        .style(move |s| {
            let p = theme.get().palette;
            // Subtle accent border when focused so the user knows keys go to the PTY.
            let border = if is_focused.get() {
                p.accent.with_alpha(0.35)
            } else {
                p.bg_base
            };
            s.flex_grow(1.0)
                .min_height(0.0)
                .min_width(0.0)
                .height_full()
                .border(1.0)
                .border_color(border)
                .cursor(CursorStyle::Text)
        });

    // ── Find bar ─────────────────────────────────────────────────────────
    let find_step = {
        let core = Arc::clone(&core);
        move |dir: i32| {
            let hits = find_hits.get_untracked();
            if hits.is_empty() {
                return;
            }
            let n = hits.len();
            let idx = match find_idx.get_untracked() {
                Some(i) if dir < 0 => (i + n - 1) % n,
                Some(i) => (i + 1) % n,
                None => n - 1,
            };
            find_idx.set(Some(idx));
            if let Ok(mut c) = core.lock() {
                let rows = c.size().rows as u64;
                c.scroll_to_abs(hits[idx].row.saturating_sub(rows / 2));
            }
            bump();
        }
    };
    let find_btn = |text: &'static str| {
        container(label(move || text)).style(move |s| {
            let p = theme.get().palette;
            s.font_size(12.0)
                .color(p.text_muted)
                .padding_horiz(6.0)
                .padding_vert(3.0)
                .border_radius(3.0)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(p.bg_elevated).color(p.text_primary))
        })
    };
    let find_bar = container(
        stack((
            text_input(find_query)
                .placeholder("Find in terminal")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(12.0)
                        .width(200.0)
                        .padding(4.0)
                        .background(p.bg_elevated)
                        .color(p.text_primary)
                        .border(1.0)
                        .border_color(p.accent)
                        .border_radius(3.0)
                })
                .on_event_cont(EventListener::KeyDown, {
                    let find_step = find_step.clone();
                    move |e| {
                        if let Event::KeyDown(k) = e {
                            match k.key.logical_key {
                                Key::Named(NamedKey::Enter) => {
                                    // Enter searches upward (older output).
                                    let dir = if k.modifiers.contains(Modifiers::SHIFT) {
                                        1
                                    } else {
                                        -1
                                    };
                                    find_step(dir);
                                }
                                Key::Named(NamedKey::Escape) => find_open.set(false),
                                _ => {}
                            }
                        }
                    }
                }),
            label(move || {
                let n = find_hits.get().len();
                match find_idx.get() {
                    Some(i) if n > 0 => format!("  {} of {}", i + 1, n),
                    _ => format!("  {} result{}", n, if n == 1 { "" } else { "s" }),
                }
            })
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0).color(p.text_muted).padding_horiz(8.0)
            }),
            find_btn("↑").on_click_stop({
                let find_step = find_step.clone();
                move |_| find_step(-1)
            }),
            find_btn("↓").on_click_stop(move |_| find_step(1)),
            find_btn("✕").on_click_stop(move |_| find_open.set(false)),
        ))
        .style(|s| {
            s.flex_row()
                .items_center()
                .padding_horiz(8.0)
                .padding_vert(4.0)
        }),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full()
            .background(p.bg_panel)
            .border_bottom(1.0)
            .border_color(p.border)
            .apply_if(!find_open.get(), |s| s.display(Display::None))
    });

    // ── Command blocks (one card per finished shell command) ─────────────
    let blocks_list = {
        let state_b = state.clone();
        let core_b = Arc::clone(&core);
        dyn_stack(
            move || blocks_sig.get(),
            |b: &BlockInfo| b.id,
            move |b: BlockInfo| {
                let collapsed = create_rw_signal(true);
                let hovered = create_rw_signal(false);
                let exit_code = b.exit_code;
                let has_output = !b.output.is_empty();

                // ── Exit badge ──────────────────────────────────────────
                let badge = container(label(move || match exit_code {
                    Some(0) => " ✓ ".to_string(),
                    Some(code) => format!(" ✗ {code} "),
                    None => " • ".to_string(),
                }))
                .style(move |s| {
                    let p = theme.get().palette;
                    let (fg, bg) = match exit_code {
                        Some(0) => (
                            Color::from_rgb8(80, 200, 80),
                            Color::from_rgba8(80, 200, 80, 35),
                        ),
                        Some(_) => (p.error, p.error.with_alpha(0.15)),
                        None => (p.text_muted, p.bg_elevated),
                    };
                    s.font_size(10.0)
                        .font_weight(Weight::BOLD)
                        .color(fg)
                        .padding_horiz(4.0)
                        .padding_vert(1.0)
                        .border_radius(3.0)
                        .margin_right(6.0)
                        .background(bg)
                });

                // ── Header (prompt + command); click jumps to it ────────
                let command = b.command.clone();
                let prompt_row = b.prompt_row;
                let core_h = Arc::clone(&core_b);
                let header_text = label(move || command.clone())
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(12.0)
                            .font_family("monospace".to_string())
                            .color(p.text_secondary)
                            .flex_grow(1.0)
                            .min_width(0.0)
                            .text_ellipsis()
                            .cursor(CursorStyle::Pointer)
                    })
                    .on_click_stop(move |_| {
                        if let Ok(mut c) = core_h.lock() {
                            c.scroll_to_abs(prompt_row);
                        }
                        bump();
                    });

                // ── Collapse toggle ─────────────────────────────────────
                let toggle = container(label(move || if collapsed.get() { " ⌄ " } else { " ⌃ " }))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(10.0)
                            .color(if has_output {
                                p.text_muted
                            } else {
                                p.text_disabled
                            })
                            .padding_horiz(4.0)
                            .border_radius(3.0)
                            .cursor(if has_output {
                                CursorStyle::Pointer
                            } else {
                                CursorStyle::Default
                            })
                            .hover(|s| s.color(p.accent))
                    })
                    .on_click_stop(move |_| {
                        if has_output {
                            collapsed.update(|v| *v = !*v);
                        }
                    });

                // ── AI explain button ───────────────────────────────────
                let state_ai = state_b.clone();
                let ai_text: String = {
                    let mut s = b.command.clone();
                    s.push('\n');
                    s.push_str(&b.output);
                    let mut cut = s.len().min(4000);
                    while !s.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    s.truncate(cut);
                    s
                };
                let ai_btn = container(label(|| " ✦ "))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(10.0)
                            .color(p.text_muted)
                            .padding_horiz(4.0)
                            .border_radius(3.0)
                            .cursor(CursorStyle::Pointer)
                            .hover(|s| s.color(p.accent))
                    })
                    .on_click_stop(move |_| {
                        let prompt =
                            format!("Explain this terminal output:\n\n```\n{}\n```", ai_text);
                        state_ai.ai.pending_chat_inject.set(Some(prompt));
                        state_ai.workbench.show_right_panel.set(true);
                    });

                let header_row = container(
                    stack((badge, header_text, toggle, ai_btn))
                        .style(|s| s.flex_row().items_center().width_full()),
                )
                .style(move |s| {
                    let p = theme.get().palette;
                    s.width_full()
                        .min_height(26.0)
                        .padding_horiz(4.0)
                        .padding_vert(2.0)
                        .background(if hovered.get() {
                            p.bg_elevated
                        } else {
                            Color::TRANSPARENT
                        })
                })
                .on_event_stop(EventListener::PointerEnter, move |_| hovered.set(true))
                .on_event_stop(EventListener::PointerLeave, move |_| hovered.set(false));

                // ── Output (collapsible) ────────────────────────────────
                let output = b.output.clone();
                let output_view = label(move || output.clone()).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(11.0)
                        .font_family("monospace".to_string())
                        .color(p.text_primary)
                        .padding_horiz(8.0)
                        .padding_vert(2.0)
                        .width_full()
                        .apply_if(collapsed.get(), |s| s.display(Display::None))
                });

                container(stack((header_row, output_view)).style(|s| s.flex_col().width_full()))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.width_full()
                            .border_bottom(1.0)
                            .border_color(p.border.with_alpha(0.25))
                    })
            },
        )
        .style(|s| s.flex_col().width_full())
    };
    let blocks_panel = stack((
        label(|| "Commands").style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .font_weight(Weight::BOLD)
                .color(p.text_muted)
                .padding_horiz(8.0)
                .padding_vert(4.0)
        }),
        label(|| "No finished commands yet. Blocks come from shell integration (bash).").style(
            move |s| {
                let p = theme.get().palette;
                s.font_size(11.0)
                    .color(p.text_disabled)
                    .padding_horiz(8.0)
                    .apply_if(!blocks_sig.get().is_empty(), |s| s.display(Display::None))
            },
        ),
        scroll(blocks_list).style(|s| s.flex_grow(1.0).min_height(0.0).width_full()),
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width(300.0)
            .height_full()
            .background(p.bg_panel)
            .border_left(1.0)
            .border_color(p.border)
            .apply_if(!blocks_open.get(), |s| s.display(Display::None))
    });

    let body = stack((output_area, blocks_panel))
        .style(|s| s.flex_row().flex_grow(1.0).min_height(0.0).width_full());

    stack((find_bar, body)).style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .flex_grow(1.0)
            .min_height(0.0)
            .width_full()
            .background(p.bg_base)
    })
}

// ── Multi-tab terminal panel ───────────────────────────────────────────────────

/// One terminal tab and the signals the panel shares with its session.
#[derive(Clone)]
struct TermTab {
    id: usize,
    /// Tab title (renamable).
    name: RwSignal<String>,
    /// Bumped by the Clear button.
    clear_nonce: RwSignal<u64>,
    shell: String,
    /// Working directory from OSC 7, shown in the title.
    cwd: RwSignal<String>,
    /// The session's PTY writer once the shell is running.
    pty_writer: RwSignal<Option<SharedPtyWriter>>,
    /// Prompt rows still in the scrollback.
    prompt_positions: RwSignal<Vec<usize>>,
    /// `(nonce, direction)` request to scroll to the previous/next prompt.
    prompt_jump: RwSignal<(u64, i32)>,
}

impl TermTab {
    fn new(id: usize, shell: &str) -> Self {
        Self {
            id,
            name: create_rw_signal(shell.to_string()),
            clear_nonce: create_rw_signal(0),
            shell: shell.to_string(),
            cwd: create_rw_signal(String::new()),
            pty_writer: create_rw_signal(None),
            prompt_positions: create_rw_signal(Vec::new()),
            prompt_jump: create_rw_signal((0, 0)),
        }
    }
}

/// Real PTY terminal panel with multiple tab support.
///
/// A "+" button spawns a new shell session. Each session is independent —
/// closing a tab kills that PTY (the OS will reap it) and removes the tab.
/// Tab names can be edited by double-clicking the tab label.
///
/// `run_in_terminal_text`: when set to `Some(text)` from outside (e.g. editor
/// right-click → "Run in Terminal"), writes `text\n` to the active PTY and
/// resets the signal to `None`.
pub fn terminal_panel(
    state: IdeState,
    run_in_terminal_text: RwSignal<Option<String>>,
) -> impl IntoView {
    let theme = state.workbench.theme;
    // Shell selector index (cycles through SHELLS)
    let shell_idx: RwSignal<usize> = create_rw_signal(0usize);

    // Terminal zoom — independent font size, clamped 8..32
    let term_font_size: RwSignal<u32> = create_rw_signal(13u32);

    // Terminal split — show two PTY panes side by side
    let term_split: RwSignal<bool> = create_rw_signal(false);

    // Terminal find — find bar state
    let term_find_open: RwSignal<bool> = create_rw_signal(false);
    let term_find_query: RwSignal<String> = create_rw_signal(String::new());

    // AI command bar — Warp-style natural-language → shell command
    let ai_cmd_open: RwSignal<bool> = create_rw_signal(false);
    let ai_cmd_query: RwSignal<String> = create_rw_signal(String::new());
    let ai_cmd_thinking: RwSignal<bool> = create_rw_signal(false);
    let (ai_cmd_tx, ai_cmd_rx) = std::sync::mpsc::sync_channel::<Result<String, String>>(4);
    let ai_cmd_result = create_signal_from_channel(ai_cmd_rx);

    // Feature 3: active terminal's PTY writer — set by single_terminal once PTY is ready.
    // Writing bytes to this sends them directly to the active shell.
    let active_pty_writer: RwSignal<Option<SharedPtyWriter>> = create_rw_signal(None);

    // Feature 2: prompt rows (OSC 133;A) in the active terminal, for the
    // prev/next command buttons.
    let prompt_positions: RwSignal<Vec<usize>> = create_rw_signal(Vec::new());

    // Command-block list (finished commands with exit badges) beside each terminal.
    let blocks_open: RwSignal<bool> = create_rw_signal(false);

    // Feature 3: watch run_in_terminal_text — write to active PTY when set.
    {
        let writer_sig = active_pty_writer;
        create_effect(move |_| {
            if let Some(text) = run_in_terminal_text.get() {
                if let Some(ref writer_arc) = writer_sig.get_untracked() {
                    if let Ok(mut guard) = writer_arc.lock() {
                        if let Some(ref mut w) = *guard {
                            let mut payload = text.into_bytes();
                            payload.push(b'\n');
                            let _ = w.write_all(&payload);
                            let _ = w.flush();
                        }
                    }
                }
                run_in_terminal_text.set(None);
            }
        });
    }

    // AI command bar: when AI returns a command, write it to the active PTY (no newline).
    {
        let writer = active_pty_writer;
        let status_toast = state.workbench.status_toast;
        create_effect(move |_| {
            let Some(res) = ai_cmd_result.get() else {
                return;
            };
            ai_cmd_thinking.set(false);
            match res {
                Ok(cmd) => {
                    let cmd = cmd.trim().trim_matches('`').to_string();
                    if let Some(ref arc) = writer.get_untracked() {
                        if let Ok(mut guard) = arc.lock() {
                            if let Some(ref mut w) = *guard {
                                let _ = w.write_all(cmd.as_bytes());
                                let _ = w.flush();
                            }
                        }
                    }
                    ai_cmd_open.set(false);
                    ai_cmd_query.set(String::new());
                }
                Err(e) => {
                    tracing::warn!(target: "phazeai_ui", "AI cmd error: {e}");
                    crate::app::show_toast(status_toast, format!("AI command: {e}"));
                    ai_cmd_open.set(false);
                }
            }
        });
    }

    let tab_data: RwSignal<Vec<TermTab>> = create_rw_signal(vec![TermTab::new(1, "bash")]);
    let active_tab: RwSignal<usize> = create_rw_signal(1);
    let next_id: RwSignal<usize> = create_rw_signal(2);

    // Command palette "New Terminal Tab" — bump new_terminal_nonce to open a fresh tab.
    {
        let nonce = state.workbench.new_terminal_nonce;
        create_effect(move |prev: Option<u64>| {
            let cur = nonce.get();
            if prev.is_some() && prev != Some(cur) {
                let id = next_id.get_untracked();
                next_id.set(id + 1);
                let shell_name = SHELLS[shell_idx.get_untracked() % SHELLS.len()].to_string();
                tab_data.update(|data| data.push(TermTab::new(id, &shell_name)));
                active_tab.set(id);
            }
            cur
        });
    }

    // Keep active_pty_writer and prompt_positions in sync with the active tab's signals.
    {
        create_effect(move |_| {
            let id = active_tab.get();
            let data = tab_data.get();
            if let Some(tab) = data.iter().find(|t| t.id == id) {
                active_pty_writer.set(tab.pty_writer.get());
                prompt_positions.set(tab.prompt_positions.get());
            }
        });
    }
    // Which tab is currently being renamed (None = none)
    let editing_tab: RwSignal<Option<usize>> = create_rw_signal(None);
    let rename_text: RwSignal<String> = create_rw_signal(String::new());

    // Ask the active terminal to scroll to its previous (-1) / next (+1) prompt.
    let jump_prompt = move |dir: i32| {
        let id = active_tab.get_untracked();
        if let Some(tab) = tab_data.get_untracked().into_iter().find(|t| t.id == id) {
            tab.prompt_jump.update(|(n, d)| {
                *n += 1;
                *d = dir;
            });
        }
    };

    // ── Tab bar ───────────────────────────────────────────────────────────
    let tab_bar = stack((
        // Scrollable row of tabs
        dyn_stack(
            move || {
                tab_data
                    .get()
                    .into_iter()
                    .map(|t| (t.id, t.name, t.cwd))
                    .collect::<Vec<_>>()
            },
            |(id, _, _)| *id,
            move |(id, name_sig, cwd_sig)| {
                let is_active = move || active_tab.get() == id;
                let hovered = create_rw_signal(false);

                // Label shown when not renaming (shows cwd basename when available)
                let title = label(move || {
                    let name = safe_get(name_sig, String::from("Terminal"));
                    let cwd = safe_get(cwd_sig, String::new());
                    if cwd.is_empty() {
                        name
                    } else {
                        let basename = cwd.rsplit('/').next().unwrap_or(&cwd).to_string();
                        format!("{} [{}]", name, basename)
                    }
                })
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    let active = is_active();
                    let hov = hovered.get();
                    let editing = editing_tab.get() == Some(id);
                    s.font_size(11.0)
                        .color(if active {
                            p.text_primary
                        } else if hov {
                            p.text_secondary
                        } else {
                            p.text_muted
                        })
                        .padding_horiz(10.0)
                        .padding_vert(5.0)
                        .border_bottom(if active { 2.0 } else { 0.0 })
                        .border_color(p.accent)
                        .cursor(CursorStyle::Pointer)
                        .apply_if(editing, |s| s.display(Display::None))
                })
                .on_click_stop(move |_| {
                    if active_tab.get_untracked() == id {
                        // Second click on already-active tab → start rename
                        editing_tab.set(Some(id));
                        rename_text.set(name_sig.get_untracked());
                    } else {
                        active_tab.set(id);
                    }
                });

                // Inline rename input — shown when editing_tab == Some(id)
                let rename_input = container(
                    floem::views::text_input(rename_text)
                        .style(move |s| {
                            let t = theme.get();
                            let p = &t.palette;
                            s.font_size(11.0)
                                .width(80.0)
                                .padding(2.0)
                                .background(p.bg_elevated)
                                .color(p.text_primary)
                                .border(1.0)
                                .border_color(p.accent)
                                .border_radius(3.0)
                        })
                        .on_event_stop(EventListener::KeyDown, move |event| {
                            if let floem::event::Event::KeyDown(e) = event {
                                use floem::keyboard::NamedKey;
                                match e.key.logical_key {
                                    Key::Named(NamedKey::Enter) => {
                                        // Commit rename
                                        let new_name = rename_text.get_untracked();
                                        if !new_name.is_empty() {
                                            name_sig.set(new_name);
                                        }
                                        editing_tab.set(None);
                                    }
                                    Key::Named(NamedKey::Escape) => {
                                        editing_tab.set(None);
                                    }
                                    _ => {}
                                }
                            }
                        }),
                )
                .style(move |s| {
                    let visible = editing_tab.get() == Some(id);
                    s.apply_if(!visible, |s| s.display(Display::None))
                });

                // × close button — only show when hovered and there is more than 1 tab
                let close_btn = container(label(|| "×"))
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        let show = hovered.get()
                            && tab_data.get().len() > 1
                            && editing_tab.get() != Some(id);
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
                        tab_data.update(|data| data.retain(|t| t.id != id));
                        if active_tab.get_untracked() == id {
                            if let Some(last) = tab_data.get_untracked().last() {
                                active_tab.set(last.id);
                            }
                        }
                        if editing_tab.get_untracked() == Some(id) {
                            editing_tab.set(None);
                        }
                    });

                container(stack((title, rename_input, close_btn)).style(|s| s.items_center()))
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
        .style(|s| s.flex_row().flex_grow(1.0)),
        // Feature 2: "⬆" prev command button — jump to previous OSC 133;A prompt marker
        container(label(|| "\u{2B06}"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let has_markers = !prompt_positions.get().is_empty();
                s.padding_horiz(6.0)
                    .padding_vert(5.0)
                    .font_size(11.0)
                    .color(if has_markers {
                        p.text_muted
                    } else {
                        p.text_muted.with_alpha(0.35)
                    })
                    .cursor(CursorStyle::Pointer)
                    .border(1.0)
                    .border_color(p.border)
                    .border_radius(3.0)
                    .margin_right(2.0)
                    .hover(|s| s.color(p.accent))
            })
            .on_click_stop(move |_| jump_prompt(-1)),
        // Feature 2: "⬇" next command button — jump to next OSC 133;A prompt marker
        container(label(|| "\u{2B07}"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let has_markers = !prompt_positions.get().is_empty();
                s.padding_horiz(6.0)
                    .padding_vert(5.0)
                    .font_size(11.0)
                    .color(if has_markers {
                        p.text_muted
                    } else {
                        p.text_muted.with_alpha(0.35)
                    })
                    .cursor(CursorStyle::Pointer)
                    .border(1.0)
                    .border_color(p.border)
                    .border_radius(3.0)
                    .margin_right(4.0)
                    .hover(|s| s.color(p.accent))
            })
            .on_click_stop(move |_| jump_prompt(1)),
        // "A-" zoom out button
        container(label(|| "A-"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(6.0)
                    .padding_vert(5.0)
                    .font_size(11.0)
                    .color(p.text_muted)
                    .cursor(CursorStyle::Pointer)
                    .border(1.0)
                    .border_color(p.border)
                    .border_radius(3.0)
                    .margin_right(2.0)
                    .hover(|s| s.color(p.accent))
            })
            .on_click_stop(move |_| {
                term_font_size.update(|v| *v = (*v).saturating_sub(1).max(8));
            }),
        // "A0" reset zoom button
        container(label(|| "A0"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(6.0)
                    .padding_vert(5.0)
                    .font_size(11.0)
                    .color(p.text_muted)
                    .cursor(CursorStyle::Pointer)
                    .border(1.0)
                    .border_color(p.border)
                    .border_radius(3.0)
                    .margin_right(2.0)
                    .hover(|s| s.color(p.accent))
            })
            .on_click_stop(move |_| {
                term_font_size.set(13u32);
            }),
        // "A+" zoom in button
        container(label(|| "A+"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(6.0)
                    .padding_vert(5.0)
                    .font_size(11.0)
                    .color(p.text_muted)
                    .cursor(CursorStyle::Pointer)
                    .border(1.0)
                    .border_color(p.border)
                    .border_radius(3.0)
                    .margin_right(4.0)
                    .hover(|s| s.color(p.accent))
            })
            .on_click_stop(move |_| {
                term_font_size.update(|v| *v = (*v + 1).min(32));
            }),
        // "✦" AI command button — opens natural-language → shell command bar (Ctrl+K)
        container(label(
            move || if ai_cmd_thinking.get() { "⏳" } else { "✦" },
        ))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let active = ai_cmd_open.get();
            s.padding_horiz(8.0)
                .padding_vert(5.0)
                .font_size(13.0)
                .color(if active { p.accent } else { p.text_muted })
                .cursor(CursorStyle::Pointer)
                .border(1.0)
                .border_color(if active { p.accent } else { p.border })
                .border_radius(3.0)
                .margin_right(4.0)
                .hover(|s| s.color(p.accent))
        })
        .on_click_stop(move |_| ai_cmd_open.update(|v| *v = !*v)),
        // "▤" command-block list toggle (finished commands, exit badges, AI explain)
        container(label(move || {
            let n = prompt_positions.get().len();
            if n == 0 {
                "▤".to_string()
            } else {
                format!("▤ {n}")
            }
        }))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let active = blocks_open.get();
            s.padding_horiz(8.0)
                .padding_vert(5.0)
                .font_size(12.0)
                .color(if active { p.accent } else { p.text_muted })
                .cursor(CursorStyle::Pointer)
                .border(1.0)
                .border_color(if active { p.accent } else { p.border })
                .border_radius(3.0)
                .margin_right(4.0)
                .hover(|s| s.color(p.accent))
        })
        .on_click_stop(move |_| blocks_open.update(|v| *v = !*v)),
        // "⊟" split button — toggle side-by-side split
        container(label(|| "⊟"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let active = term_split.get();
                s.padding_horiz(8.0)
                    .padding_vert(5.0)
                    .font_size(13.0)
                    .color(if active { p.accent } else { p.text_muted })
                    .cursor(CursorStyle::Pointer)
                    .border(1.0)
                    .border_color(if active { p.accent } else { p.border })
                    .border_radius(3.0)
                    .margin_right(4.0)
                    .hover(|s| s.color(p.accent))
            })
            .on_click_stop(move |_| term_split.update(|v| *v = !*v)),
        // "Clear" button — sends Ctrl+L to active terminal
        container(label(|| "⌫"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(8.0)
                    .padding_vert(5.0)
                    .font_size(13.0)
                    .color(p.text_muted)
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| s.background(p.bg_elevated).color(p.text_primary))
            })
            .on_click_stop(move |_| {
                let id = active_tab.get_untracked();
                if let Some(tab) = tab_data.get_untracked().into_iter().find(|t| t.id == id) {
                    tab.clear_nonce.update(|v| *v += 1);
                }
            }),
        // Shell selector button (cycles through SHELLS)
        container(label(move || {
            SHELLS[shell_idx.get() % SHELLS.len()].to_string()
        }))
        .style(move |s| {
            let p = theme.get().palette;
            s.padding_horiz(8.0)
                .padding_vert(5.0)
                .font_size(11.0)
                .color(p.text_muted)
                .cursor(CursorStyle::Pointer)
                .border(1.0)
                .border_color(p.border)
                .border_radius(3.0)
                .margin_right(4.0)
                .hover(|s| s.color(p.accent))
        })
        .on_click_stop(move |_| shell_idx.update(|i| *i = (*i + 1) % SHELLS.len())),
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
                let id = next_id.get_untracked();
                next_id.set(id + 1);
                let shell_name = SHELLS[shell_idx.get_untracked() % SHELLS.len()].to_string();
                tab_data.update(|data| data.push(TermTab::new(id, &shell_name)));
                active_tab.set(id);
            }),
    ))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.flex_row()
            .items_center()
            .border_bottom(1.0)
            .border_color(p.border)
            .background(p.bg_panel)
            .width_full()
            .min_height(30.0)
    });

    // ── Split pane: independent PTY with fixed ID 999 ────────────────────
    let split_clear: RwSignal<u64> = create_rw_signal(0u64);
    let split_cwd: RwSignal<String> = create_rw_signal(String::new());
    // split_term_view is created once and hidden/shown based on term_split.
    // Split pane doesn't participate in run-in-terminal or command marker tracking.
    let split_term_view = single_terminal(
        theme,
        split_clear,
        "bash".to_string(),
        split_cwd,
        state.clone(),
        term_font_size,
        term_find_open,
        term_find_query,
        ai_cmd_open,
        blocks_open,
        None,
        None,
        None,
    );

    // ── Terminal instances (one per tab, hidden when not active) ──────────
    let instances = dyn_stack(
        move || safe_get(tab_data, Vec::new()),
        |t: &TermTab| t.id,
        move |t: TermTab| {
            let id = t.id;
            single_terminal(
                theme,
                t.clear_nonce,
                t.shell,
                t.cwd,
                state.clone(),
                term_font_size,
                term_find_open,
                term_find_query,
                ai_cmd_open,
                blocks_open,
                Some(t.pty_writer),
                Some(t.prompt_positions),
                Some(t.prompt_jump),
            )
            .style(move |s| {
                s.size_full()
                    .apply_if(active_tab.get() != id, |s| s.display(Display::None))
            })
        },
    )
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    // ── Content area: normal or split ─────────────────────────────────────
    // When split is active we show instances + divider + split_term side by side.
    let content_area = stack((
        // Normal terminal instances (always present, manages visibility per tab)
        container(instances).style(move |s| s.flex_grow(1.0).min_width(0.0).min_height(0.0)),
        // Vertical divider (only shown when split)
        container(empty()).style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.width(3.0)
                .height_full()
                .background(p.border)
                .apply_if(!term_split.get(), |s| s.display(Display::None))
        }),
        // Split pane terminal (only shown when split)
        container(split_term_view).style(move |s| {
            s.flex_grow(1.0)
                .min_width(0.0)
                .min_height(0.0)
                .apply_if(!term_split.get(), |s| s.display(Display::None))
        }),
    ))
    .style(|s| s.flex_row().flex_grow(1.0).min_height(0.0).width_full());

    // ── AI command bar (Ctrl+K) ─────────────────────────────────────────────
    // Shown between tab_bar and content_area when ai_cmd_open is true.
    let ai_cmd_tx_bar = ai_cmd_tx.clone();
    let ai_cmd_bar = {
        let input = text_input(ai_cmd_query)
            .placeholder("Describe a command… e.g. \"find all Rust files changed today\"")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.flex_grow(1.0)
                    .min_width(0.0)
                    .font_size(13.0)
                    .padding_horiz(10.0)
                    .padding_vert(5.0)
                    .color(p.text_primary)
                    .background(p.bg_elevated)
                    .border(1.0)
                    .border_color(p.accent)
                    .border_radius(4.0)
            })
            .on_event_stop(EventListener::KeyDown, move |ev| {
                if let Event::KeyDown(e) = ev {
                    use floem::keyboard::NamedKey;
                    match &e.key.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            ai_cmd_open.set(false);
                            ai_cmd_query.set(String::new());
                        }
                        Key::Named(NamedKey::Enter) => {
                            let desc = ai_cmd_query.get();
                            let desc = desc.trim().to_string();
                            if desc.is_empty() || ai_cmd_thinking.get() {
                                return;
                            }
                            ai_cmd_thinking.set(true);
                            let tx = ai_cmd_tx_bar.clone();
                            std::thread::spawn(move || {
                                let settings = Settings::load();
                                let client = match settings.build_small_llm_client() {
                                    Ok(c) => c,
                                    Err(e) => { let _ = tx.send(Err(format!("LLM: {e}"))); return; }
                                };
                                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                                    Ok(r) => r,
                                    Err(e) => { let _ = tx.send(Err(format!("Runtime: {e}"))); return; }
                                };
                                rt.block_on(async move {
                                    let prompt = format!(
                                        "Generate a single bash/shell command that does the following: {desc}\n\
                                         Rules: respond with ONLY the command, no explanation, no markdown, no backticks."
                                    );
                                    let _ = tx.send(
                                        crate::ai_helpers::terminal_command_reply(client, &prompt).await,
                                    );
                                });
                            });
                        }
                        _ => {}
                    }
                }
            });

        let hint = label(move || {
            if ai_cmd_thinking.get() {
                "Generating…".to_string()
            } else {
                "Enter to generate · Esc to close".to_string()
            }
        })
        .style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0).color(p.text_muted).padding_horiz(10.0)
        });

        let close_btn = container(label(|| "✕"))
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(12.0)
                    .color(p.text_muted)
                    .padding_horiz(8.0)
                    .padding_vert(4.0)
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| s.color(p.text_primary))
            })
            .on_click_stop(move |_| {
                ai_cmd_open.set(false);
                ai_cmd_query.set(String::new());
            });

        container(
            stack((
                label(|| "✦ AI Command").style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(11.0)
                        .font_weight(floem::text::Weight::BOLD)
                        .color(p.accent)
                        .margin_right(8.0)
                }),
                input,
                hint,
                close_btn,
            ))
            .style(|s| s.flex_row().items_center().gap(4.0)),
        )
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.width_full()
                .padding_horiz(10.0)
                .padding_vert(6.0)
                .background(p.bg_elevated)
                .border_bottom(1.0)
                .border_color(p.accent.with_alpha(0.4))
                .apply_if(!ai_cmd_open.get(), |s| s.display(Display::None))
        })
    };

    stack((tab_bar, ai_cmd_bar, content_area)).style(move |s| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_size_leaves_room_for_the_gutter() {
        let m = CellMetrics {
            w: 8.0,
            h: 16.0,
            font_size: 13.0,
        };
        let (cols, rows) = grid_size_for(
            (GUTTER_W + PAD_RIGHT + 80.0 * 8.0, PAD_TOP + 24.0 * 16.0),
            m,
        );
        assert_eq!((cols, rows), (80, 24));
        assert_eq!(grid_size_for((0.0, 0.0), m), (0, 0));
    }

    #[test]
    fn theme_drives_the_default_colours() {
        let theme = PhazeTheme::default();
        let p = palette_from_theme(&theme.palette);
        assert_eq!(p.bg, to_rgb(theme.palette.bg_base));
        assert_eq!(p.fg, to_rgb(theme.palette.text_primary));
        assert_eq!(p.ansi, ANSI_BASE);
    }

    #[test]
    fn shared_writer_forwards_bytes() {
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let shared: SharedPtyWriter = Arc::new(Mutex::new(None));
        write_shared(&shared, b"dropped");
        *shared.lock().unwrap() = Some(Box::new(Sink(Arc::clone(&sink))));
        let core = Mutex::new(TermCore::new(20, 5));
        process_pty_output(&core, b"\x1b[6n", &mut SharedWriterRef(&shared));
        assert_eq!(&*sink.lock().unwrap(), b"\x1b[1;1R");
    }
}
