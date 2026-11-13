//! Screen model for the integrated terminal (no Floem views here).
//!
//! [`TermCore`] wraps `alacritty_terminal`'s [`Term`] (a real cell grid with
//! cursor addressing, the alternate screen, scroll regions and erase
//! semantics) and the `vte` ANSI [`Processor`] that drives it. The PTY itself
//! stays with `portable-pty` in `terminal.rs`: bytes read from the PTY go
//! through [`TermCore::feed`], and the replies the emulator produces (cursor
//! position reports, colour queries, size queries) come back in
//! [`FeedOutcome::reply`] for the caller to write to the PTY.
//!
//! On top of the emulator this module adds what the panel needs and
//! alacritty does not do:
//! - OSC 133 (prompt / command-done, with the exit code) and OSC 7 (working
//!   directory) shell-integration markers, found by a byte scanner that
//!   survives sequences split across reads. The bytes up to each marker are
//!   fed to the emulator first, so a marker records the grid row at that
//!   moment.
//! - Absolute row numbers: the emulator's history cap is set out of reach and
//!   history is trimmed here, so `trimmed + history + line` names a row for
//!   as long as it exists (command blocks and prompt navigation use them).
//! - A [`GridSnapshot`] for painting, with every colour already resolved.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape, NamedColor, Processor, Rgb, Timeout,
};
use phazeai_core::constants::terminal::{SCROLLBACK_DRAIN, SCROLLBACK_LIMIT};

/// Completed command blocks kept per terminal.
pub const MAX_BLOCKS: usize = 200;
/// History cap handed to the emulator. It must never be reached (the emulator
/// would then drop rows without telling us); [`TermCore`] trims history to
/// `SCROLLBACK_LIMIT` itself after every read.
const EMU_HISTORY_CAP: usize = 1_000_000_000;
/// Longest OSC payload kept by the scanner (only OSC 7 and 133 are read).
const OSC_PAYLOAD_MAX: usize = 4096;
/// Output text kept per command block (for the block list and AI explain).
const BLOCK_OUTPUT_MAX_CHARS: usize = 16_000;

// ── Events from the emulator ─────────────────────────────────────────────────

/// Collects the emulator's events while bytes are processed; drained by
/// [`TermCore::feed`] into a [`FeedOutcome`].
#[derive(Clone, Default)]
pub struct EventQueue(Arc<Mutex<Vec<Event>>>);

impl EventListener for EventQueue {
    fn send_event(&self, event: Event) {
        if let Ok(mut q) = self.0.lock() {
            q.push(event);
        }
    }
}

impl EventQueue {
    fn drain(&self) -> Vec<Event> {
        self.0
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }
}

/// Synchronized updates (`CSI ? 2026 h`) are not buffered: alacritty's
/// processor holds bytes back until the end marker or a timer the caller has
/// to poll, and a program that never sends the end marker would freeze the
/// view. Drawing every update as it comes is always correct, only less smooth.
#[derive(Default)]
pub struct NoSyncUpdates;

impl Timeout for NoSyncUpdates {
    fn set_timeout(&mut self, _: Duration) {}
    fn clear_timeout(&mut self) {}
    fn pending_timeout(&self) -> bool {
        false
    }
}

/// What a [`TermCore::feed`] call produced besides grid changes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FeedOutcome {
    /// Bytes to write back to the PTY (query replies), in order.
    pub reply: Vec<u8>,
    /// Text a program asked to put on the clipboard (OSC 52).
    pub clipboard: Option<String>,
    /// A command block started or finished.
    pub blocks_changed: bool,
    /// OSC 7 reported a (possibly unchanged) working directory.
    pub cwd_changed: bool,
}

// ── Sizes and colours ────────────────────────────────────────────────────────

/// Grid size in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermSize {
    pub cols: usize,
    pub rows: usize,
}

impl TermSize {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols: cols.max(2),
            rows: rows.max(1),
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// The 16 base colours (the table the terminal has always used).
pub const ANSI_BASE: [Rgb; 16] = [
    rgb(0, 0, 0),       // 0  Black
    rgb(194, 54, 33),   // 1  Red
    rgb(37, 188, 36),   // 2  Green
    rgb(173, 173, 39),  // 3  Yellow
    rgb(73, 46, 225),   // 4  Blue
    rgb(211, 56, 211),  // 5  Magenta
    rgb(51, 187, 200),  // 6  Cyan
    rgb(203, 204, 205), // 7  White
    rgb(129, 131, 131), // 8  Bright Black
    rgb(252, 57, 31),   // 9  Bright Red
    rgb(49, 231, 34),   // 10 Bright Green
    rgb(234, 236, 35),  // 11 Bright Yellow
    rgb(88, 51, 255),   // 12 Bright Blue
    rgb(249, 53, 248),  // 13 Bright Magenta
    rgb(20, 240, 240),  // 14 Bright Cyan
    rgb(233, 235, 235), // 15 Bright White
];

/// Colours the terminal draws with when a program does not choose its own
/// (foreground, background and cursor come from the IDE theme).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermPalette {
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    pub ansi: [Rgb; 16],
}

