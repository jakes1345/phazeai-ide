use arboard;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use floem::{
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, Modifiers},
    peniko::Color,
    reactive::{create_effect, create_memo, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    style::{CursorStyle, Display},
    text::{Attrs, AttrsList, FamilyOwned, TextLayout, Weight},
    views::{canvas, container, dyn_stack, empty, label, scroll, stack, text_input, Decorators},
    IntoView, Renderer,
};
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use vte::{Params, Perform};

use crate::commands::{execute_command, match_global_shortcut};
use crate::domain_state::IdeState;
use crate::util::safe_get;
use phazeai_core::constants::terminal as term_consts;
use phazeai_core::Settings;

use crate::theme::PhazeTheme;
use std::time::{Duration, Instant};

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
        let to_val = |v: u8| {
            if v == 0 {
                0u8
            } else {
                55u8.saturating_add(v.saturating_mul(40))
            }
        };
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
        Self {
            segments: Vec::new(),
        }
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

// ── Terminal Block ────────────────────────────────────────────────────────────

/// One completed shell command unit: the prompt display + all output.
/// Populated from OSC 133 markers (A = prompt start, D = command done).
#[derive(Clone, Debug)]
struct TermBlock {
    id: u64,
    lines: Vec<TermLine>,
    exit_code: i32,
}

// ── Terminal State ────────────────────────────────────────────────────────────

/// Maximum completed blocks retained in memory (each may have many lines).
const MAX_BLOCKS: usize = 200;
/// Maximum lines in the current (in-progress) block before trimming old ones.
const MAX_CURRENT_LINES: usize = 5_000;

struct TermState {
    /// Finalized blocks (OSC 133;D received, committed on next OSC 133;A).
    blocks: Vec<TermBlock>,
    /// Lines accumulating for the current in-progress block.
    current_block_lines: Vec<TermLine>,
    /// Stable id for the block currently being built.
    current_block_id: u64,
    /// Monotonically increasing id counter.
    next_id: u64,
    /// Exit code received from OSC 133;D, held until A fires and commits.
    pending_exit_code: Option<i32>,

    current_line: TermLine,
    cur_fg: TermColor,
    cur_bg: TermColor,
    cur_bold: bool,
    cursor_col: usize,
    pub cwd: String,
}