impl Default for TermPalette {
    fn default() -> Self {
        Self {
            fg: rgb(204, 204, 204),
            bg: rgb(16, 16, 20),
            cursor: rgb(204, 204, 204),
            ansi: ANSI_BASE,
        }
    }
}

fn dim(c: Rgb) -> Rgb {
    let f = |v: u8| ((v as u16 * 2) / 3) as u8;
    rgb(f(c.r), f(c.g), f(c.b))
}

impl TermPalette {
    /// Colour of xterm's 256-colour index `idx`.
    pub fn indexed(&self, idx: u8) -> Rgb {
        match idx {
            0..=15 => self.ansi[idx as usize],
            16..=231 => {
                let i = idx - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
            }
            _ => {
                let v = 8 + (idx - 232) * 10;
                rgb(v, v, v)
            }
        }
    }

    /// Colour for an index into alacritty's colour table (0..256 indexed,
    /// then foreground, background, cursor, dim colours, bright and dim
    /// foreground).
    pub fn by_index(&self, idx: usize) -> Rgb {
        const FG: usize = NamedColor::Foreground as usize;
        const BG: usize = NamedColor::Background as usize;
        const CURSOR: usize = NamedColor::Cursor as usize;
        const DIM_BLACK: usize = NamedColor::DimBlack as usize;
        const DIM_WHITE: usize = NamedColor::DimWhite as usize;
        const DIM_FG: usize = NamedColor::DimForeground as usize;
        match idx {
            0..=255 => self.indexed(idx as u8),
            FG => self.fg,
            BG => self.bg,
            CURSOR => self.cursor,
            DIM_BLACK..=DIM_WHITE => dim(self.ansi[idx - DIM_BLACK]),
            DIM_FG => dim(self.fg),
            _ => self.fg, // BrightForeground
        }
    }
}

/// Resolve a cell colour: a program's own colour-table changes (OSC 4/10/11)
/// first, then the palette.
pub fn resolve_color(color: AnsiColor, palette: &TermPalette, colors: &Colors) -> Rgb {
    match color {
        AnsiColor::Spec(c) => c,
        AnsiColor::Indexed(i) => colors[i as usize].unwrap_or_else(|| palette.indexed(i)),
        AnsiColor::Named(n) => colors[n as usize].unwrap_or_else(|| palette.by_index(n as usize)),
    }
}

/// How one cell is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellStyle {
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
}

/// Resolve a cell's attributes into colours and text style (dim, inverse and
/// hidden are applied here).
pub fn cell_style(cell: &Cell, palette: &TermPalette, colors: &Colors) -> CellStyle {
    let flags = cell.flags;
    let mut fg = resolve_color(cell.fg, palette, colors);
    let mut bg = resolve_color(cell.bg, palette, colors);
    if flags.contains(Flags::DIM) {
        fg = dim(fg);
    }
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg;
    }
    CellStyle {
        fg,
        bg,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        strikeout: flags.contains(Flags::STRIKEOUT),
    }
}

// ── Escape-sequence scanner (OSC markers, history clears) ────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

#[derive(Debug, PartialEq, Eq)]
enum ScanEvent {
    /// An OSC sequence ended; feed bytes up to `end` (exclusive) first.
    Osc { end: usize, payload: Vec<u8> },
    /// `CSI 3 J` or `ESC c` (they drop history); `esc` is the index of its
    /// ESC when that is in the current chunk.
    HistoryClear { esc: Option<usize>, end: usize },
}

/// Finds OSC sequences and history-clearing sequences in the PTY byte stream,
/// keeping its state between reads.
#[derive(Default)]
struct SeqScanner {
    state: ScanState,
    payload: Vec<u8>,
    csi: Vec<u8>,
    csi_simple: bool,
    esc_at: Option<usize>,
}

impl SeqScanner {
    /// Start of a new chunk: positions from the previous one are gone.
    fn new_chunk(&mut self) {
        self.esc_at = None;
    }

    fn step(&mut self, b: u8, i: usize, out: &mut Vec<ScanEvent>) {
        match self.state {
            ScanState::Ground => {
                if b == 0x1b {
                    self.enter_esc(i);
                }
            }
            ScanState::Esc => self.after_esc(b, i, out),
            ScanState::Csi => match b {
                0x1b => self.enter_esc(i),
                0x18 | 0x1a => self.state = ScanState::Ground,
                0x30..=0x3f => {
                    if self.csi.len() < 16 {
                        self.csi.push(b);
                    } else {
                        self.csi_simple = false;
                    }
                }
                0x20..=0x2f => self.csi_simple = false,
                0x40..=0x7e => {
                    if b == b'J' && self.csi_simple && self.csi == b"3" {
                        out.push(ScanEvent::HistoryClear {
                            esc: self.esc_at,
                            end: i + 1,
                        });
                    }
                    self.state = ScanState::Ground;
                }
                _ => {}
            },
            ScanState::Osc => match b {
                0x07 => {
                    out.push(ScanEvent::Osc {
                        end: i + 1,
                        payload: std::mem::take(&mut self.payload),
                    });
                    self.state = ScanState::Ground;
                }
                0x1b => self.state = ScanState::OscEsc,
                0x18 | 0x1a => {
                    self.payload.clear();
                    self.state = ScanState::Ground;
                }
                _ => {
                    if self.payload.len() < OSC_PAYLOAD_MAX {
                        self.payload.push(b);
                    }
                }
            },
            ScanState::OscEsc => {
                if b == b'\\' {
                    out.push(ScanEvent::Osc {
                        end: i + 1,
                        payload: std::mem::take(&mut self.payload),
                    });
                    self.state = ScanState::Ground;
                } else {
                    // The ESC ended the OSC (the parser dispatches it there)
                    // and starts a new escape sequence with this byte.
                    out.push(ScanEvent::Osc {
                        end: i,
                        payload: std::mem::take(&mut self.payload),
                    });
                    self.esc_at = i.checked_sub(1);
                    self.after_esc(b, i, out);
                }
            }
        }
    }

    fn enter_esc(&mut self, i: usize) {
        self.esc_at = Some(i);
        self.state = ScanState::Esc;
    }

    fn after_esc(&mut self, b: u8, i: usize, out: &mut Vec<ScanEvent>) {
        match b {
            b']' => {
                self.payload.clear();
                self.state = ScanState::Osc;
            }
            b'[' => {
                self.csi.clear();
                self.csi_simple = true;
                self.state = ScanState::Csi;
            }
            b'c' => {
                out.push(ScanEvent::HistoryClear {
                    esc: self.esc_at,
                    end: i + 1,
                });
                self.state = ScanState::Ground;
            }
            0x1b => self.enter_esc(i),
            _ => self.state = ScanState::Ground,
        }
    }
}

/// Parse an OSC 7 URI (`file://[host]/path`) into a local path.
pub fn parse_osc7(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path = if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{}", rest.split_once('/')?.1)
    };
    let path = percent_decode(&path);
    (!path.is_empty()).then_some(path)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ── Command blocks ───────────────────────────────────────────────────────────

/// One shell command, delimited by OSC 133 markers: `A` (prompt start) opens
/// it at the cursor's row, `D;<code>` (command done) closes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub id: u64,
    /// Absolute row of the prompt.
    pub prompt_row: u64,
    /// Absolute row after the output (exclusive), once finished.
    pub end_row: Option<u64>,
    /// Exit code from `133;D;<code>`; `None` when none was given.
    pub exit_code: Option<i32>,
    pub finished: bool,
    /// Prompt + command line text, captured when the block finished.
    pub command: String,
    /// Output text, captured when the block finished (capped).
    pub output: String,
}

/// Gutter mark for a viewport row that holds a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowMark {
    pub finished: bool,
    pub exit_code: Option<i32>,
}

// ── Snapshot for painting ────────────────────────────────────────────────────

/// Highlight state of a cell for find-in-terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FindMark {
    #[default]
    None,
    Match,
    Current,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapCell {
    pub c: char,
    /// Zero-width characters (combining marks) drawn with `c`.
    pub zerowidth: Vec<char>,
    pub style: CellStyle,
    /// First half of a double-width character.
    pub wide: bool,
    /// Placeholder cell (second half of a wide character, or the padding
    /// before a wide character that wrapped); draws nothing.
    pub spacer: bool,
    pub selected: bool,
    pub find: FindMark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapCursor {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    pub wide: bool,
}

/// Everything needed to paint the viewport.
#[derive(Clone, Debug)]
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Vec<SnapCell>>,
    pub cursor: Option<SnapCursor>,
    pub marks: Vec<Option<RowMark>>,
    pub default_bg: Rgb,
    pub default_fg: Rgb,
    pub cursor_color: Rgb,
    pub display_offset: usize,
    pub alt_screen: bool,
}

/// A find-in-terminal match: absolute row and column range (end exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindHit {
    pub row: u64,
    pub start: usize,
    pub end: usize,
}

// ── The core ─────────────────────────────────────────────────────────────────

/// One terminal's emulator state; shared between the PTY reader thread
/// (which feeds it) and the UI (which paints and scrolls it).
pub struct TermCore {
    term: Term<EventQueue>,
    processor: Processor<NoSyncUpdates>,
    events: EventQueue,
    scanner: SeqScanner,
    palette: TermPalette,
    /// Rows dropped from the top of the primary screen's history so far.
    trimmed: u64,
    /// Primary history size as of the last time it was the active grid.
    primary_history: usize,
    cell_px: (u16, u16),
    blocks: Vec<Block>,
    next_block_id: u64,
    blocks_version: u64,
    cwd: Option<String>,
}

impl TermCore {
    pub fn new(cols: usize, rows: usize) -> Self {
        let events = EventQueue::default();
        let config = Config {
            scrolling_history: EMU_HISTORY_CAP,
            ..Config::default()
        };
        let term = Term::new(config, &TermSize::new(cols, rows), events.clone());
        Self {
            term,
            processor: Processor::new(),
            events,
            scanner: SeqScanner::default(),
            palette: TermPalette::default(),
            trimmed: 0,
            primary_history: 0,
            cell_px: (8, 16),
            blocks: Vec::new(),
            next_block_id: 1,
            blocks_version: 0,
            cwd: None,
        }
    }

    // ── Feeding ──────────────────────────────────────────────────────────