impl TermState {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            current_block_lines: Vec::new(),
            current_block_id: 0,
            next_id: 1,
            pending_exit_code: None,
            current_line: TermLine::new(),
            cur_fg: TermColor::Default,
            cur_bg: TermColor::Default,
            cur_bold: false,
            cursor_col: 0,
            cwd: String::new(),
        }
    }

    fn commit_line(&mut self) {
        let line = std::mem::replace(&mut self.current_line, TermLine::new());
        self.current_block_lines.push(line);
        if self.current_block_lines.len() > MAX_CURRENT_LINES {
            let drain = self.current_block_lines.len() - MAX_CURRENT_LINES;
            self.current_block_lines.drain(0..drain);
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

    /// OSC 133;D;code — store exit code for the block being built.
    fn on_exit_code(&mut self, code: i32) {
        self.pending_exit_code = Some(code);
    }

    /// OSC 133;A — commit the current block and start a new one.
    fn on_prompt_start(&mut self) {
        if !self.current_line.is_empty() {
            let line = std::mem::replace(&mut self.current_line, TermLine::new());
            self.current_block_lines.push(line);
            self.cursor_col = 0;
        }
        if !self.current_block_lines.is_empty() {
            let exit_code = self.pending_exit_code.unwrap_or(0);
            self.blocks.push(TermBlock {
                id: self.current_block_id,
                lines: std::mem::take(&mut self.current_block_lines),
                exit_code,
            });
            if self.blocks.len() > MAX_BLOCKS {
                let drain = self.blocks.len() - MAX_BLOCKS;
                self.blocks.drain(0..drain);
            }
            self.current_block_id = self.next_id;
            self.next_id += 1;
        }
        self.pending_exit_code = None;
    }

    fn handle_sgr(&mut self, params: &Params) {
        let mut iter = params.iter();
        while let Some(param) = iter.next() {
            let code = param.first().copied().unwrap_or(0);
            match code {
                0 => self.reset_attrs(),
                1 => self.cur_bold = true,
                2..=4 => {}
                22 => self.cur_bold = false,
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
                                let r =
                                    iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                let g =
                                    iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                let b =
                                    iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
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
                                    self.cur_bg =
                                        TermColor::Indexed(idx.first().copied().unwrap_or(0) as u8);
                                }
                            }
                            2 => {
                                let r =
                                    iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                let g =
                                    iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                                let b =
                                    iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
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

    fn csi_dispatch(
        &mut self,
        params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        if let Ok(mut state) = self.state.lock() {
            match action {
                'm' => state.handle_sgr(params),
                'J' => {
                    let p = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(0);
                    if (p == 2 || p == 3) && !state.current_line.is_empty() {
                        let line = std::mem::replace(&mut state.current_line, TermLine::new());
                        state.current_block_lines.push(line);
                    }
                }
                'K' => {
                    let p = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(0);
                    if p == 0 {
                        state.current_line = TermLine::new();
                    }
                }
                'A' => {
                    let n = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(1)
                        .max(1) as usize;
                    if !state.current_line.is_empty() {
                        let line = std::mem::replace(&mut state.current_line, TermLine::new());
                        state.current_block_lines.push(line);
                    }
                    let len = state.current_block_lines.len();
                    state.current_block_lines.truncate(len.saturating_sub(n));
                }
                'H' | 'f' => {
                    let row = params
                        .iter()
                        .next()
                        .and_then(|p| p.first().copied())
                        .unwrap_or(1);
                    if row == 1 && !state.current_line.is_empty() {
                        let line = std::mem::replace(&mut state.current_line, TermLine::new());
                        state.current_block_lines.push(line);
                    }
                }
                _ => {}
            }
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }
        // OSC 7: working directory notification
        // Format: \e]7;file://hostname/path\a
        if params[0] == b"7" && params.len() > 1 {
            let url = String::from_utf8_lossy(params[1]);
            if let Some(path) = url.strip_prefix("file://") {
                let path = if let Some(p) = path.split_once('/').map(|x| x.1) {
                    format!("/{}", p)
                } else {
                    path.to_string()
                };
                if let Ok(mut s) = self.state.lock() {
                    s.cwd = path;
                }
            }
        }

        // OSC 133: shell integration (FinalTerm / VSCode protocol)
        // A = prompt start, B = prompt end, C = command executing, D;code = done
        if params[0] == b"133" && params.len() > 1 {
            let marker = String::from_utf8_lossy(params[1]);
            if marker.starts_with('A') {
                if let Ok(mut s) = self.state.lock() {
                    s.on_prompt_start();
                }
            } else if marker.starts_with("D") {
                // D;exit_code — parse exit code after the semicolon
                let code = marker
                    .strip_prefix("D;")
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(0);
                if let Ok(mut s) = self.state.lock() {
                    s.on_exit_code(code);
                }
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
            Backspace => {
                if alt {
                    vec![0x1b, 0x7f] // ESC + DEL = word backward delete
                } else {
                    vec![0x7f] // normal backspace
                }
            }
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
                if ctrl {
                    b"\x1b[1;5H".to_vec()
                } else {
                    b"\x1b[H".to_vec()
                }
            }
            End => {
                if ctrl {
                    b"\x1b[1;5F".to_vec()
                } else {
                    b"\x1b[F".to_vec()
                }
            }
            ArrowUp => {
                if ctrl {
                    b"\x1b[1;5A".to_vec()
                } else {
                    b"\x1b[A".to_vec()
                }
            }
            ArrowDown => {
                if ctrl {
                    b"\x1b[1;5B".to_vec()
                } else {
                    b"\x1b[B".to_vec()
                }
            }
            ArrowRight => {
                if ctrl {
                    b"\x1b[1;5C".to_vec()
                } else {
                    b"\x1b[C".to_vec()
                }
            }
            ArrowLeft => {
                if ctrl {
                    b"\x1b[1;5D".to_vec()
                } else {
                    b"\x1b[D".to_vec()
                }
            }
            PageUp => b"\x1b[5~".to_vec(),
            PageDown => b"\x1b[6~".to_vec(),
            F1 => b"\x1bOP".to_vec(),
            F2 => b"\x1bOQ".to_vec(),
            F3 => b"\x1bOR".to_vec(),
            F4 => b"\x1bOS".to_vec(),
            F5 => b"\x1b[15~".to_vec(),
            F6 => b"\x1b[17~".to_vec(),
            F7 => b"\x1b[18~".to_vec(),
            F8 => b"\x1b[19~".to_vec(),
            F9 => b"\x1b[20~".to_vec(),
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
                        let ctrl_byte = (c.to_ascii_uppercase() as u8)
                            .wrapping_sub(b'A')
                            .wrapping_add(1);
                        return vec![ctrl_byte];
                    }
                    // Special Ctrl combos
                    match c {
                        '[' => return b"\x1b".to_vec(), // Ctrl+[ = ESC
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

const SHELLS: &[&str] = &["bash", "zsh", "fish", "sh"];
/// Maximum lines rendered at once — keeps the dyn_stack fast.
const MAX_RENDER_LINES: usize = 500;

fn build_line_layout(
    line: &TermLine,
    default_fg: Color,
    _default_bg: Color,
    font_size: f32,
) -> TextLayout {
    let plain = line.plain_text();
    let fonts = [
        FamilyOwned::Name("JetBrains Mono".to_string()),
        FamilyOwned::Name("Fira Code".to_string()),
        FamilyOwned::Name("Cascadia Code".to_string()),
        FamilyOwned::Monospace,
    ];
    let default_attrs = Attrs::new()
        .font_size(font_size)
        .color(default_fg)
        .family(&fonts);
    let mut attrs_list = AttrsList::new(default_attrs);

    let mut byte_offset: usize = 0;
    for seg in &line.segments {
        let seg_len = seg.text.len();
        if seg_len == 0 {
            continue;
        }
        let start = byte_offset;
        let end = byte_offset + seg_len;
        byte_offset = end;

        let fg = seg.fg.to_floem_color(default_fg);
        let mut span_attrs = Attrs::new().font_size(font_size).color(fg).family(&fonts);
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
/// `clear_nonce`: when incremented, sends Ctrl+L to the PTY to clear the screen.
/// `shell`: the shell binary name or path to launch (e.g. "bash", "zsh").
/// `cwd_out`: signal that receives the current working directory via OSC 7.
/// `term_font_size`: reactive font size (8..32).
/// `find_open`: whether the find bar is visible.
/// `find_query`: current find query string.
/// Shared PTY writer type — Arc so it can be cloned and shared with callers.
type SharedPtyWriter = Arc<Mutex<Option<Box<dyn Write + Send>>>>;

/// One independent PTY terminal session.
/// `pty_writer_out`: if `Some`, will be set to `Arc::clone` of this terminal's PTY
/// writer once the PTY is ready — callers can then write bytes directly to the shell
/// (Feature 3 — Run in Terminal).
/// `prompt_positions_out`: if `Some`, receives live-updated prompt line positions from
/// OSC 133;A markers so the caller can implement prev/next command navigation (Feature 2).
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
    pty_writer_out: Option<RwSignal<Option<SharedPtyWriter>>>,
    prompt_positions_out: Option<RwSignal<Vec<usize>>>,
) -> impl IntoView {
    // ── Shared VTE state ──────────────────────────────────────────────────
    let term_state: Arc<Mutex<TermState>> = Arc::new(Mutex::new(TermState::new()));
    let pty_writer: SharedPtyWriter = Arc::new(Mutex::new(None));
    // Keep the master PTY handle alive so we can resize it when the view dimensions change.
    let pty_master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>> = Arc::new(Mutex::new(None));

    // Expose pty_writer Arc to caller (Feature 3: Run in Terminal).
    // The Arc is shared — the background PTY thread fills Option<Box<dyn Write+Send>> inside it.
    // The caller just clones the Arc and writes to it after the PTY is ready.
    if let Some(out_sig) = pty_writer_out {
        out_sig.set(Some(Arc::clone(&pty_writer)));
    }

    // ── Update channel: reader thread → reactive signal ───────────────────
    // Coalesced update channel: at most one pending repaint signal at a time.
    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let update_signal = create_signal_from_channel(update_rx);

    // ── Reactive buffers ──────────────────────────────────────────────────
    // completed_blocks: stable list of finished blocks (exit_code known).
    // Key is block id — dyn_stack creates each block view exactly once.
    let completed_blocks: RwSignal<Vec<(u64, Vec<TermLine>, i32)>> = create_rw_signal(vec![]);
    // live_lines: lines of the current in-progress block. Updates every keystroke.
    let live_lines: RwSignal<Vec<TermLine>> = create_rw_signal(vec![]);
    // Increment on every update so auto-scroll can detect new output
    let line_version: RwSignal<u64> = create_rw_signal(0);
    // Cursor column position for rendering the cursor block
    let cursor_col_sig: RwSignal<usize> = create_rw_signal(0usize);
    // Throttle expensive UI syncs under high PTY throughput.
    let last_ui_sync_at: RwSignal<Option<Instant>> = create_rw_signal(None);

    // ── Spawn PTY thread ──────────────────────────────────────────────────
    {
        let term_state_t = Arc::clone(&term_state);
        let pty_writer_t = Arc::clone(&pty_writer);
        let pty_master_t = Arc::clone(&pty_master);

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
                        s.current_block_lines.push(err);
                    }
                    let _ = update_tx.try_send(());
                    return;
                }
            };

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
                        s.current_block_lines.push(err);
                    }
                    let _ = update_tx.try_send(());
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

            // Inject PROMPT_COMMAND for OSC 7 (cwd) and OSC 133;A (shell integration) tracking
            {
                let pty_w2 = Arc::clone(&pty_writer_t);
                thread::spawn(move || {
                    thread::sleep(std::time::Duration::from_millis(300));
                    if let Ok(mut guard) = pty_w2.lock() {
                        if let Some(ref mut w) = *guard {
                            // OSC 133;D;$? = exit code of last cmd, then A = prompt start,
                            // then OSC 7 = cwd. D fires before A so each block gets its code.
                            let cmd = "export PROMPT_COMMAND='printf \"\\033]133;D;$?\\007\\033]133;A\\007\\033]7;file://${HOSTNAME}${PWD}\\007\"'\n";
                            let _ = w.write_all(cmd.as_bytes());
                            let _ = w.flush();
                        }
                    }
                });
            }

            let mut reader = match pair.master.try_clone_reader() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("PTY clone_reader error: {e}");
                    return;
                }
            };

            // Store master handle for resize calls from the UI thread.
            if let Ok(mut guard) = pty_master_t.lock() {
                *guard = Some(pair.master);
            }

            let mut child = child;
            let mut parser = vte::Parser::new();
            let mut performer = VtePerformer {
                state: Arc::clone(&term_state_t),
            };
            let mut buf = [0u8; term_consts::READ_BUFFER_SIZE];

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        for &byte in &buf[..n] {
                            parser.advance(&mut performer, byte);
                        }
                        let _ = update_tx.try_send(());
                    }
                    Err(_) => break,
                }
            }
            let _ = child.wait();

            if let Ok(mut s) = term_state_t.lock() {
                if !s.current_line.is_empty() {
                    let line = std::mem::replace(&mut s.current_line, TermLine::new());
                    s.current_block_lines.push(line);
                }
            }
            let _ = update_tx.try_send(());
        });
    }

    // ── Sync thread state → reactive signals ─────────────────────────────
    {
        let term_state_e = Arc::clone(&term_state);
        create_effect(move |_| {
            update_signal.get();
            let now = Instant::now();
            if let Some(prev) = last_ui_sync_at.get_untracked() {
                if now.duration_since(prev) < Duration::from_millis(16) {
                    return;
                }
            }
            last_ui_sync_at.set(Some(now));
            if let Ok(ts) = term_state_e.lock() {
                completed_blocks.set(
                    ts.blocks
                        .iter()
                        .map(|b| (b.id, b.lines.clone(), b.exit_code))
                        .collect(),
                );
                let mut live = ts.current_block_lines.clone();
                if !ts.current_line.is_empty() {
                    live.push(ts.current_line.clone());
                }
                live_lines.set(live);
                line_version.update(|v| *v += 1);
                cursor_col_sig.set(ts.cursor_col);
                // Propagate block count as prompt positions for prev/next nav buttons.
                if let Some(pp_sig) = prompt_positions_out {
                    pp_sig.set((0..ts.blocks.len()).collect());
                }
            }
        });
    }

    // ── Sync cwd from OSC 7 → cwd_out signal ─────────────────────────────
    {
        let term_state_cwd = Arc::clone(&term_state);
        create_effect(move |_| {
            update_signal.get();
            if let Ok(s) = term_state_cwd.lock() {
                let cwd = s.cwd.clone();
                if !cwd.is_empty() {
                    cwd_out.set(cwd);
                }
            }
        });
    }

    // ── PTY send helper ───────────────────────────────────────────────────
    let pty_writer_send = Arc::clone(&pty_writer);
    let send_to_pty = move |data: Vec<u8>| {
        if data.is_empty() {
            return;
        }
        if let Ok(mut guard) = pty_writer_send.lock() {
            if let Some(ref mut w) = *guard {
                let _ = w.write_all(&data);
                let _ = w.flush();
            }
        }
    };

    // ── Clear terminal effect (Ctrl+L → clears screen) ───────────────────
    {
        let pty_w = Arc::clone(&pty_writer);
        let last_clear = create_rw_signal(0u64);
        create_effect(move |_| {
            let n = clear_nonce.get();
            if n == 0 || n == last_clear.get_untracked() {
                return;
            }
            last_clear.set(n);
            if let Ok(mut guard) = pty_w.lock() {
                if let Some(ref mut w) = *guard {
                    // Ctrl+L (form feed) — clears terminal in bash/zsh/fish/pwsh
                    let _ = w.write_all(b"\x0c");
                    let _ = w.flush();
                }
            }
        });
    }

    // ── Focus state ───────────────────────────────────────────────────────
    let is_focused = create_rw_signal(false);

    // ── Helper: render a single TermLine as a rich_text row ──────────────
    let line_row = move |tl: TermLine| {
        let segments = tl.segments.clone();
        let init = {
            let t = theme.get_untracked();
            let p = &t.palette;
            let fs = term_font_size.get_untracked() as f32;
            if segments.is_empty() {
                let mut lay = TextLayout::new();
                lay.set_text(
                    " ",
                    AttrsList::new(
                        Attrs::new()
                            .font_size(fs)
                            .color(p.text_primary)
                            .family(&[FamilyOwned::Monospace]),
                    ),
                    None,
                );
                lay
            } else {
                build_line_layout(&tl, p.text_primary, p.bg_base, fs)
            }
        };
        let layout_sig: RwSignal<TextLayout> = create_rw_signal(init);
        create_effect(move |_| {
            let t = theme.get();
            let p = &t.palette;
            let fs = term_font_size.get() as f32;
            let new_lay = if segments.is_empty() {
                let mut lay = TextLayout::new();
                lay.set_text(
                    " ",
                    AttrsList::new(
                        Attrs::new()
                            .font_size(fs)
                            .color(p.text_primary)
                            .family(&[FamilyOwned::Monospace]),
                    ),
                    None,
                );
                lay
            } else {
                build_line_layout(
                    &TermLine {
                        segments: segments.clone(),
                    },
                    p.text_primary,
                    p.bg_base,
                    fs,
                )
            };
            layout_sig.set(new_lay);
        });
        container(floem::views::rich_text(move || layout_sig.get()).style(|s| s.width_full()))
            .style(|s| s.padding_horiz(8.0).padding_vert(1.0).width_full())
    };

    // ── Completed blocks (one collapsible card per shell command) ─────────
    let blocks_view = {
        let state_b = state.clone();
        dyn_stack(
            move || completed_blocks.get(),
            |(id, _, _): &(u64, Vec<TermLine>, i32)| *id,
            move |(id, lines, exit_code): (u64, Vec<TermLine>, i32)| {
                let _ = id; // used as key only
                let collapsed = create_rw_signal(lines.len() > 60);
                let hovered = create_rw_signal(false);

                // Split: first line = prompt/command header; rest = output
                let header_line = lines.first().cloned().unwrap_or_else(TermLine::new);
                let output_lines: Vec<TermLine> = lines.into_iter().skip(1).collect();
                let has_output = !output_lines.is_empty();

                // ── Exit badge ──────────────────────────────────────────
                let badge = container(
                    label(move || {
                        if exit_code == 0 {
                            " ✓ ".to_string()
                        } else {
                            format!(" ✗ {} ", exit_code)
                        }
                    })
                    .style(move |s| {
                        s.font_size(10.0)
                            .color(if exit_code == 0 {
                                Color::from_rgb8(80, 200, 80)
                            } else {
                                theme.get().palette.error
                            })
                            .font_weight(Weight::BOLD)
                    }),
                )
                .style(move |s| {
                    let bg = if exit_code == 0 {
                        Color::from_rgba8(80, 200, 80, 35)
                    } else {
                        theme.get().palette.error.with_alpha(0.15)
                    };
                    s.padding_horiz(4.0)
                        .padding_vert(1.0)
                        .border_radius(3.0)
                        .margin_right(6.0)
                        .background(bg)
                });

                // ── Header text (prompt + command) ──────────────────────
                let header_init = {
                    let t = theme.get_untracked();
                    build_line_layout(
                        &header_line,
                        t.palette.text_secondary,
                        t.palette.bg_base,
                        term_font_size.get_untracked() as f32,
                    )
                };
                let header_sig: RwSignal<TextLayout> = create_rw_signal(header_init);
                let hl2 = header_line.clone();
                create_effect(move |_| {
                    let t = theme.get();
                    let fs = term_font_size.get() as f32;
                    header_sig.set(build_line_layout(
                        &hl2,
                        t.palette.text_secondary,
                        t.palette.bg_base,
                        fs,
                    ));
                });
                let header_text = floem::views::rich_text(move || header_sig.get())
                    .style(|s| s.flex_grow(1.0).min_width(0.0));

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
                let lines_ai: String = {
                    let mut s = header_line.plain_text();
                    s.push('\n');
                    for l in &output_lines {
                        s.push_str(&l.plain_text());
                        s.push('\n');
                    }
                    s.truncate(4000);
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
                            format!("Explain this terminal output:\n\n```\n{}\n```", lines_ai);
                        state_ai.ai.pending_chat_inject.set(Some(prompt));
                        state_ai.workbench.show_right_panel.set(true);
                    });

                // ── Header row ──────────────────────────────────────────
                let header_row = container(
                    stack((badge, header_text, toggle, ai_btn))
                        .style(|s| s.flex_row().items_center().width_full()),
                )
                .style(move |s| {
                    let p = theme.get().palette;
                    s.width_full()
                        .min_height(28.0)
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

                // ── Output lines (collapsible) ──────────────────────────
                let ol_segs: Vec<Vec<TermSegment>> =
                    output_lines.iter().map(|l| l.segments.clone()).collect();
                let output_rows = dyn_stack(
                    move || ol_segs.clone().into_iter().enumerate().collect::<Vec<_>>(),
                    |(i, _): &(usize, Vec<TermSegment>)| *i,
                    move |(_, segs): (usize, Vec<TermSegment>)| {
                        line_row(TermLine { segments: segs })
                    },
                )
                .style(move |s| {
                    s.flex_col()
                        .width_full()
                        .apply_if(collapsed.get(), |s| s.display(Display::None))
                });

                // ── Block card ──────────────────────────────────────────
                container(stack((header_row, output_rows)).style(|s| s.flex_col().width_full()))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.width_full()
                            .border_bottom(1.0)
                            .border_color(p.border.with_alpha(0.25))
                            .margin_bottom(1.0)
                    })
            },
        )
        .style(|s| s.flex_col().width_full())
    };

    // ── Live / current-block lines (no exit code yet) ─────────────────────
    let live_view = dyn_stack(
        move || {
            let all = live_lines.get();
            let n = all.len();
            all.into_iter()
                .enumerate()
                .skip(n.saturating_sub(MAX_RENDER_LINES))
                .collect::<Vec<_>>()
        },
        |(i, _): &(usize, TermLine)| *i,
        move |(_, tl): (usize, TermLine)| line_row(tl),
    )
    .style(|s| s.flex_col().width_full().padding_vert(4.0));

    let output_list = stack((blocks_view, live_view)).style(|s| s.flex_col().width_full());

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
            theme.get().palette.text_primary.with_alpha(0.7),
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
    let pty_master_resize = Arc::clone(&pty_master);
    let pty_master_font_resize = Arc::clone(&pty_master);

    // Track the last PTY size to avoid redundant resize calls.
    let last_pty_cols: RwSignal<u16> = create_rw_signal(220u16);
    let last_pty_rows: RwSignal<u16> = create_rw_signal(40u16);
    // Track the last known pixel dimensions so font-size changes can recompute cols/rows.
    let last_pixel_w: RwSignal<f64> = create_rw_signal(0.0_f64);
    let last_pixel_h: RwSignal<f64> = create_rw_signal(0.0_f64);

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
                            if let Ok(ts) = term_state_c.lock() {
                                let mut parts: Vec<String> = ts
                                    .blocks
                                    .iter()
                                    .flat_map(|b| b.lines.iter())
                                    .chain(ts.current_block_lines.iter())
                                    .map(|line| line.plain_text())
                                    .collect();
                                if !ts.current_line.is_empty() {
                                    parts.push(ts.current_line.plain_text());
                                }
                                let text = parts.join("\n");
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    let _ = clipboard.set_text(text);
                                }
                            }
                            return;
                        }

                        // Ctrl+Shift+= — zoom terminal font in
                        if ch.as_str() == "=" || ch.as_str() == "+" {
                            term_font_size.update(|v| *v = (*v + 1).min(32));
                            return;
                        }

                        // Ctrl+Shift+- — zoom terminal font out
                        if ch.as_str() == "-" || ch.as_str() == "_" {
                            term_font_size.update(|v| *v = (*v).saturating_sub(1).max(8));
                            return;
                        }
                    }
                }

                let bytes = key_to_pty_bytes(e);
                send_to_pty(bytes);
            }
        })
        .style(|s| s.flex_grow(1.0).min_height(0.0).width_full())
        // Resize PTY when this view's layout size changes.
        .on_resize(move |rect| {
            let font_sz = term_font_size.get_untracked() as f64;
            let char_w = font_sz * 0.60; // standard monospace width-to-height ratio
            let char_h = font_sz + 3.0;
            let pw = rect.width();
            let ph = rect.height();
            last_pixel_w.set(pw);
            last_pixel_h.set(ph);
            let cols = (pw / char_w).max(1.0) as u16;
            let rows = (ph / char_h).max(1.0) as u16;
            if cols == last_pty_cols.get_untracked() && rows == last_pty_rows.get_untracked() {
                return;
            }
            last_pty_cols.set(cols);
            last_pty_rows.set(rows);
            if let Ok(guard) = pty_master_resize.lock() {
                if let Some(ref master) = *guard {
                    let _ = master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: pw as u16,
                        pixel_height: ph as u16,
                    });
                }
            }
        });

    // Re-trigger PTY resize whenever the terminal font size changes.
    create_effect(move |_| {
        let font_sz = term_font_size.get() as f64; // subscribes to font size changes
        let pw = last_pixel_w.get_untracked();
        let ph = last_pixel_h.get_untracked();
        if pw == 0.0 || ph == 0.0 {
            return;
        }
        let char_w = font_sz * 0.60;
        let char_h = font_sz + 3.0;
        let cols = (pw / char_w).max(1.0) as u16;
        let rows = (ph / char_h).max(1.0) as u16;
        last_pty_cols.set(cols);
        last_pty_rows.set(rows);
        if let Ok(guard) = pty_master_font_resize.lock() {
            if let Some(ref master) = *guard {
                let _ = master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: pw as u16,
                    pixel_height: ph as u16,
                });
            }
        }
    });

    // ── Find bar (shown when find_open is true) ───────────────────────────
    let find_results_count = create_memo(move |_| {
        let query = find_query.get();
        if query.is_empty() {
            return 0usize;
        }
        let query_lower = query.to_lowercase();
        let mut count = 0usize;
        for (_, block_lines, _) in completed_blocks.get() {
            count += block_lines
                .iter()
                .filter(|l| l.plain_text().to_lowercase().contains(&query_lower))
                .count();
        }
        count += live_lines
            .get()
            .iter()
            .filter(|l| l.plain_text().to_lowercase().contains(&query_lower))
            .count();
        count
    });

    let find_bar = container(
        stack((
            text_input(find_query).style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(12.0)
                    .width(200.0)
                    .padding(4.0)
                    .background(p.bg_elevated)
                    .color(p.text_primary)
                    .border(1.0)
                    .border_color(p.accent)
                    .border_radius(3.0)
            }),
            label(move || {
                let count = find_results_count.get();
                format!("  {} result{}", count, if count == 1 { "" } else { "s" })
            })
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(11.0).color(p.text_muted).padding_horiz(8.0)
            }),
            container(label(|| "✕"))
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.font_size(13.0)
                        .color(p.text_muted)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .cursor(CursorStyle::Pointer)
                        .hover(|s| s.background(p.bg_elevated).color(p.text_primary))
                })
                .on_click_stop(move |_| find_open.set(false)),
        ))
        .style(|s| {
            s.flex_row()
                .items_center()
                .padding_horiz(8.0)
                .padding_vert(4.0)
        }),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.width_full()
            .background(p.bg_panel)
            .border_bottom(1.0)
            .border_color(p.border)
            .apply_if(!find_open.get(), |s| s.display(Display::None))
    });

    // ── Assemble — find bar + output area ────────────────────────────────
    stack((find_bar, output_area)).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.flex_col()
            .flex_grow(1.0)
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

    // Feature 2: command marker positions from OSC 133;A in the active terminal.
    // Each entry is a line index where a new prompt started.
    let prompt_positions: RwSignal<Vec<usize>> = create_rw_signal(Vec::new());
    // Current position in the command-marker navigation list (for prev/next jumps).
    let cmd_marker_idx: RwSignal<usize> = create_rw_signal(0usize);
    // Target line to scroll to (None = no pending scroll).
    // Note: wire to actual scroll-to-line once Floem exposes stable scroll API.
    let _cmd_scroll_target: RwSignal<Option<usize>> = create_rw_signal(None);

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
                    eprintln!("[PhazeAI] AI cmd error: {e}");
                    ai_cmd_open.set(false);
                }
            }
        });
    }

    // Each entry: (id, name_signal, clear_nonce_signal, shell_str, cwd_signal,
    //              pty_writer_signal, prompt_positions_signal)
    // pty_writer_signal: receives the PTY writer Arc once single_terminal initialises
    // prompt_positions_signal: receives OSC 133;A positions from that terminal
    type TabEntry = (
        usize,
        RwSignal<String>,
        RwSignal<u64>,
        String,
        RwSignal<String>,
        RwSignal<Option<SharedPtyWriter>>,
        RwSignal<Vec<usize>>,
    );
    let tab_data: RwSignal<Vec<TabEntry>> = create_rw_signal(vec![(
        1usize,
        create_rw_signal("bash".to_string()),
        create_rw_signal(0u64),
        "bash".to_string(),
        create_rw_signal(String::new()),
        create_rw_signal(None::<SharedPtyWriter>),
        create_rw_signal(Vec::<usize>::new()),
    )]);
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
                tab_data.update(|data| {
                    data.push((
                        id,
                        create_rw_signal(shell_name.clone()),
                        create_rw_signal(0u64),
                        shell_name,
                        create_rw_signal(String::new()),
                        create_rw_signal(None::<SharedPtyWriter>),
                        create_rw_signal(Vec::<usize>::new()),
                    ))
                });
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
            if let Some((_, _, _, _, _, pw_sig, pp_sig)) = data.iter().find(|(tid, ..)| *tid == id)
            {
                active_pty_writer.set(pw_sig.get());
                prompt_positions.set(pp_sig.get());
            }
        });
    }
    // Which tab is currently being renamed (None = none)
    let editing_tab: RwSignal<Option<usize>> = create_rw_signal(None);
    let rename_text: RwSignal<String> = create_rw_signal(String::new());

    // ── Tab bar ───────────────────────────────────────────────────────────
    let tab_bar = stack((
        // Scrollable row of tabs
        dyn_stack(
            move || {
                tab_data
                    .get()
                    .into_iter()
                    .map(|(id, ns, cs, _sh, cwd, _pw, _pp)| (id, ns, cs, cwd))
                    .collect::<Vec<_>>()
            },
            |(id, _, _, _)| *id,
            move |(id, name_sig, _clear_sig, cwd_sig)| {
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
                        tab_data.update(|data| data.retain(|(tid, _, _, _, _, _, _)| *tid != id));
                        if active_tab.get_untracked() == id {
                            if let Some((last, _, _, _, _, _, _)) = tab_data.get_untracked().last()
                            {
                                active_tab.set(*last);
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
            .on_click_stop(move |_| {
                let positions = prompt_positions.get_untracked();
                if positions.is_empty() {
                    return;
                }
                let cur = cmd_marker_idx.get_untracked();
                let new_idx = cur.saturating_sub(1);
                cmd_marker_idx.set(new_idx);
                // Note: scroll terminal to positions[new_idx] once Floem exposes stable scroll API
                let _target_line = positions.get(new_idx).copied();
            }),
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
            .on_click_stop(move |_| {
                let positions = prompt_positions.get_untracked();
                if positions.is_empty() {
                    return;
                }
                let cur = cmd_marker_idx.get_untracked();
                let new_idx = (cur + 1).min(positions.len().saturating_sub(1));
                cmd_marker_idx.set(new_idx);
                // Note: scroll terminal to positions[new_idx] once Floem exposes stable scroll API
                let _target_line = positions.get(new_idx).copied();
            }),
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
                if let Some((_, _, clear_sig, _, _, _, _)) = tab_data
                    .get_untracked()
                    .into_iter()
                    .find(|(tid, ..)| *tid == id)
                {
                    clear_sig.update(|v| *v += 1);
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
                tab_data.update(|data| {
                    data.push((
                        id,
                        create_rw_signal(shell_name.clone()),
                        create_rw_signal(0u64),
                        shell_name,
                        create_rw_signal(String::new()),
                        create_rw_signal(None::<SharedPtyWriter>),
                        create_rw_signal(Vec::<usize>::new()),
                    ))
                });
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
        None,
        None,
    );

    // ── Terminal instances (one per tab, hidden when not active) ──────────
    let instances = dyn_stack(
        move || {
            safe_get(tab_data, Vec::new())
                .into_iter()
                .map(|(id, _, cs, shell, cwd, pw_sig, pp_sig)| (id, cs, shell, cwd, pw_sig, pp_sig))
                .collect::<Vec<_>>()
        },
        |(id, _, _, _, _, _)| *id,
        move |(id, clear_sig, shell, cwd_sig, pw_sig, pp_sig)| {
            single_terminal(
                theme,
                clear_sig,
                shell,
                cwd_sig,
                state.clone(),
                term_font_size,
                term_find_open,
                term_find_query,
                ai_cmd_open,
                Some(pw_sig),
                Some(pp_sig),
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
                                let client = match settings.build_llm_client() {
                                    Ok(c) => c,
                                    Err(e) => { let _ = tx.send(Err(format!("LLM: {e}"))); return; }
                                };
                                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                                    Ok(r) => r,
                                    Err(e) => { let _ = tx.send(Err(format!("Runtime: {e}"))); return; }
                                };
                                rt.block_on(async move {
                                    use phazeai_core::{Agent, AgentEvent};
                                    let agent = Agent::new(client);
                                    let prompt = format!(
                                        "Generate a single bash/shell command that does the following: {desc}\n\
                                         Rules: respond with ONLY the command, no explanation, no markdown, no backticks."
                                    );
                                    let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
                                    let run_fut = agent.run_with_events(&prompt, agent_tx);
                                    let collect_fut = async move {
                                        let mut s = String::new();
                                        while let Some(ev) = agent_rx.recv().await {
                                            if let AgentEvent::TextDelta(d) = ev { s.push_str(&d); }
                                        }
                                        s
                                    };
                                    let (_, collected) = tokio::join!(run_fut, collect_fut);
                                    let cmd = collected.trim().trim_matches('`').to_string();
                                    let _ = tx.send(if cmd.is_empty() {
                                        Err("AI returned empty command".into())
                                    } else {
                                        Ok(cmd)
                                    });
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