    /// Process bytes read from the PTY.
    pub fn feed(&mut self, bytes: &[u8]) -> FeedOutcome {
        let mut out = FeedOutcome::default();
        let mut fed = 0usize;
        let mut events = Vec::new();
        self.scanner.new_chunk();
        for (i, &b) in bytes.iter().enumerate() {
            self.scanner.step(b, i, &mut events);
            for ev in events.drain(..) {
                match ev {
                    ScanEvent::Osc { end, payload } => {
                        let end = end.max(fed);
                        self.advance(&bytes[fed..end]);
                        fed = end;
                        self.apply_osc(&payload, &mut out);
                    }
                    ScanEvent::HistoryClear { esc, end } => {
                        let split = esc.unwrap_or(fed).max(fed);
                        self.advance(&bytes[fed..split]);
                        fed = split;
                        let before = self.primary_history_now();
                        self.advance(&bytes[fed..end]);
                        fed = end;
                        let after = self.primary_history_now();
                        if after < before {
                            self.trimmed += (before - after) as u64;
                        }
                    }
                }
            }
        }
        self.advance(&bytes[fed..]);
        self.trim_history();
        self.drain_events(&mut out);
        out
    }

    fn advance(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.processor.advance(&mut self.term, bytes);
            if !self.is_alt_screen() {
                self.primary_history = self.term.grid().history_size();
            }
        }
    }

    fn primary_history_now(&self) -> usize {
        if self.is_alt_screen() {
            self.primary_history
        } else {
            self.term.grid().history_size()
        }
    }

    /// Keep history at `SCROLLBACK_LIMIT` rows, dropping the oldest in steps
    /// of `SCROLLBACK_DRAIN` and counting them in `trimmed`.
    fn trim_history(&mut self) {
        if self.is_alt_screen() {
            return;
        }
        let history = self.term.grid().history_size();
        if history > SCROLLBACK_LIMIT + SCROLLBACK_DRAIN {
            // A command still running whose prompt is about to go: keep its
            // text now, while the rows exist.
            let boundary = self.trimmed + (history - SCROLLBACK_LIMIT) as u64;
            let cursor_row = self.cursor_abs_row();
            if let Some(idx) = self.blocks.iter().rposition(|b| !b.finished) {
                let b = &self.blocks[idx];
                if b.prompt_row < boundary && b.command.is_empty() {
                    let (command, output) = self.capture_block_text(b.prompt_row, cursor_row);
                    let b = &mut self.blocks[idx];
                    b.command = command;
                    b.output = output;
                }
            }
            let grid = self.term.grid_mut();
            grid.update_history(SCROLLBACK_LIMIT);
            grid.update_history(EMU_HISTORY_CAP);
            self.trimmed += (history - SCROLLBACK_LIMIT) as u64;
            self.primary_history = SCROLLBACK_LIMIT;
        }
    }

    fn drain_events(&mut self, out: &mut FeedOutcome) {
        for event in self.events.drain() {
            match event {
                Event::PtyWrite(text) => out.reply.extend_from_slice(text.as_bytes()),
                Event::ColorRequest(index, format) => {
                    let color =
                        self.term.colors()[index].unwrap_or_else(|| self.palette.by_index(index));
                    out.reply.extend_from_slice(format(color).as_bytes());
                }
                Event::TextAreaSizeRequest(format) => {
                    let size = WindowSize {
                        num_lines: self.term.screen_lines() as u16,
                        num_cols: self.term.columns() as u16,
                        cell_width: self.cell_px.0,
                        cell_height: self.cell_px.1,
                    };
                    out.reply.extend_from_slice(format(size).as_bytes());
                }
                Event::ClipboardStore(_, text) => out.clipboard = Some(text),
                // Reading the clipboard from a program (OSC 52 paste) is off,
                // and the title, bell and redraw hints are not used.
                _ => {}
            }
        }
    }

    fn apply_osc(&mut self, payload: &[u8], out: &mut FeedOutcome) {
        let text = String::from_utf8_lossy(payload);
        let mut parts = text.split(';');
        match parts.next() {
            Some("7") => {
                let uri = text.split_once(';').map(|x| x.1).unwrap_or("");
                if let Some(path) = parse_osc7(uri) {
                    self.cwd = Some(path);
                    out.cwd_changed = true;
                }
            }
            Some("133") => {
                let marker = parts.next().unwrap_or("");
                match marker {
                    "A" => {
                        self.on_prompt_start();
                        out.blocks_changed = true;
                    }
                    "D" => {
                        let code = parts.next().and_then(|c| c.trim().parse::<i32>().ok());
                        self.on_command_done(code);
                        out.blocks_changed = true;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // ── Blocks ───────────────────────────────────────────────────────────

    fn cursor_abs_row(&self) -> u64 {
        let grid = self.term.grid();
        self.trimmed + grid.history_size() as u64 + grid.cursor.point.line.0.max(0) as u64
    }

    fn on_prompt_start(&mut self) {
        if self.is_alt_screen() {
            return;
        }
        let row = self.cursor_abs_row();
        if let Some(last) = self.blocks.last() {
            if !last.finished && last.prompt_row == row {
                return; // the same prompt announced again
            }
        }
        self.finish_open_block(None);
        self.blocks.push(Block {
            id: self.next_block_id,
            prompt_row: row,
            end_row: None,
            exit_code: None,
            finished: false,
            command: String::new(),
            output: String::new(),
        });
        self.next_block_id += 1;
        if self.blocks.len() > MAX_BLOCKS {
            let drop = self.blocks.len() - MAX_BLOCKS;
            self.blocks.drain(0..drop);
        }
        self.blocks_version += 1;
    }

    fn on_command_done(&mut self, code: Option<i32>) {
        if self.is_alt_screen() {
            return;
        }
        if self.finish_open_block(code) {
            self.blocks_version += 1;
        }
    }

    /// Close the last block if it is still open. Returns whether one was.
    fn finish_open_block(&mut self, code: Option<i32>) -> bool {
        let Some(idx) = self.blocks.iter().rposition(|b| !b.finished) else {
            return false;
        };
        let cursor = self.term.grid().cursor.point;
        let mut end = self.cursor_abs_row();
        if cursor.column.0 > 0 {
            end += 1;
        }
        let prompt_row = self.blocks[idx].prompt_row;
        let end = end.max(prompt_row + 1);
        // Text kept when the prompt row was trimmed away stays; otherwise
        // capture it now.
        let keep = prompt_row < self.trimmed && !self.blocks[idx].command.is_empty();
        let captured = (!keep).then(|| self.capture_block_text(prompt_row, end));
        let block = &mut self.blocks[idx];
        block.end_row = Some(end);
        block.exit_code = code;
        block.finished = true;
        if let Some((command, output)) = captured {
            block.command = command;
            block.output = output;
        }
        true
    }

    /// Grid line of an absolute row, if the row still exists.
    fn abs_to_line(&self, abs: u64) -> Option<Line> {
        let grid = self.term.grid();
        let base = self.trimmed + grid.history_size() as u64;
        let line = abs as i64 - base as i64;
        let top = -(grid.history_size() as i64);
        let bottom = grid.screen_lines() as i64 - 1;
        (top..=bottom).contains(&line).then_some(Line(line as i32))
    }

    fn line_text(&self, line: Line) -> String {
        let grid = self.term.grid();
        let row = &grid[line];
        let mut s = String::new();
        for col in 0..grid.columns() {
            let cell = &row[Column(col)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            s.push(cell_char(cell));
            if let Some(zw) = cell.zerowidth() {
                s.extend(zw.iter());
            }
        }
        s.trim_end().to_string()
    }

    fn line_wraps(&self, line: Line) -> bool {
        let grid = self.term.grid();
        grid[line][Column(grid.columns() - 1)]
            .flags
            .contains(Flags::WRAPLINE)
    }

    /// Text of rows `[start, end)`: the first logical line (the prompt and
    /// command, following wrapped rows) and the rest (the output).
    fn capture_block_text(&self, start: u64, end: u64) -> (String, String) {
        let mut command = String::new();
        let mut output = String::new();
        let mut in_command = true;
        let mut row = start;
        while row < end {
            if !in_command && output.len() >= BLOCK_OUTPUT_MAX_CHARS {
                break;
            }
            let Some(line) = self.abs_to_line(row) else {
                row += 1;
                continue;
            };
            let text = self.line_text(line);
            let wraps = self.line_wraps(line);
            if in_command {
                command.push_str(&text);
                if !wraps {
                    in_command = false;
                }
            } else {
                output.push_str(&text);
                if !wraps {
                    output.push('\n');
                }
            }
            row += 1;
        }
        if output.len() > BLOCK_OUTPUT_MAX_CHARS {
            let mut cut = BLOCK_OUTPUT_MAX_CHARS;
            while !output.is_char_boundary(cut) {
                cut -= 1;
            }
            output.truncate(cut);
        }
        while output.ends_with('\n') {
            output.pop();
        }
        (command, output)
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Increases whenever a block starts or finishes.
    pub fn blocks_version(&self) -> u64 {
        self.blocks_version
    }

    /// Absolute rows of every known prompt that still exists in the grid.
    pub fn prompt_rows(&self) -> Vec<u64> {
        self.blocks
            .iter()
            .map(|b| b.prompt_row)
            .filter(|r| *r >= self.trimmed)
            .collect()
    }

    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    // ── Size, palette, modes ─────────────────────────────────────────────

    pub fn size(&self) -> TermSize {
        TermSize::new(self.term.columns(), self.term.screen_lines())
    }

    /// Resize the grid (the caller resizes the PTY). `cell_px` is the cell
    /// size in pixels, reported to programs that ask for it.
    pub fn resize(&mut self, cols: usize, rows: usize, cell_px: (u16, u16)) {
        self.cell_px = cell_px;
        let new = TermSize::new(cols, rows);
        if new != self.size() {
            self.term.resize(new);
            if !self.is_alt_screen() {
                self.primary_history = self.term.grid().history_size();
            }
        }
    }

    pub fn set_palette(&mut self, palette: TermPalette) {
        self.palette = palette;
    }

    pub fn palette(&self) -> &TermPalette {
        &self.palette
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn is_alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Rows dropped from the top of history so far.
    pub fn trimmed_rows(&self) -> u64 {
        self.trimmed
    }

    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Cursor position on the screen (row, column), ignoring scrolling.
    pub fn cursor_point(&self) -> (usize, usize) {
        let p = self.term.grid().cursor.point;
        (p.line.0.max(0) as usize, p.column.0)
    }

    /// Absolute row at the top of the viewport.
    pub fn viewport_top_abs(&self) -> u64 {
        self.trimmed + self.history_size() as u64 - self.display_offset() as u64
    }

    // ── Scrolling ────────────────────────────────────────────────────────

    /// Scroll the view; positive `lines` moves up into history.
    pub fn scroll_lines(&mut self, lines: i32) {
        if lines != 0 {
            self.term.scroll_display(Scroll::Delta(lines));
        }
    }

    pub fn scroll_page(&mut self, up: bool) {
        self.term
            .scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
    }

    pub fn scroll_to_bottom(&mut self) {
        if self.display_offset() != 0 {
            self.term.scroll_display(Scroll::Bottom);
        }
    }

    /// Scroll so the absolute row `abs` is at the top of the view (as far as
    /// history allows).
    pub fn scroll_to_abs(&mut self, abs: u64) {
        let history = self.history_size() as i64;
        let line = abs as i64 - (self.trimmed as i64 + history);
        let want = (-line).clamp(0, history);
        let delta = want - self.display_offset() as i64;
        self.scroll_lines(delta as i32);
    }

    /// Jump to the previous (`dir < 0`) or next prompt relative to the top of
    /// the view. Returns whether the view moved.
    pub fn jump_prompt(&mut self, dir: i32) -> bool {
        let top = self.viewport_top_abs();
        let rows = self.prompt_rows();
        let target = if dir < 0 {
            rows.iter().rev().copied().find(|r| *r < top)
        } else {
            rows.iter().copied().find(|r| *r > top)
        };
        let before = self.display_offset();
        match target {
            Some(r) => self.scroll_to_abs(r),
            None if dir > 0 => self.scroll_to_bottom(),
            None => {}
        }
        before != self.display_offset()
    }

    // ── Selection and copy ───────────────────────────────────────────────

    fn viewport_to_point(&self, row: usize, col: usize) -> Point {
        let rows = self.term.screen_lines();
        let cols = self.term.columns();
        let row = row.min(rows - 1) as i32;
        Point::new(
            Line(row - self.display_offset() as i32),
            Column(col.min(cols - 1)),
        )
    }

    /// Start a selection at a viewport cell (`Simple` for a click,
    /// `Semantic` for a double click, `Lines` for a triple click).
    pub fn start_selection(&mut self, ty: SelectionType, row: usize, col: usize, side: Side) {
        let point = self.viewport_to_point(row, col);
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    pub fn update_selection(&mut self, row: usize, col: usize, side: Side) {
        let point = self.viewport_to_point(row, col);
        if let Some(sel) = self.term.selection.as_mut() {
            sel.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.term.selection.as_ref().is_some_and(|s| !s.is_empty())
    }

    /// Text of the selection, if there is a non-empty one.
    pub fn selection_text(&self) -> Option<String> {
        if !self.has_selection() {
            return None;
        }
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    /// All text in history and on the screen, trailing blank lines removed.
    pub fn all_text(&self) -> String {
        let grid = self.term.grid();
        let mut lines = Vec::new();
        let mut current = String::new();
        let top = -(grid.history_size() as i32);
        for l in top..grid.screen_lines() as i32 {
            let line = Line(l);
            current.push_str(&self.line_text(line));
            if !self.line_wraps(line) {
                lines.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Text of the viewport rows (for tests and debugging).
    pub fn screen_lines_text(&self) -> Vec<String> {
        (0..self.term.screen_lines())
            .map(|r| self.line_text(Line(r as i32 - self.display_offset() as i32)))
            .collect()
    }

    // ── Find ─────────────────────────────────────────────────────────────

    /// Case-insensitive matches of `query` in history and on the screen
    /// (within single rows).
    pub fn find(&self, query: &str) -> Vec<FindHit> {
        let needle: Vec<char> = query.chars().map(fold_char).collect();
        if needle.is_empty() {
            return Vec::new();
        }
        let grid = self.term.grid();
        let base = self.trimmed + grid.history_size() as u64;
        let top = -(grid.history_size() as i32);
        let mut hits = Vec::new();
        for l in top..grid.screen_lines() as i32 {
            let abs = (base as i64 + l as i64) as u64;
            for (start, end) in self.find_in_line(Line(l), &needle) {
                hits.push(FindHit {
                    row: abs,
                    start,
                    end,
                });
            }
        }
        hits
    }

    /// Matches of `needle` (already case-folded) in one row, as column ranges.
    fn find_in_line(&self, line: Line, needle: &[char]) -> Vec<(usize, usize)> {
        let grid = self.term.grid();
        let row = &grid[line];
        let mut chars = Vec::with_capacity(grid.columns());
        let mut cols = Vec::with_capacity(grid.columns());
        for col in 0..grid.columns() {
            let cell = &row[Column(col)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            chars.push(fold_char(cell_char(cell)));
            cols.push(col);
        }
        let mut out = Vec::new();
        if chars.len() < needle.len() {
            return out;
        }
        let mut i = 0;
        while i + needle.len() <= chars.len() {
            if chars[i..i + needle.len()] == *needle {
                let last = i + needle.len() - 1;
                let end_col = cols[last]
                    + if row[Column(cols[last])].flags.contains(Flags::WIDE_CHAR) {
                        2
                    } else {
                        1
                    };
                out.push((cols[i], end_col));
                i += needle.len();
            } else {
                i += 1;
            }
        }
        out
    }

    // ── Snapshot ─────────────────────────────────────────────────────────

    /// Everything needed to paint the viewport. `find` highlights matches of
    /// that query; `current` marks one match as the current one.
    pub fn snapshot(&self, find: Option<&str>, current: Option<FindHit>) -> GridSnapshot {
        let content = self.term.renderable_content();
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let offset = grid.display_offset();
        let colors = self.term.colors();
        let palette = &self.palette;
        let selection = content.selection;
        let needle: Vec<char> = find
            .map(|q| q.chars().map(fold_char).collect())
            .unwrap_or_default();
        let base = self.trimmed + grid.history_size() as u64;

        let mut cells = Vec::with_capacity(rows);
        for r in 0..rows {
            let line = Line(r as i32 - offset as i32);
            let row = &grid[line];
            let mut out_row = Vec::with_capacity(cols);
            for c in 0..cols {
                let cell = &row[Column(c)];
                let point = Point::new(line, Column(c));
                out_row.push(SnapCell {
                    c: cell_char(cell),
                    zerowidth: cell.zerowidth().map(|z| z.to_vec()).unwrap_or_default(),
                    style: cell_style(cell, palette, colors),
                    wide: cell.flags.contains(Flags::WIDE_CHAR),
                    spacer: cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
                    selected: selection.is_some_and(|s| s.contains(point)),
                    find: FindMark::None,
                });
            }
            if !needle.is_empty() {
                let abs = (base as i64 + line.0 as i64) as u64;
                for (start, end) in self.find_in_line(line, &needle) {
                    let is_current =
                        current.is_some_and(|h| h.row == abs && h.start == start && h.end == end);
                    for cell in out_row.iter_mut().take(end.min(cols)).skip(start) {
                        cell.find = if is_current {
                            FindMark::Current
                        } else {
                            FindMark::Match
                        };
                    }
                }
            }
            cells.push(out_row);
        }

        let cursor = {
            let rc = content.cursor;
            let vr = rc.point.line.0 + offset as i32;
            (rc.shape != CursorShape::Hidden && vr >= 0 && (vr as usize) < rows).then(|| {
                SnapCursor {
                    row: vr as usize,
                    col: rc.point.column.0.min(cols - 1),
                    shape: rc.shape,
                    wide: grid[rc.point].flags.contains(Flags::WIDE_CHAR),
                }
            })
        };

        let mut marks = vec![None; rows];
        if !self.is_alt_screen() {
            for b in &self.blocks {
                let line = b.prompt_row as i64 - base as i64;
                let vr = line + offset as i64;
                if (0..rows as i64).contains(&vr) {
                    marks[vr as usize] = Some(RowMark {
                        finished: b.finished,
                        exit_code: b.exit_code,
                    });
                }
            }
        }

        GridSnapshot {
            cols,
            rows,
            cells,
            cursor,
            marks,
            default_bg: resolve_color(AnsiColor::Named(NamedColor::Background), palette, colors),
            default_fg: resolve_color(AnsiColor::Named(NamedColor::Foreground), palette, colors),
            cursor_color: colors[NamedColor::Cursor as usize].unwrap_or(palette.cursor),
            display_offset: offset,
            alt_screen: self.is_alt_screen(),
        }
    }
}

/// Feed PTY output to `core` and write the emulator's replies to `reply_to`
/// (the PTY), outside the lock. This is what the reader thread runs.
pub fn process_pty_output(
    core: &Mutex<TermCore>,
    bytes: &[u8],
    reply_to: &mut dyn std::io::Write,
) -> FeedOutcome {
    let outcome = match core.lock() {
        Ok(mut c) => c.feed(bytes),
        Err(_) => return FeedOutcome::default(),
    };
    if !outcome.reply.is_empty() {
        let _ = reply_to.write_all(&outcome.reply);
        let _ = reply_to.flush();
    }
    outcome
}

/// The character a cell shows. The emulator keeps a TAB in the cell where
/// the tab started; it is blank on screen.
fn cell_char(cell: &Cell) -> char {
    if cell.c == '\t' {
        ' '
    } else {
        cell.c
    }
}

fn fold_char(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

// ── Input encoding ───────────────────────────────────────────────────────────

/// Modifier keys held during a key press or mouse event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    /// xterm's modifier parameter (1 = none).
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
    fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }
}

/// Keys that are not plain text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermKey {
    Enter,
    Backspace,
    Tab,
    Escape,
    Delete,
    Insert,
    Home,
    End,
    Up,
    Down,
    Right,
    Left,
    PageUp,
    PageDown,
    F(u8),
}

/// Bytes a special key sends. `app_cursor` is DECCKM (`CSI ? 1 h`): programs
/// such as `less`, `htop` and `vim` turn it on and then expect `ESC O A`
/// for the arrows.
pub fn key_bytes(key: TermKey, mods: Mods, app_cursor: bool) -> Vec<u8> {
    let cursor = |letter: u8| -> Vec<u8> {
        if mods.any() {
            format!("\x1b[1;{}{}", mods.param(), letter as char).into_bytes()
        } else if app_cursor {
            vec![0x1b, b'O', letter]
        } else {
            vec![0x1b, b'[', letter]
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if mods.any() {
            format!("\x1b[{};{}~", n, mods.param()).into_bytes()
        } else {
            format!("\x1b[{}~", n).into_bytes()
        }
    };
    match key {
        TermKey::Enter => {
            if mods.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        TermKey::Backspace => {
            if mods.alt {
                vec![0x1b, 0x7f]
            } else if mods.ctrl {
                vec![0x08]
            } else {
                vec![0x7f]
            }
        }
        TermKey::Tab => {
            if mods.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        TermKey::Escape => b"\x1b".to_vec(),
        TermKey::Delete => tilde(3),
        TermKey::Insert => tilde(2),
        TermKey::PageUp => tilde(5),
        TermKey::PageDown => tilde(6),
        TermKey::Home => cursor(b'H'),
        TermKey::End => cursor(b'F'),
        TermKey::Up => cursor(b'A'),
        TermKey::Down => cursor(b'B'),
        TermKey::Right => cursor(b'C'),
        TermKey::Left => cursor(b'D'),
        TermKey::F(n @ 1..=4) => {
            let letter = b"PQRS"[(n - 1) as usize];
            if mods.any() {
                format!("\x1b[1;{}{}", mods.param(), letter as char).into_bytes()
            } else {
                vec![0x1b, b'O', letter]
            }
        }
        TermKey::F(n) => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            match n.checked_sub(5).and_then(|i| CODES.get(i as usize)) {
                Some(code) => tilde(*code),
                None => Vec::new(),
            }
        }
    }
}

/// Bytes typed text sends: Ctrl turns letters into control characters, Alt
/// prefixes ESC.
pub fn text_key_bytes(text: &str, mods: Mods) -> Vec<u8> {
    if mods.ctrl {
        let Some(c) = text.chars().next() else {
            return Vec::new();
        };
        let byte = if c.is_ascii_alphabetic() {
            Some(c.to_ascii_uppercase() as u8 - b'A' + 1)
        } else {
            match c {
                '@' | ' ' | '2' => Some(0x00),
                '[' | '3' => Some(0x1b),
                '\\' | '4' => Some(0x1c),
                ']' | '5' => Some(0x1d),
                '^' | '6' => Some(0x1e),
                '_' | '-' | '7' => Some(0x1f),
                '?' | '8' => Some(0x7f),
                _ => None,
            }
        };
        match byte {
            Some(b) if mods.alt => vec![0x1b, b],
            Some(b) => vec![b],
            None => Vec::new(),
        }
    } else if mods.alt {
        let mut out = vec![0x1b];
        out.extend_from_slice(text.as_bytes());
        out
    } else {
        text.as_bytes().to_vec()
    }
}

/// Bytes for pasting `text`: with bracketed paste on (`CSI ? 2004 h`) the
/// text is wrapped in `ESC [200~` / `ESC [201~` (with any ESC in it removed
/// so it cannot end the bracket early); otherwise newlines become CR, as a
/// typed Enter would.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend(text.bytes().filter(|b| *b != 0x1b));
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Mouse buttons as the terminal protocol numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseCode {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held (any-event tracking).
    NoButton,
}

impl MouseCode {
    fn number(self) -> u8 {
        match self {
            MouseCode::Left => 0,
            MouseCode::Middle => 1,
            MouseCode::Right => 2,
            MouseCode::NoButton => 3,
            MouseCode::WheelUp => 64,
            MouseCode::WheelDown => 65,
        }
    }
}

/// Encode a mouse event for a program that enabled mouse reporting.
/// `col`/`row` are 0-based viewport cells. SGR mode (`CSI ? 1006 h`) gives
/// `ESC [ < b ; x ; y M|m`; otherwise the legacy `ESC [ M` form is used
/// (coordinates past 223 cannot be sent then, unless UTF-8 mode is on).
pub fn mouse_report(
    code: MouseCode,
    pressed: bool,
    motion: bool,
    col: usize,
    row: usize,
    mods: Mods,
    mode: TermMode,
) -> Vec<u8> {
    let mut b = code.number();
    if mods.shift {
        b += 4;
    }
    if mods.alt {
        b += 8;
    }
    if mods.ctrl {
        b += 16;
    }
    if motion {
        b += 32;
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if pressed { 'M' } else { 'm' };
        return format!("\x1b[<{};{};{}{}", b, col + 1, row + 1, end).into_bytes();
    }
    if !pressed {
        // Legacy release: button 3 with the modifiers kept.
        b = (b & !0b11) | 3;
    }
    let mut out = b"\x1b[M".to_vec();
    out.push(32 + b);
    for v in [col + 1, row + 1] {
        let v = 32 + v;
        if mode.contains(TermMode::UTF8_MOUSE) {
            match char::from_u32(v as u32).filter(|_| v < 2048) {
                Some(c) => {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                None => return Vec::new(),
            }
        } else if v <= 255 {
            out.push(v as u8);
        } else {
            return Vec::new();
        }
    }
    out
}

#[cfg(test)]
#[path = "terminal_emu_tests.rs"]
mod tests;
