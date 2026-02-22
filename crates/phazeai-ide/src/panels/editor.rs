use egui::{self, Color32, FontId, Sense, Vec2, TextFormat, text::LayoutJob};
use lsp_types::Diagnostic;
use ropey::Rope;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;
use syntect::highlighting::{HighlightState, Highlighter, RangedHighlightIterator, ThemeSet};
use syntect::parsing::{ParseState, SyntaxSet};
use tree_sitter_highlight::{
    Highlight, HighlightConfiguration, HighlightEvent,
    Highlighter as TsHighlighter,
};

use crate::themes::ThemeColors;

// ── Tree-Sitter Highlight Engine ──────────────────────────────────────────

/// Highlight capture names in priority order.
/// Index matches the `Highlight` value returned by tree-sitter-highlight.
const TS_NAMES: &[&str] = &[
    "attribute",          // 0
    "comment",            // 1
    "constant",           // 2
    "constant.builtin",   // 3
    "constructor",        // 4
    "function",           // 5
    "function.builtin",   // 6
    "function.macro",     // 7
    "keyword",            // 8
    "module",             // 9
    "number",             // 10
    "operator",           // 11
    "property",           // 12
    "string",             // 13
    "string.escape",      // 14
    "type",               // 15
    "type.builtin",       // 16
    "variable",           // 17
    "variable.builtin",   // 18
    "variable.parameter", // 19
    "label",              // 20
    "punctuation",        // 21
    "punctuation.bracket",// 22
    "punctuation.delimiter", // 23
];

/// A byte-range highlight span: (start_byte, end_byte, highlight_index)
type TsSpan = (usize, usize, usize);

/// Engine wrapping tree-sitter-highlight for supported languages.
struct TsHighlightEngine {
    rust_config: HighlightConfiguration,
    highlighter: TsHighlighter,
}

impl TsHighlightEngine {
    fn new() -> Option<Self> {
        let mut rust_config = HighlightConfiguration::new(
            tree_sitter_rust::LANGUAGE.into(),
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            tree_sitter_rust::INJECTIONS_QUERY,
            "",
        )
        .ok()?;
        rust_config.configure(TS_NAMES);
        Some(Self {
            rust_config,
            highlighter: TsHighlighter::new(),
        })
    }

    /// Compute highlight spans for a Rust source buffer.
    /// Returns spans sorted by start_byte (tree-sitter guarantees this).
    fn compute_rust(&mut self, source: &[u8]) -> Vec<TsSpan> {
        let events = match self.highlighter.highlight(
            &self.rust_config,
            source,
            None,
            |_| None,
        ) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let mut spans = Vec::new();
        let mut stack: Vec<usize> = Vec::new();

        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(Highlight(idx))) => {
                    stack.push(idx);
                }
                Ok(HighlightEvent::Source { start, end }) => {
                    if end > start {
                        if let Some(&hl) = stack.last() {
                            spans.push((start, end, hl));
                        }
                    }
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    stack.pop();
                }
                Err(_) => break,
            }
        }
        spans
    }
}

/// Map a tree-sitter highlight index to a theme Color32.
fn ts_color(idx: usize, theme: &ThemeColors) -> Color32 {
    match idx {
        0  => Color32::from_rgb(210, 150, 250), // attribute → light purple
        1  => theme.comment,                     // comment
        2  => theme.number,                      // constant
        3  => theme.number,                      // constant.builtin
        4  => theme.function,                    // constructor
        5  => theme.function,                    // function
        6  => theme.function,                    // function.builtin
        7  => Color32::from_rgb(255, 160, 90),   // function.macro → orange
        8  => theme.keyword,                     // keyword
        9  => theme.type_name,                   // module
        10 => theme.number,                      // number
        11 => theme.text_secondary,              // operator
        12 => Color32::from_rgb(180, 220, 255),  // property → light blue
        13 => theme.string,                      // string
        14 => Color32::from_rgb(255, 200, 130),  // string.escape → yellow-orange
        15 => theme.type_name,                   // type
        16 => theme.type_name,                   // type.builtin
        17 => theme.text,                        // variable
        18 => theme.keyword,                     // variable.builtin
        19 => Color32::from_rgb(200, 230, 255),  // variable.parameter → pale blue
        20 => Color32::from_rgb(255, 200, 100),  // label → yellow
        21 => theme.text_muted,                  // punctuation
        22 => theme.text_muted,                  // punctuation.bracket
        23 => theme.text_muted,                  // punctuation.delimiter
        _  => theme.text,
    }
}

/// Highlight a single line using pre-computed tree-sitter spans.
/// `line_byte_start` is the byte offset of line_idx in the whole document.
fn highlight_line_ts(
    line_text: &str,
    line_byte_start: usize,
    ts_spans: &[TsSpan],
    theme: &ThemeColors,
    font_size: f32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    if line_text.is_empty() {
        job.append(" ", 0.0, TextFormat {
            font_id: FontId::monospace(font_size),
            color: theme.text,
            ..Default::default()
        });
        return job;
    }

    let line_byte_end = line_byte_start + line_text.len();

    // Find spans that overlap this line using binary search on start_byte
    let first = ts_spans.partition_point(|&(_, end, _)| end <= line_byte_start);
    let last = ts_spans.partition_point(|&(start, _, _)| start < line_byte_end);

    if first >= last {
        // No highlights for this line
        job.append(line_text, 0.0, TextFormat {
            font_id: FontId::monospace(font_size),
            color: theme.text,
            ..Default::default()
        });
        return job;
    }

    let mut cursor = 0usize; // char cursor within line_text

    for &(span_start, span_end, hl_idx) in &ts_spans[first..last] {
        // Clamp span to this line
        let rel_start = span_start.saturating_sub(line_byte_start).min(line_text.len());
        let rel_end = span_end.saturating_sub(line_byte_start).min(line_text.len());

        // Unhighlighted gap before this span
        if rel_start > cursor {
            let gap = &line_text[cursor..rel_start];
            job.append(gap, 0.0, TextFormat {
                font_id: FontId::monospace(font_size),
                color: theme.text,
                ..Default::default()
            });
        }

        if rel_end > rel_start {
            let span_text = &line_text[rel_start..rel_end];
            job.append(span_text, 0.0, TextFormat {
                font_id: FontId::monospace(font_size),
                color: ts_color(hl_idx, theme),
                ..Default::default()
            });
            cursor = rel_end;
        }
    }

    // Trailing unhighlighted text
    if cursor < line_text.len() {
        job.append(&line_text[cursor..], 0.0, TextFormat {
            font_id: FontId::monospace(font_size),
            color: theme.text,
            ..Default::default()
        });
    }

    if job.sections.is_empty() {
        job.append(line_text, 0.0, TextFormat {
            font_id: FontId::monospace(font_size),
            color: theme.text,
            ..Default::default()
        });
    }

    job
}

// ── Git Gutter ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum GitLineStatus {
    Added,
    Modified,
    Deleted,
}

/// Blame info for a single line.
#[derive(Clone)]
pub struct BlameInfo {
    pub short_hash: String,
    pub author: String,
    pub date: String,
    pub summary: String,
}

// ── Text Position ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TextPosition {
    pub line: usize,
    pub col: usize,
}

impl TextPosition {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
    pub fn zero() -> Self {
        Self { line: 0, col: 0 }
    }
}

impl PartialOrd for TextPosition {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TextPosition {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.line.cmp(&other.line).then(self.col.cmp(&other.col))
    }
}

// ── Selection ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Selection {
    /// Where selection started (fixed end)
    pub anchor: TextPosition,
    /// Where the cursor is (moving end)
    pub cursor: TextPosition,
}

impl Selection {
    pub fn new(pos: TextPosition) -> Self {
        Self { anchor: pos, cursor: pos }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Returns (start, end) in document order
    pub fn ordered(&self) -> (TextPosition, TextPosition) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    pub fn contains_line(&self, line: usize) -> bool {
        let (start, end) = self.ordered();
        line >= start.line && line <= end.line
    }
}

// ── Undo System (Rope-based, efficient) ──────────────────────────────────

const MAX_UNDO_HISTORY: usize = 200;
const EDIT_GROUP_SIZE: usize = 10;

#[derive(Clone)]
struct UndoEntry {
    /// Rope clone is O(log n) — structural sharing, not a full copy
    rope: Rope,
    cursor: TextPosition,
    selection: Option<Selection>,
}

struct UndoStack {
    entries: Vec<UndoEntry>,
    position: usize,
}

impl UndoStack {
    fn new(rope: &Rope) -> Self {
        Self {
            entries: vec![UndoEntry {
                rope: rope.clone(),
                cursor: TextPosition::zero(),
                selection: None,
            }],
            position: 0,
        }
    }

    fn push(&mut self, rope: &Rope, cursor: TextPosition, selection: Option<Selection>) {
        self.entries.truncate(self.position + 1);
        self.entries.push(UndoEntry { rope: rope.clone(), cursor, selection });
        self.position = self.entries.len() - 1;
        if self.entries.len() > MAX_UNDO_HISTORY {
            self.entries.remove(0);
            self.position = self.entries.len() - 1;
        }
    }

    fn undo(&mut self) -> Option<&UndoEntry> {
        if self.position > 0 {
            self.position -= 1;
            Some(&self.entries[self.position])
        } else {
            None
        }
    }

    fn redo(&mut self) -> Option<&UndoEntry> {
        if self.position + 1 < self.entries.len() {
            self.position += 1;
            Some(&self.entries[self.position])
        } else {
            None
        }
    }
}

// ── Bracket Matching + Code Folding helpers ──────────────────────────────

fn char_idx_to_pos(rope: &Rope, char_idx: usize) -> TextPosition {
    let safe_idx = char_idx.min(rope.len_chars().saturating_sub(1));
    let line = rope.char_to_line(safe_idx);
    let line_start = rope.line_to_char(line);
    TextPosition { line, col: safe_idx - line_start }
}

/// Scan the rope from the cursor position to find the matching bracket pair.
/// Returns (open_pos, close_pos) or None.
fn find_bracket_match(rope: &Rope, cursor: TextPosition) -> Option<(TextPosition, TextPosition)> {
    let total_chars = rope.len_chars();
    if total_chars == 0 { return None; }

    let cursor_idx = {
        if cursor.line >= rope.len_lines() { return None; }
        let line_start = rope.line_to_char(cursor.line);
        let line = rope.line(cursor.line);
        let line_len = line.len_chars();
        let max_col = if line_len > 0 && line.char(line_len - 1) == '\n' { line_len - 1 } else { line_len };
        line_start + cursor.col.min(max_col)
    };

    let check_indices: &[usize] = if cursor_idx > 0 {
        &[cursor_idx, cursor_idx - 1][..]
    } else {
        &[cursor_idx][..]
    };

    for &check_idx in check_indices {
        if check_idx >= total_chars { continue; }
        let ch = rope.char(check_idx);
        let (open, close, forward) = match ch {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            '}' => ('{', '}', false),
            _ => continue,
        };
        let start_pos = char_idx_to_pos(rope, check_idx);
        // Limit scan to 50k chars to avoid freezing on huge files
        if forward {
            let mut depth = 0i32;
            let end = (check_idx + 50_000).min(total_chars);
            for idx in check_idx..end {
                let c = rope.char(idx);
                if c == open { depth += 1; }
                else if c == close {
                    depth -= 1;
                    if depth == 0 { return Some((start_pos, char_idx_to_pos(rope, idx))); }
                }
            }
        } else {
            let mut depth = 0i32;
            let start = check_idx.saturating_sub(50_000);
            for idx in (start..=check_idx).rev() {
                let c = rope.char(idx);
                if c == close { depth += 1; }
                else if c == open {
                    depth -= 1;
                    if depth == 0 { return Some((char_idx_to_pos(rope, idx), start_pos)); }
                }
            }
        }
        break; // Only try the first bracket found at cursor
    }
    None
}

/// Scan forward from start_line to find the line containing the matching closing brace.
fn find_fold_end(rope: &Rope, start_line: usize) -> Option<usize> {
    let total_lines = rope.len_lines();
    if start_line >= total_lines { return None; }
    let start_text = rope.line(start_line).to_string();
    let trimmed = start_text.trim_end();
    let (open, close) = if trimmed.ends_with('{') { ('{', '}') }
        else if trimmed.ends_with('(') { ('(', ')') }
        else if trimmed.ends_with('[') { ('[', ']') }
        else { return None; };

    let mut depth: i32 = 0;
    for line_idx in start_line..total_lines.min(start_line + 5000) {
        for ch in rope.line(line_idx).chars() {
            if ch == open { depth += 1; }
            else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return if line_idx > start_line { Some(line_idx) } else { None };
                }
            }
        }
    }
    None
}

// ── Syntax Highlight Cache ───────────────────────────────────────────────

/// Cached highlight state at the START of each line.
/// Enables stateful multi-line syntax highlighting (block comments, strings, etc.)
#[derive(Clone)]
struct LineHighlightState {
    parse_state: ParseState,
    highlight_state: HighlightState,
}

struct HighlightCache {
    /// State at the beginning of line[i]. `None` = needs recompute.
    states: Vec<Option<LineHighlightState>>,
    /// First dirty line (invalidate from here onward)
    dirty_from: usize,
    /// Per-line LayoutJob cache: (raw_line_text, rendered_job).
    /// Avoids re-running syntect every frame for unchanged lines.
    job_cache: Vec<Option<(String, LayoutJob)>>,
}

impl HighlightCache {
    fn new() -> Self {
        Self {
            states: Vec::new(),
            dirty_from: 0,
            job_cache: Vec::new(),
        }
    }

    fn invalidate_from(&mut self, line: usize) {
        self.dirty_from = self.dirty_from.min(line);
        // Clear cached states and jobs from that line forward
        for i in line..self.states.len() {
            self.states[i] = None;
        }
        for i in line..self.job_cache.len() {
            self.job_cache[i] = None;
        }
    }

    fn ensure_size(&mut self, num_lines: usize) {
        if self.states.len() < num_lines {
            self.states.resize(num_lines, None);
        }
        if self.job_cache.len() < num_lines {
            self.job_cache.resize_with(num_lines, || None);
        }
    }
}

// ── Editor Tab ───────────────────────────────────────────────────────────

pub struct EditorTab {
    pub path: Option<PathBuf>,
    pub title: String,
    pub rope: Rope,
    pub modified: bool,
    pub cursor: TextPosition,
    pub selection: Option<Selection>,
    pub language: String,
    undo_stack: UndoStack,
    edits_since_snapshot: usize,
    highlight_cache: HighlightCache,
    /// True when cursor changed this frame (triggers scroll)
    pub cursor_changed: bool,

    // Legacy accessors used by app.rs status bar
    pub cursor_line: usize,
    pub cursor_col: usize,

    /// Git gutter: line_idx (0-based) → status
    pub git_hunks: HashMap<usize, GitLineStatus>,
    pub last_git_check: Option<Instant>,

    /// Git blame cache: line_idx (0-based) → blame info (populated lazily)
    pub blame_cache: HashMap<usize, BlameInfo>,
    pub blame_loaded: bool,

    /// LSP diagnostics for this file (line_idx → list of diagnostics)
    pub diagnostics: HashMap<usize, Vec<Diagnostic>>,

    /// LSP document version counter (incremented on each change)
    pub lsp_version: i32,

    /// Multi-cursor: additional cursor positions beyond the primary cursor
    pub extra_cursors: Vec<TextPosition>,

    /// Column/block selection (Alt+Shift+drag): (anchor, cursor)
    pub block_selection: Option<(TextPosition, TextPosition)>,

    // ── LSP interactive features ──────────────────────────────────────────
    /// Last mouse-over position (line, character). Set each frame when hovering text.
    pub hover_pos: Option<(u32, u32)>,
    /// When hover_pos was last updated (for 500ms debounce).
    pub hover_start: Option<Instant>,
    /// Whether a hover request is in-flight (avoid duplicate requests).
    pub hover_pending: bool,
    /// Most recently received hover text (cleared when cursor moves).
    pub hover_text: Option<String>,
    /// Completions currently being displayed.
    pub completions: Vec<lsp_types::CompletionItem>,
    /// Whether the completion popup is visible.
    pub completion_visible: bool,
    /// Index into `completions` for the currently highlighted item.
    pub completion_selected: usize,

    /// Bracket match: the two positions of the matching bracket pair at cursor.
    pub bracket_match: Option<(TextPosition, TextPosition)>,
    /// Cursor position when bracket_match was last computed (skip recompute if unchanged).
    bracket_match_cursor: Option<TextPosition>,
    /// Code folding: maps fold-start line → fold-end line (inclusive).
    pub folded_lines: HashMap<usize, usize>,
    /// Cached set of lines hidden inside fold regions. Rebuilt only when fold_generation changes.
    fold_hidden_cache: HashSet<usize>,
    /// Monotonically incremented when folded_lines is modified.
    fold_generation: u64,
    fold_cache_generation: u64,

    /// Tree-sitter highlight spans (byte_start, byte_end, highlight_idx).
    /// Recomputed when `ts_computed_len` differs from `rope.len_bytes()`.
    pub ts_spans: Vec<TsSpan>,
    pub ts_computed_len: usize,
}

impl EditorTab {
    pub fn new_untitled() -> Self {
        let rope = Rope::new();
        Self {
            undo_stack: UndoStack::new(&rope),
            path: None,
            title: "Untitled".to_string(),
            rope,
            modified: false,
            cursor: TextPosition::zero(),
            selection: None,
            language: String::new(),
            edits_since_snapshot: 0,
            highlight_cache: HighlightCache::new(),
            cursor_changed: false,
            cursor_line: 0,
            cursor_col: 0,
            git_hunks: HashMap::new(),
            last_git_check: None,
            blame_cache: HashMap::new(),
            blame_loaded: false,
            diagnostics: HashMap::new(),
            lsp_version: 0,
            extra_cursors: Vec::new(),
            block_selection: None,
            hover_pos: None,
            hover_start: None,
            hover_pending: false,
            hover_text: None,
            completions: Vec::new(),
            completion_visible: false,
            completion_selected: 0,
            bracket_match: None,
            bracket_match_cursor: None,
            folded_lines: HashMap::new(),
            fold_hidden_cache: HashSet::new(),
            fold_generation: 0,
            fold_cache_generation: u64::MAX,
            ts_spans: Vec::new(),
            ts_computed_len: usize::MAX,
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
            undo_stack: UndoStack::new(&rope),
            path: Some(path),
            title,
            rope,
            modified: false,
            cursor: TextPosition::zero(),
            selection: None,
            language,
            edits_since_snapshot: 0,
            highlight_cache: HighlightCache::new(),
            cursor_changed: false,
            cursor_line: 0,
            cursor_col: 0,
            git_hunks: HashMap::new(),
            last_git_check: None,
            blame_cache: HashMap::new(),
            blame_loaded: false,
            diagnostics: HashMap::new(),
            lsp_version: 0,
            extra_cursors: Vec::new(),
            block_selection: None,
            hover_pos: None,
            hover_start: None,
            hover_pending: false,
            hover_text: None,
            completions: Vec::new(),
            completion_visible: false,
            completion_selected: 0,
            bracket_match: None,
            bracket_match_cursor: None,
            folded_lines: HashMap::new(),
            fold_hidden_cache: HashSet::new(),
            fold_generation: 0,
            fold_cache_generation: u64::MAX,
            ts_spans: Vec::new(),
            ts_computed_len: usize::MAX,
        })
    }

    pub fn save(&mut self) -> Result<(), std::io::Error> {
        if let Some(ref path) = self.path {
            let content: String = self.rope.to_string();
            std::fs::write(path, content)?;
            self.modified = false;
            // Invalidate blame so it refreshes after save (new commit info)
            self.blame_cache.clear();
            self.blame_loaded = false;
        }
        Ok(())
    }

    pub fn reload_from_disk(&mut self) {
        if let Some(ref path) = self.path {
            if let Ok(content) = std::fs::read_to_string(path) {
                self.rope = Rope::from_str(&content);
                self.modified = false;
                self.undo_stack = UndoStack::new(&self.rope);
                self.edits_since_snapshot = 0;
                self.highlight_cache.invalidate_from(0);
                // Invalidate blame cache so it refreshes after external edit
                self.blame_cache.clear();
                self.blame_loaded = false;
            }
        }
    }

    fn sync_legacy_cursor(&mut self) {
        self.cursor_line = self.cursor.line;
        self.cursor_col = self.cursor.col;
    }

    fn maybe_snapshot(&mut self) {
        self.edits_since_snapshot += 1;
        if self.edits_since_snapshot >= EDIT_GROUP_SIZE {
            self.force_snapshot();
        }
    }

    /// Apply a set of LSP TextEdits to this buffer (used for formatting).
    /// Edits must be sorted in reverse order (bottom-to-top) to keep offsets stable.
    pub fn apply_lsp_edits(&mut self, edits: &[lsp_types::TextEdit]) {
        self.force_snapshot();
        for edit in edits {
            let start_line = edit.range.start.line as usize;
            let start_col = edit.range.start.character as usize;
            let end_line = edit.range.end.line as usize;
            let end_col = edit.range.end.character as usize;
            if start_line >= self.rope.len_lines() { continue; }
            let start_idx = self.rope.line_to_char(start_line) + start_col;
            let end_idx = if end_line >= self.rope.len_lines() {
                self.rope.len_chars()
            } else {
                self.rope.line_to_char(end_line) + end_col
            };
            if start_idx <= end_idx && end_idx <= self.rope.len_chars() {
                self.rope.remove(start_idx..end_idx);
                self.rope.insert(start_idx, &edit.new_text);
            }
        }
        self.modified = true;
        self.highlight_cache.invalidate_from(0);
    }

    fn force_snapshot(&mut self) {
        let cur = self.cursor;
        let sel = self.selection.clone();
        self.undo_stack.push(&self.rope, cur, sel);
        self.edits_since_snapshot = 0;
    }

    /// Convert TextPosition to rope char index
    pub fn pos_to_char_idx(&self, pos: TextPosition) -> usize {
        if pos.line >= self.rope.len_lines() {
            return self.rope.len_chars();
        }
        let line_start = self.rope.line_to_char(pos.line);
        let line = self.rope.line(pos.line);
        let line_len = line.len_chars();
        // Don't include the trailing newline in the addressable range
        let max_col = if line_len > 0 && line.char(line_len - 1) == '\n' {
            line_len - 1
        } else {
            line_len
        };
        line_start + pos.col.min(max_col)
    }

    pub fn cursor_char_idx(&self) -> usize {
        self.pos_to_char_idx(self.cursor)
    }

    /// Clamp cursor column to valid range for its line
    fn clamp_cursor_col(&mut self) {
        let line = self.cursor.line.min(self.rope.len_lines().saturating_sub(1));
        self.cursor.line = line;
        if line < self.rope.len_lines() {
            let line_slice = self.rope.line(line);
            let line_len = line_slice.len_chars();
            let max_col = if line_len > 0 && line_slice.char(line_len - 1) == '\n' {
                line_len - 1
            } else {
                line_len
            };
            self.cursor.col = self.cursor.col.min(max_col);
        } else {
            self.cursor.col = 0;
        }
    }

    /// Get the selected char range (start_idx, end_idx) if selection is active
    pub fn selection_char_range(&self) -> Option<(usize, usize)> {
        if let Some(ref sel) = self.selection {
            if !sel.is_empty() {
                let (start, end) = sel.ordered();
                return Some((self.pos_to_char_idx(start), self.pos_to_char_idx(end)));
            }
        }
        None
    }

    /// Delete selection and place cursor at start
    fn delete_selection(&mut self) -> bool {
        if let Some((start_idx, end_idx)) = self.selection_char_range() {
            let (start_pos, _) = match self.selection.as_ref() {
                Some(s) => s.ordered(),
                None => return false,
            };
            self.rope.remove(start_idx..end_idx);
            self.cursor = start_pos;
            self.selection = None;
            self.modified = true;
            self.highlight_cache.invalidate_from(start_pos.line);
            return true;
        }
        false
    }

    /// Get selected text as a String
    pub fn selected_text(&self) -> Option<String> {
        let (start_idx, end_idx) = self.selection_char_range()?;
        Some(self.rope.slice(start_idx..end_idx).to_string())
    }

    pub fn insert_char(&mut self, ch: char) {
        // Delete selection first if any
        self.delete_selection();
        let idx = self.cursor_char_idx();
        self.rope.insert_char(idx, ch);
        let dirty_line = self.cursor.line;
        if ch == '\n' {
            self.cursor.line += 1;
            self.cursor.col = 0;
            self.force_snapshot();
        } else {
            self.cursor.col += 1;
            self.maybe_snapshot();
        }
        self.modified = true;
        self.cursor_changed = true;
        self.highlight_cache.invalidate_from(dirty_line);
        self.sync_legacy_cursor();
    }

    pub fn insert_str(&mut self, s: &str) {
        self.delete_selection();
        let idx = self.cursor_char_idx();
        let dirty_line = self.cursor.line;
        self.rope.insert(idx, s);
        // Advance cursor by char count
        for ch in s.chars() {
            if ch == '\n' {
                self.cursor.line += 1;
                self.cursor.col = 0;
            } else {
                self.cursor.col += 1;
            }
        }
        self.modified = true;
        self.cursor_changed = true;
        self.force_snapshot();
        self.highlight_cache.invalidate_from(dirty_line);
        self.sync_legacy_cursor();
    }

    pub fn delete_back(&mut self) {
        if self.delete_selection() {
            self.maybe_snapshot();
            self.sync_legacy_cursor();
            return;
        }
        let idx = self.cursor_char_idx();
        if idx == 0 {
            return;
        }
        let dirty_line = if self.cursor.col == 0 {
            self.cursor.line.saturating_sub(1)
        } else {
            self.cursor.line
        };
        if self.cursor.col == 0 && self.cursor.line > 0 {
            self.cursor.line -= 1;
            let line = self.rope.line(self.cursor.line);
            let line_len = line.len_chars();
            self.cursor.col = if line_len > 0 && line.char(line_len - 1) == '\n' {
                line_len - 1
            } else {
                line_len
            };
        } else if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
        self.rope.remove(idx - 1..idx);
        self.modified = true;
        self.cursor_changed = true;
        self.maybe_snapshot();
        self.highlight_cache.invalidate_from(dirty_line);
        self.sync_legacy_cursor();
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection() {
            self.maybe_snapshot();
            self.sync_legacy_cursor();
            return;
        }
        let idx = self.cursor_char_idx();
        if idx >= self.rope.len_chars() {
            return;
        }
        let dirty_line = self.cursor.line;
        self.rope.remove(idx..idx + 1);
        self.modified = true;
        self.cursor_changed = true;
        self.maybe_snapshot();
        self.highlight_cache.invalidate_from(dirty_line);
        self.clamp_cursor_col();
        self.sync_legacy_cursor();
    }

    /// Delete word backward (Ctrl+Backspace)
    pub fn delete_word_back(&mut self) {
        if self.delete_selection() {
            self.sync_legacy_cursor();
            return;
        }
        let start_idx = self.cursor_char_idx();
        let new_pos = self.word_start_before(self.cursor);
        let end_idx = start_idx;
        let new_idx = self.pos_to_char_idx(new_pos);
        if new_idx < end_idx {
            self.rope.remove(new_idx..end_idx);
            self.cursor = new_pos;
            self.modified = true;
            self.cursor_changed = true;
            self.force_snapshot();
            self.highlight_cache.invalidate_from(new_pos.line);
        }
        self.sync_legacy_cursor();
    }

    /// Delete word forward (Ctrl+Delete)
    pub fn delete_word_forward(&mut self) {
        if self.delete_selection() {
            self.sync_legacy_cursor();
            return;
        }
        let start_idx = self.cursor_char_idx();
        let new_pos = self.word_end_after(self.cursor);
        let end_idx = self.pos_to_char_idx(new_pos);
        if end_idx > start_idx {
            self.rope.remove(start_idx..end_idx);
            self.modified = true;
            self.cursor_changed = true;
            self.force_snapshot();
            self.highlight_cache.invalidate_from(self.cursor.line);
            self.clamp_cursor_col();
        }
        self.sync_legacy_cursor();
    }

    /// Find start of previous word
    fn word_start_before(&self, pos: TextPosition) -> TextPosition {
        let idx = self.pos_to_char_idx(pos);
        if idx == 0 {
            return TextPosition::zero();
        }
        let mut i = idx;
        // Skip whitespace
        while i > 0 {
            let ch = self.rope.char(i - 1);
            if !ch.is_whitespace() { break; }
            i -= 1;
        }
        // Skip word chars
        while i > 0 {
            let ch = self.rope.char(i - 1);
            if ch.is_whitespace() { break; }
            i -= 1;
        }
        self.char_idx_to_pos(i)
    }

    /// Find end of next word
    fn word_end_after(&self, pos: TextPosition) -> TextPosition {
        let idx = self.pos_to_char_idx(pos);
        let len = self.rope.len_chars();
        if idx >= len {
            return self.char_idx_to_pos(len);
        }
        let mut i = idx;
        // Skip whitespace
        while i < len {
            let ch = self.rope.char(i);
            if !ch.is_whitespace() { break; }
            i += 1;
        }
        // Skip word chars
        while i < len {
            let ch = self.rope.char(i);
            if ch.is_whitespace() { break; }
            i += 1;
        }
        self.char_idx_to_pos(i)
    }

    /// Move cursor one word left
    pub fn move_word_left(&mut self, extend_selection: bool) {
        let new_pos = self.word_start_before(self.cursor);
        self.move_cursor_to(new_pos, extend_selection);
    }

    /// Move cursor one word right
    pub fn move_word_right(&mut self, extend_selection: bool) {
        let new_pos = self.word_end_after(self.cursor);
        self.move_cursor_to(new_pos, extend_selection);
    }

    /// Convert char index to TextPosition
    pub fn char_idx_to_pos(&self, idx: usize) -> TextPosition {
        let idx = idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        let line_start = self.rope.line_to_char(line);
        TextPosition::new(line, idx - line_start)
    }

    /// Move cursor to position, optionally extending selection
    pub fn move_cursor_to(&mut self, pos: TextPosition, extend_selection: bool) {
        if extend_selection {
            if self.selection.is_none() {
                self.selection = Some(Selection::new(self.cursor));
            }
            if let Some(ref mut sel) = self.selection {
                sel.cursor = pos;
            }
        } else {
            self.selection = None;
        }
        self.cursor = pos;
        self.clamp_cursor_col();
        self.cursor_changed = true;
        self.sync_legacy_cursor();
    }

    pub fn move_up(&mut self, extend: bool) {
        if self.cursor.line > 0 {
            let new = TextPosition::new(self.cursor.line - 1, self.cursor.col);
            self.move_cursor_to(new, extend);
        } else if !extend {
            self.selection = None;
        }
    }

    pub fn move_down(&mut self, extend: bool) {
        if self.cursor.line + 1 < self.rope.len_lines() {
            let new = TextPosition::new(self.cursor.line + 1, self.cursor.col);
            self.move_cursor_to(new, extend);
        } else if !extend {
            self.selection = None;
        }
    }

    pub fn move_left(&mut self, extend: bool) {
        // If selection and not extending, jump to start of selection
        if !extend {
            if let Some(ref sel) = self.selection.clone() {
                if !sel.is_empty() {
                    let (start, _) = sel.ordered();
                    self.cursor = start;
                    self.selection = None;
                    self.cursor_changed = true;
                    self.sync_legacy_cursor();
                    return;
                }
            }
        }
        let idx = self.cursor_char_idx();
        if idx > 0 {
            let new_idx = idx - 1;
            let new_pos = self.char_idx_to_pos(new_idx);
            self.move_cursor_to(new_pos, extend);
        } else if !extend {
            self.selection = None;
        }
    }

    pub fn move_right(&mut self, extend: bool) {
        // If selection and not extending, jump to end of selection
        if !extend {
            if let Some(ref sel) = self.selection.clone() {
                if !sel.is_empty() {
                    let (_, end) = sel.ordered();
                    self.cursor = end;
                    self.selection = None;
                    self.cursor_changed = true;
                    self.sync_legacy_cursor();
                    return;
                }
            }
        }
        let idx = self.cursor_char_idx();
        if idx < self.rope.len_chars() {
            let new_pos = self.char_idx_to_pos(idx + 1);
            self.move_cursor_to(new_pos, extend);
        }
    }

    pub fn move_home(&mut self, extend: bool) {
        // Smart home: go to first non-whitespace, then column 0
        let line = self.rope.line(self.cursor.line);
        let first_non_ws = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let target_col = if self.cursor.col != first_non_ws { first_non_ws } else { 0 };
        let new_pos = TextPosition::new(self.cursor.line, target_col);
        self.move_cursor_to(new_pos, extend);
    }

    pub fn move_end(&mut self, extend: bool) {
        if self.cursor.line < self.rope.len_lines() {
            let line = self.rope.line(self.cursor.line);
            let line_len = line.len_chars();
            let end_col = if line_len > 0 && line.char(line_len - 1) == '\n' {
                line_len - 1
            } else {
                line_len
            };
            let new_pos = TextPosition::new(self.cursor.line, end_col);
            self.move_cursor_to(new_pos, extend);
        }
    }

    /// Ctrl+D: select the word under cursor, or find next occurrence of current selection.
    pub fn select_next_occurrence(&mut self) {
        let text = self.rope.to_string();

        // Get current search term
        let search_term = if let Some(ref sel) = self.selection {
            if !sel.is_empty() {
                let (start, end) = sel.ordered();
                let start_char = self.rope.line_to_char(start.line) + start.col;
                let end_char = self.rope.line_to_char(end.line) + end.col;
                if start_char < end_char {
                    self.rope.slice(start_char..end_char).to_string()
                } else {
                    return;
                }
            } else {
                // No selection: select word under cursor
                self.select_word_at_cursor();
                return;
            }
        } else {
            // No selection at all: select word under cursor
            self.select_word_at_cursor();
            return;
        };

        // Find next occurrence after current cursor
        let cursor_char = self.rope.line_to_char(self.cursor.line) + self.cursor.col;
        let search_start = cursor_char;

        if let Some(pos) = text[search_start..].find(&search_term) {
            let abs_pos = search_start + pos;
            let anchor = self.char_idx_to_pos(abs_pos);
            let end_pos = self.char_idx_to_pos(abs_pos + search_term.len());
            self.selection = Some(Selection { anchor, cursor: end_pos });
            self.cursor = end_pos;
            self.cursor_changed = true;
            self.sync_legacy_cursor();
        } else if let Some(pos) = text.find(&search_term) {
            // Wrap around
            let anchor = self.char_idx_to_pos(pos);
            let end_pos = self.char_idx_to_pos(pos + search_term.len());
            self.selection = Some(Selection { anchor, cursor: end_pos });
            self.cursor = end_pos;
            self.cursor_changed = true;
            self.sync_legacy_cursor();
        }
    }

    fn select_word_at_cursor(&mut self) {
        let cursor_char = self.cursor_char_idx();
        let text = self.rope.to_string();
        let chars: Vec<char> = text.chars().collect();

        if cursor_char >= chars.len() { return; }

        // Find word boundaries
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut start = cursor_char;
        let mut end = cursor_char;

        while start > 0 && is_word(chars[start - 1]) { start -= 1; }
        while end < chars.len() && is_word(chars[end]) { end += 1; }

        if start < end {
            let anchor = self.char_idx_to_pos(start);
            let end_pos = self.char_idx_to_pos(end);
            self.selection = Some(Selection { anchor, cursor: end_pos });
            self.cursor = end_pos;
            self.cursor_changed = true;
            self.sync_legacy_cursor();
        }
    }

    pub fn select_all(&mut self) {
        let total_chars = self.rope.len_chars();
        let end_pos = self.char_idx_to_pos(total_chars);
        self.selection = Some(Selection {
            anchor: TextPosition::zero(),
            cursor: end_pos,
        });
        self.cursor = end_pos;
        self.cursor_changed = true;
        self.sync_legacy_cursor();
    }

    /// Recompute the bracket match for the current cursor position.
    pub fn update_bracket_match(&mut self) {
        // Skip expensive scan if cursor hasn't moved since last computation
        if self.bracket_match_cursor == Some(self.cursor) {
            return;
        }
        self.bracket_match_cursor = Some(self.cursor);
        self.bracket_match = find_bracket_match(&self.rope, self.cursor);
    }

    // ── Git Gutter ────────────────────────────────────────────────────────

    pub fn maybe_refresh_git_hunks(&mut self) {
        let needs = match self.last_git_check {
            None => true,
            Some(t) => t.elapsed().as_secs() >= 5,
        };
        if needs {
            self.refresh_git_hunks();
        }
    }

    pub fn refresh_git_hunks(&mut self) {
        self.last_git_check = Some(Instant::now());
        self.git_hunks.clear();

        let path = match &self.path {
            Some(p) => p.clone(),
            None => return,
        };

        let work_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let output = std::process::Command::new("git")
            .args(["diff", "--unified=0", "--", path.to_str().unwrap_or("")])
            .current_dir(work_dir)
            .output();

        let output = match output {
            Ok(o) => o,
            Err(_) => return,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_git_diff_hunks(&stdout, &mut self.git_hunks);

        // Also check against HEAD for new untracked content (git diff HEAD)
        if self.git_hunks.is_empty() {
            let output2 = std::process::Command::new("git")
                .args(["diff", "HEAD", "--unified=0", "--", path.to_str().unwrap_or("")])
                .current_dir(work_dir)
                .output();
            if let Ok(o) = output2 {
                let s = String::from_utf8_lossy(&o.stdout);
                parse_git_diff_hunks(&s, &mut self.git_hunks);
            }
        }
    }

    // ── Git Blame ──────────────────────────────────────────────────────────

    /// Load blame info for all lines via `git blame --porcelain`.
    /// Results are cached; subsequent calls are no-ops.
    pub fn load_blame(&mut self) {
        if self.blame_loaded { return; }
        self.blame_loaded = true;

        let path = match &self.path {
            Some(p) => p.clone(),
            None => return,
        };

        let work_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let output = std::process::Command::new("git")
            .args(["blame", "--porcelain", "--", path.to_str().unwrap_or("")])
            .current_dir(work_dir)
            .output();

        let output = match output {
            Ok(o) if o.status.success() => o,
            _ => return,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_blame_porcelain(&stdout, &mut self.blame_cache);
    }

    /// Returns a hover label for the given line (loads blame if needed).
    pub fn blame_for_line(&mut self, line_idx: usize) -> Option<String> {
        self.load_blame();
        self.blame_cache.get(&line_idx).map(|b| {
            format!("{} {} • {}", b.short_hash, b.author, b.date)
        })
    }

    // ── Multi-Cursor ──────────────────────────────────────────────────────

    /// Add an extra cursor at the given position (Alt+Click).
    pub fn add_cursor(&mut self, pos: TextPosition) {
        // Don't duplicate the primary cursor
        if pos == self.cursor { return; }
        // Remove if already exists (toggle off)
        if let Some(idx) = self.extra_cursors.iter().position(|&p| p == pos) {
            self.extra_cursors.remove(idx);
        } else {
            self.extra_cursors.push(pos);
        }
    }

    /// Remove all extra cursors.
    pub fn collapse_cursors(&mut self) {
        self.extra_cursors.clear();
    }

    pub fn has_multi_cursor(&self) -> bool {
        !self.extra_cursors.is_empty()
    }

    /// Insert a char at all cursor positions (primary + extra).
    /// Extra cursors are updated to stay correct after all insertions.
    pub fn insert_char_multi(&mut self, ch: char) {
        if self.extra_cursors.is_empty() {
            self.insert_char(ch);
            return;
        }

        self.delete_selection(); // Primary selection only — multi-cursor keeps no selection
        self.force_snapshot();

        // Collect all cursor char indices (primary + extra), sort descending
        let primary_idx = self.cursor_char_idx();
        let mut positions: Vec<(usize, bool)> = vec![(primary_idx, true)]; // (char_idx, is_primary)
        for &ec in &self.extra_cursors {
            positions.push((self.pos_to_char_idx(ec), false));
        }
        // Sort descending by char index
        positions.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        positions.dedup_by_key(|p| p.0);

        // Insert character at each position (descending, so earlier positions stay valid)
        for &(idx, _) in &positions {
            self.rope.insert_char(idx, ch);
        }
        self.modified = true;
        self.cursor_changed = true;
        self.highlight_cache.invalidate_from(self.cursor.line);

        // Update cursor positions: each cursor at original index X shifts by
        // count of insertion sites Y where Y <= X (including X itself)
        let orig_positions: Vec<usize> = positions.iter().map(|p| p.0).collect();

        let shift_for = |x: usize| -> usize {
            orig_positions.iter().filter(|&&y| y <= x).count()
        };

        // Update primary cursor
        let new_primary_idx = primary_idx + shift_for(primary_idx);
        if ch == '\n' {
            self.cursor = self.char_idx_to_pos(new_primary_idx);
        } else {
            self.cursor = self.char_idx_to_pos(new_primary_idx);
        }

        // Update extra cursors
        let new_extra: Vec<TextPosition> = self.extra_cursors.iter().map(|&ec| {
            let ec_idx = self.pos_to_char_idx(ec);
            let new_idx = ec_idx + shift_for(ec_idx);
            self.char_idx_to_pos(new_idx)
        }).collect();
        self.extra_cursors = new_extra;

        self.sync_legacy_cursor();
    }

    /// Delete backward at all cursor positions.
    pub fn delete_back_multi(&mut self) {
        if self.extra_cursors.is_empty() {
            self.delete_back();
            return;
        }
        self.force_snapshot();

        let primary_idx = self.cursor_char_idx();
        let mut indices: Vec<usize> = vec![primary_idx];
        for &ec in &self.extra_cursors {
            let idx = self.pos_to_char_idx(ec);
            if idx > 0 { indices.push(idx); }
        }
        // Sort descending
        indices.sort_unstable_by(|a, b| b.cmp(a));
        indices.dedup();

        // Remove char before each cursor (index-1) in descending order
        for &idx in &indices {
            if idx > 0 {
                self.rope.remove(idx - 1..idx);
            }
        }
        self.modified = true;
        self.cursor_changed = true;
        self.highlight_cache.invalidate_from(0);

        // Update positions: each cursor at original X shifts by
        // -count of deletions at positions Y where Y-1 < X (i.e., Y <= X)
        let shift_for = |x: usize| -> usize {
            indices.iter().filter(|&&y| y > 0 && y <= x).count()
        };

        if primary_idx > 0 {
            let new_primary_idx = primary_idx - shift_for(primary_idx);
            self.cursor = self.char_idx_to_pos(new_primary_idx);
        }

        let new_extra: Vec<TextPosition> = self.extra_cursors.iter().map(|&ec| {
            let ec_idx = self.pos_to_char_idx(ec);
            if ec_idx == 0 { return ec; }
            let new_idx = ec_idx.saturating_sub(shift_for(ec_idx));
            self.char_idx_to_pos(new_idx)
        }).collect();
        self.extra_cursors = new_extra;
        self.sync_legacy_cursor();
    }

    /// Auto-indent: when Enter is pressed, match indentation of previous line
    pub fn insert_newline_with_indent(&mut self) {
        let current_line = self.cursor.line;
        // Get the leading whitespace of the current line
        let indent: String = if current_line < self.rope.len_lines() {
            let line = self.rope.line(current_line);
            line.chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect()
        } else {
            String::new()
        };
        self.insert_char('\n');
        // Insert the indentation
        let dirty_line = self.cursor.line;
        let idx = self.cursor_char_idx();
        if !indent.is_empty() {
            self.rope.insert(idx, &indent);
            self.cursor.col += indent.len();
            self.modified = true;
            self.highlight_cache.invalidate_from(dirty_line);
        }
        self.sync_legacy_cursor();
    }

    pub fn undo(&mut self) {
        if self.edits_since_snapshot > 0 {
            self.force_snapshot();
        }
        if let Some(entry) = self.undo_stack.undo() {
            self.rope = entry.rope.clone();
            self.cursor = entry.cursor;
            self.selection = entry.selection.clone();
            self.modified = true;
            self.edits_since_snapshot = 0;
            self.highlight_cache.invalidate_from(0);
            self.cursor_changed = true;
            self.sync_legacy_cursor();
        }
    }

    pub fn redo(&mut self) {
        if let Some(entry) = self.undo_stack.redo() {
            self.rope = entry.rope.clone();
            self.cursor = entry.cursor;
            self.selection = entry.selection.clone();
            self.modified = true;
            self.edits_since_snapshot = 0;
            self.highlight_cache.invalidate_from(0);
            self.cursor_changed = true;
            self.sync_legacy_cursor();
        }
    }

    pub fn copy_to_clipboard(&self) {
        if let Some(text) = self.selected_text() {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set_text(text);
            }
        }
    }

    pub fn cut_to_clipboard(&mut self) {
        self.copy_to_clipboard();
        if self.delete_selection() {
            self.force_snapshot();
            self.sync_legacy_cursor();
        }
    }

    pub fn paste_from_clipboard(&mut self) {
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                self.insert_str(&text);
            }
        }
    }

    /// Get the line text without trailing newline
    pub fn line_text(&self, line_idx: usize) -> String {
        if line_idx >= self.rope.len_lines() {
            return String::new();
        }
        let mut s = self.rope.line(line_idx).to_string();
        // Strip trailing newline in-place (one allocation instead of two)
        if s.ends_with('\n') {
            s.pop();
        }
        s
    }
}

// ── Find/Replace State ───────────────────────────────────────────────────

pub struct FindReplaceState {
    pub visible: bool,
    pub query: String,
    pub replacement: String,
    pub matches: Vec<(usize, usize)>,
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
                start += pos + query.len().max(1);
                if start >= search_str.len() { break; }
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
        if self.matches.is_empty() || self.query.is_empty() { return; }
        let (line_idx, col) = self.matches[self.current_match];
        let line_start = tab.rope.line_to_char(line_idx);
        let start_char = line_start + col;
        let end_char = start_char + self.query.len();
        tab.force_snapshot();
        tab.rope.remove(start_char..end_char);
        tab.rope.insert(start_char, &self.replacement);
        tab.modified = true;
        tab.highlight_cache.invalidate_from(line_idx);
        self.search(&tab.rope);
    }

    pub fn replace_all(&mut self, tab: &mut EditorTab) {
        if self.matches.is_empty() || self.query.is_empty() { return; }
        tab.force_snapshot();
        let matches: Vec<_> = self.matches.iter().rev().cloned().collect();
        let first_line = matches.last().map(|(l, _)| *l).unwrap_or(0);
        for (line_idx, col) in matches {
            let line_start = tab.rope.line_to_char(line_idx);
            let start_char = line_start + col;
            let end_char = start_char + self.query.len();
            tab.rope.remove(start_char..end_char);
            tab.rope.insert(start_char, &self.replacement);
        }
        tab.modified = true;
        tab.highlight_cache.invalidate_from(first_line);
        self.search(&tab.rope);
    }
}

// ── Async File Load ───────────────────────────────────────────────────────

/// Result of a background file read, delivered via channel to the UI thread.
struct FileLoadResult {
    path: PathBuf,
    content: Result<String, String>,
    /// If set, scroll to this line after loading.
    jump_to_line: Option<usize>,
}

// ── Editor Panel ─────────────────────────────────────────────────────────

pub struct EditorPanel {
    pub tabs: Vec<EditorTab>,
    pub active_tab: usize,
    pub show_line_numbers: bool,
    pub show_minimap: bool,
    pub font_size: f32,
    pub find_replace: FindReplaceState,
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    /// Tree-sitter highlight engine (None if initialization failed).
    ts_engine: Option<TsHighlightEngine>,
    pub has_focus: bool,
    /// Cached char width for click-to-cursor (updated each frame)
    char_width: f32,
    /// Channel for receiving async file loads from background threads.
    file_load_tx: mpsc::SyncSender<FileLoadResult>,
    file_load_rx: mpsc::Receiver<FileLoadResult>,
    /// Tracks paths currently being loaded to prevent duplicate loads.
    loading_paths: HashSet<PathBuf>,
    /// Index of the tab currently being dragged for reordering.
    dragging_tab: Option<usize>,
    /// Index of the hover-target during a tab drag.
    drag_target: Option<usize>,
}

impl EditorPanel {
    pub fn new(font_size: f32) -> Self {
        let (file_load_tx, file_load_rx) = mpsc::sync_channel(64);
        Self {
            tabs: vec![EditorTab::new_untitled()],
            active_tab: 0,
            show_line_numbers: true,
            font_size,
            show_minimap: true,
            find_replace: FindReplaceState::new(),
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            ts_engine: TsHighlightEngine::new(),
            has_focus: false,
            char_width: font_size * 0.6,
            file_load_tx,
            file_load_rx,
            loading_paths: HashSet::new(),
            dragging_tab: None,
            drag_target: None,
        }
    }

    /// Drain the async file load channel and insert any loaded tabs.
    /// Call this once per frame from `app.rs`. Returns paths of newly opened tabs
    /// (so `app.rs` can notify LSP, watchers, etc.).
    pub fn drain_file_loads(&mut self) -> Vec<PathBuf> {
        let mut opened = Vec::new();
        while let Ok(result) = self.file_load_rx.try_recv() {
            self.loading_paths.remove(&result.path);
            match result.content {
                Err(e) => tracing::error!("Failed to load {:?}: {e}", result.path),
                Ok(content) => {
                    // Check if a tab for this path was already added (e.g. by a
                    // racing sync open_file call) while the thread was running.
                    let already_open = self.tabs.iter().position(|t| t.path.as_ref() == Some(&result.path));
                    let tab_idx = if let Some(idx) = already_open {
                        idx
                    } else {
                        let title = result.path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "Untitled".to_string());
                        let language = detect_language(&result.path);
                        let rope = Rope::from_str(&content);
                        let tab = EditorTab {
                            undo_stack: UndoStack::new(&rope),
                            path: Some(result.path.clone()),
                            title,
                            rope,
                            modified: false,
                            cursor: TextPosition::zero(),
                            selection: None,
                            language,
                            edits_since_snapshot: 0,
                            highlight_cache: HighlightCache::new(),
                            cursor_changed: false,
                            cursor_line: 0,
                            cursor_col: 0,
                            git_hunks: HashMap::new(),
                            last_git_check: None,
                            blame_cache: HashMap::new(),
                            blame_loaded: false,
                            diagnostics: HashMap::new(),
                            lsp_version: 0,
                            extra_cursors: Vec::new(),
                            block_selection: None,
                            hover_pos: None,
                            hover_start: None,
                            hover_pending: false,
                            hover_text: None,
                            completions: Vec::new(),
                            completion_visible: false,
                            completion_selected: 0,
                            bracket_match: None,
                            bracket_match_cursor: None,
                            folded_lines: HashMap::new(),
                            fold_hidden_cache: HashSet::new(),
                            fold_generation: 0,
                            fold_cache_generation: u64::MAX,
                            ts_spans: Vec::new(),
                            ts_computed_len: usize::MAX,
                        };
                        self.tabs.push(tab);
                        self.tabs.len() - 1
                    };
                    self.active_tab = tab_idx;
                    opened.push(result.path.clone());

                    // Scroll to requested line if any
                    if let Some(line) = result.jump_to_line {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            let clamped = line.min(tab.rope.len_lines().saturating_sub(1));
                            tab.cursor = TextPosition::new(clamped, 0);
                            tab.selection = None;
                            tab.cursor_changed = true;
                        }
                    }
                }
            }
        }
        opened
    }

    /// Open a file asynchronously (non-blocking). If the file is already open,
    /// switches to its tab immediately. Otherwise, spawns a background thread
    /// to read it and calls `egui::Context::request_repaint()` when done.
    pub fn open_file(&mut self, path: PathBuf) {
        // Already open → switch tab immediately
        for (i, tab) in self.tabs.iter().enumerate() {
            if tab.path.as_ref() == Some(&path) {
                self.active_tab = i;
                return;
            }
        }
        // Already being loaded → ignore duplicate request
        if self.loading_paths.contains(&path) {
            return;
        }
        self.loading_paths.insert(path.clone());
        let tx = self.file_load_tx.clone();
        std::thread::spawn(move || {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string());
            let _ = tx.send(FileLoadResult { path, content, jump_to_line: None });
        });
    }

    /// Open a file asynchronously and jump to a specific line after loading.
    pub fn open_file_at_line(&mut self, path: PathBuf, line: usize) {
        // Already open → switch and jump immediately
        for (i, tab) in self.tabs.iter().enumerate() {
            if tab.path.as_ref() == Some(&path) {
                self.active_tab = i;
                if let Some(tab) = self.tabs.get_mut(i) {
                    let clamped = line.min(tab.rope.len_lines().saturating_sub(1));
                    tab.cursor = TextPosition::new(clamped, 0);
                    tab.selection = None;
                    tab.cursor_changed = true;
                }
                return;
            }
        }
        if self.loading_paths.contains(&path) {
            return;
        }
        self.loading_paths.insert(path.clone());
        let tx = self.file_load_tx.clone();
        std::thread::spawn(move || {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string());
            let _ = tx.send(FileLoadResult { path, content, jump_to_line: Some(line) });
        });
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

    // ── LSP helpers ───────────────────────────────────────────────────────

    /// Returns (path, line, character) if the active tab has a hover position
    /// that has been stable for > 500ms and no request is already in-flight.
    pub fn lsp_hover_request(&mut self) -> Option<(std::path::PathBuf, u32, u32)> {
        let tab = self.tabs.get_mut(self.active_tab)?;
        if tab.hover_pending { return None; }
        let (line, col) = tab.hover_pos?;
        let elapsed = tab.hover_start?.elapsed();
        if elapsed.as_millis() < 500 { return None; }
        let path = tab.path.clone()?;
        tab.hover_pending = true;
        Some((path, line, col))
    }

    /// Called by app.rs when an LSP hover result arrives.
    pub fn set_hover_result(&mut self, text: Option<String>) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.hover_pending = false;
            tab.hover_text = text;
        }
    }

    /// Returns (path, line, character) for a go-to-definition request at
    /// the current cursor position.
    pub fn lsp_goto_def_request(&self) -> Option<(std::path::PathBuf, u32, u32)> {
        let tab = self.tabs.get(self.active_tab)?;
        let path = tab.path.clone()?;
        Some((path, tab.cursor.line as u32, tab.cursor.col as u32))
    }

    /// Trigger a completion request at the cursor position.
    pub fn lsp_completion_request(&self) -> Option<(std::path::PathBuf, u32, u32)> {
        let tab = self.tabs.get(self.active_tab)?;
        let path = tab.path.clone()?;
        Some((path, tab.cursor.line as u32, tab.cursor.col as u32))
    }

    /// Called when completion results arrive.
    pub fn set_completion_result(&mut self, items: Vec<lsp_types::CompletionItem>) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.completions = items;
            tab.completion_visible = !tab.completions.is_empty();
            tab.completion_selected = 0;
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        // Update char_width based on current font size
        self.char_width = self.font_size * 0.601; // slightly above 0.6 for monospace

        // Tab bar (with drag-to-reorder)
        ui.horizontal(|ui| {
            let mut close_idx = None;
            let mut swap: Option<(usize, usize)> = None;

            for (i, tab) in self.tabs.iter().enumerate() {
                let label = if tab.modified {
                    format!("{} ●", tab.title)
                } else {
                    tab.title.clone()
                };
                let is_active = i == self.active_tab;
                let is_drop_target = self.drag_target == Some(i)
                    && self.dragging_tab.map_or(false, |d| d != i);

                let bg = if is_drop_target {
                    theme.accent.gamma_multiply(0.4)
                } else if is_active {
                    theme.background
                } else {
                    theme.background_secondary
                };
                let fg = if is_active { theme.text } else { theme.text_muted };

                let tab_response = ui.add(
                    egui::Button::new(egui::RichText::new(&label).color(fg))
                        .fill(bg)
                        .frame(true)
                        .min_size(Vec2::new(0.0, 28.0))
                        .sense(egui::Sense::click_and_drag())
                );

                if tab_response.clicked() {
                    self.active_tab = i;
                }
                if tab_response.middle_clicked() {
                    close_idx = Some(i);
                }
                if tab_response.drag_started() {
                    self.dragging_tab = Some(i);
                    self.drag_target = Some(i);
                }
                if tab_response.hovered() && self.dragging_tab.is_some() {
                    self.drag_target = Some(i);
                }
                if tab_response.drag_stopped() {
                    if let (Some(from), Some(to)) = (self.dragging_tab, self.drag_target) {
                        if from != to {
                            swap = Some((from, to));
                        }
                    }
                    self.dragging_tab = None;
                    self.drag_target = None;
                }

                if self.tabs.len() > 1 {
                    if ui.small_button("×").clicked() {
                        close_idx = Some(i);
                    }
                }
                ui.add_space(4.0);
            }

            if ui.small_button("+").clicked() {
                self.new_tab();
            }

            if let Some(idx) = close_idx {
                self.close_tab(idx);
            }
            if let Some((from, to)) = swap {
                self.tabs.swap(from, to);
                if self.active_tab == from {
                    self.active_tab = to;
                } else if self.active_tab == to {
                    self.active_tab = from;
                }
            }
        });

        // Breadcrumb bar (file > scope > function)
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let crumbs = build_breadcrumbs(tab);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                for (i, crumb) in crumbs.iter().enumerate() {
                    if i > 0 {
                        ui.colored_label(theme.text_muted, "›");
                    }
                    ui.colored_label(theme.text_secondary, egui::RichText::new(crumb).size(11.5));
                }
            });
        }

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
                if find_resp.changed() { do_search = true; }
                if find_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    do_next = true;
                }
                if ui.small_button("▲").clicked() { do_prev = true; }
                if ui.small_button("▼").clicked() { do_next = true; }

                let match_count = self.find_replace.matches.len();
                if match_count > 0 {
                    ui.label(format!("{}/{}", self.find_replace.current_match + 1, match_count));
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
                if ui.small_button("Replace").clicked() { do_replace = true; }
                if ui.small_button("All").clicked() { do_replace_all = true; }
                if ui.small_button("✕").clicked() {
                    self.find_replace.visible = false;
                    self.find_replace.matches.clear();
                }
            });

            if do_search {
                if let Some(tab) = self.tabs.get(self.active_tab) {
                    let rope = tab.rope.clone();
                    self.find_replace.search(&rope);
                }
            }
            if do_next {
                self.find_replace.next_match();
                if let Some(&(line, col)) = self.find_replace.matches.get(self.find_replace.current_match) {
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        tab.cursor = TextPosition::new(line, col);
                        tab.cursor_changed = true;
                        tab.sync_legacy_cursor();
                    }
                }
            }
            if do_prev {
                self.find_replace.prev_match();
                if let Some(&(line, col)) = self.find_replace.matches.get(self.find_replace.current_match) {
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        tab.cursor = TextPosition::new(line, col);
                        tab.cursor_changed = true;
                        tab.sync_legacy_cursor();
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
        let char_width = self.char_width;

        if active_tab < self.tabs.len() {
            let syntax_set = &self.syntax_set;
            let theme_set = &self.theme_set;
            let find_matches = if self.find_replace.visible {
                Some((&self.find_replace.matches, self.find_replace.current_match, self.find_replace.query.len()))
            } else {
                None
            };
            let tab = &mut self.tabs[active_tab];
            render_editor_content(ui, tab, theme, font_size, show_line_numbers, char_width, syntax_set, theme_set, &mut self.ts_engine, find_matches, self.has_focus);

            // ── LSP hover popup ──────────────────────────────────────────────
            if let Some(ref hover_text) = tab.hover_text.clone() {
                if let Some(ptr_pos) = ui.input(|i| i.pointer.hover_pos()) {
                    let popup_id = ui.id().with("lsp_hover_popup");
                    egui::Area::new(popup_id)
                        .fixed_pos(ptr_pos + egui::Vec2::new(16.0, 16.0))
                        .order(egui::Order::Tooltip)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.set_max_width(500.0);
                                ui.label(egui::RichText::new(hover_text).monospace().size(12.0));
                            });
                        });
                }
            }

            // ── Completion popup ─────────────────────────────────────────────
            if tab.completion_visible && !tab.completions.is_empty() {
                let completions_clone: Vec<_> = tab.completions.iter()
                    .map(|c| c.label.clone())
                    .collect();
                let selected = tab.completion_selected;
                let popup_id = ui.id().with("lsp_completion_popup");
                let mut chosen: Option<usize> = None;
                let cursor = tab.cursor;
                egui::Area::new(popup_id)
                    .fixed_pos({
                        // Position below cursor line roughly
                        let row = cursor.line as f32 * (font_size + 4.0);
                        let col = cursor.col as f32 * char_width;
                        ui.min_rect().min + egui::Vec2::new(col + 48.0, row + font_size + 4.0 + 6.0)
                    })
                    .order(egui::Order::Tooltip)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_max_width(350.0);
                            ui.set_max_height(200.0);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for (i, label) in completions_clone.iter().enumerate() {
                                    let is_sel = i == selected;
                                    let bg = if is_sel { theme.accent } else { theme.background_secondary };
                                    let rt = egui::RichText::new(label).monospace().size(12.0);
                                    if ui.add(egui::Button::new(rt).fill(bg).frame(true)).clicked() {
                                        chosen = Some(i);
                                    }
                                }
                            });
                        });
                    });
                if let Some(idx) = chosen {
                    let label = tab.completions[idx].label.clone();
                    // Replace the current word with the chosen completion
                    let col = tab.cursor.col;
                    let line_idx = tab.cursor.line;
                    let line_start = tab.rope.line_to_char(line_idx);
                    // Find start of current word
                    let line_text = tab.line_text(line_idx);
                    let word_start = line_text[..col]
                        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let abs_word_start = line_start + word_start;
                    let abs_cursor = line_start + col;
                    tab.force_snapshot();
                    tab.rope.remove(abs_word_start..abs_cursor);
                    tab.rope.insert(abs_word_start, &label);
                    tab.cursor = TextPosition::new(line_idx, word_start + label.len());
                    tab.sync_legacy_cursor();
                    tab.completion_visible = false;
                    tab.completions.clear();
                    tab.highlight_cache.invalidate_from(line_idx);
                }
                // Keyboard navigation for completion popup
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    tab.completion_visible = false;
                    tab.completions.clear();
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    let len = tab.completions.len();
                    tab.completion_selected = (selected + 1).min(len.saturating_sub(1));
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    tab.completion_selected = selected.saturating_sub(1);
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Tab)) {
                    let idx = tab.completion_selected;
                    if idx < tab.completions.len() {
                        let label = tab.completions[idx].label.clone();
                        let col = tab.cursor.col;
                        let line_idx = tab.cursor.line;
                        let line_start = tab.rope.line_to_char(line_idx);
                        let line_text = tab.line_text(line_idx);
                        let word_start = line_text[..col]
                            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                            .map(|i| i + 1)
                            .unwrap_or(0);
                        let abs_word_start = line_start + word_start;
                        let abs_cursor = line_start + col;
                        tab.force_snapshot();
                        tab.rope.remove(abs_word_start..abs_cursor);
                        tab.rope.insert(abs_word_start, &label);
                        tab.cursor = TextPosition::new(line_idx, word_start + label.len());
                        tab.sync_legacy_cursor();
                        tab.highlight_cache.invalidate_from(line_idx);
                        tab.completion_visible = false;
                        tab.completions.clear();
                    }
                }
            }

            // ── Minimap ──────────────────────────────────────────────────────
            if self.show_minimap {
                let minimap_width = 80.0_f32;
                let clip = ui.clip_rect();
                let minimap_rect = egui::Rect::from_min_size(
                    egui::pos2(clip.right() - minimap_width, clip.top()),
                    egui::Vec2::new(minimap_width, clip.height()),
                );
                let painter = ui.painter_at(minimap_rect);
                painter.rect_filled(minimap_rect, 0.0, Color32::from_rgba_premultiplied(15, 15, 20, 220));

                let total_lines = tab.rope.len_lines().max(1);
                let line_h = (minimap_rect.height() / total_lines as f32).max(0.5_f32).min(3.0_f32);

                // Current viewport: estimate from cursor position
                let scroll_frac = tab.cursor.line as f32 / total_lines as f32;
                let viewport_h = (minimap_rect.height() * 0.25).max(8.0);
                let vp_y = (minimap_rect.top() + scroll_frac * minimap_rect.height() - viewport_h * 0.5)
                    .max(minimap_rect.top())
                    .min(minimap_rect.bottom() - viewport_h);
                painter.rect_filled(
                    egui::Rect::from_min_size(egui::pos2(minimap_rect.left(), vp_y), egui::Vec2::new(minimap_width, viewport_h)),
                    0.0, Color32::from_rgba_premultiplied(80, 80, 120, 60),
                );

                for line_idx in 0..total_lines {
                    let y = minimap_rect.top() + line_idx as f32 * line_h;
                    let line_text = tab.line_text(line_idx);
                    let trimmed = line_text.trim_end_matches('\n').trim_end_matches('\r');
                    let indent = trimmed.len() - trimmed.trim_start().len();
                    let content_len = trimmed.trim().len();
                    if content_len > 0 {
                        let x_start = minimap_rect.left() + (indent as f32 * 0.4).min(16.0);
                        let x_end = (x_start + content_len as f32 * 0.45).min(minimap_rect.right() - 2.0);
                        if x_end > x_start {
                            let color = if tab.diagnostics.contains_key(&line_idx) {
                                Color32::from_rgb(200, 80, 80)
                            } else {
                                Color32::from_rgba_premultiplied(140, 140, 160, 200)
                            };
                            painter.line_segment(
                                [egui::pos2(x_start, y), egui::pos2(x_end, y)],
                                egui::Stroke::new(line_h.max(1.0), color),
                            );
                        }
                    }
                }

                // Click minimap to scroll to line
                let mm_resp = ui.interact(minimap_rect, ui.id().with("minimap_interact"), Sense::click());
                if mm_resp.clicked() {
                    if let Some(pos) = mm_resp.interact_pointer_pos() {
                        let rel = ((pos.y - minimap_rect.top()) / minimap_rect.height()).clamp(0.0, 1.0);
                        let target = (rel * total_lines as f32) as usize;
                        tab.cursor = TextPosition::new(target.min(total_lines.saturating_sub(1)), 0);
                        tab.cursor_changed = true;
                        tab.sync_legacy_cursor();
                    }
                }
            }
        }
    }
}

// ── Editor Rendering ──────────────────────────────────────────────────────

fn render_editor_content(
    ui: &mut egui::Ui,
    tab: &mut EditorTab,
    theme: &ThemeColors,
    font_size: f32,
    show_line_numbers: bool,
    char_width: f32,
    syntax_set: &SyntaxSet,
    theme_set: &ThemeSet,
    ts_engine: &mut Option<TsHighlightEngine>,
    find_matches: Option<(&Vec<(usize, usize)>, usize, usize)>,
    has_focus: bool,
) {
    // ── Tree-sitter: recompute spans when file changes ─────────────────────
    let use_ts_highlight = tab.language == "rs";
    if use_ts_highlight {
        let current_len = tab.rope.len_bytes();
        if tab.ts_computed_len != current_len {
            if let Some(ref mut engine) = ts_engine {
                let source: Vec<u8> = tab.rope.bytes().collect();
                tab.ts_spans = engine.compute_rust(&source);
                tab.ts_computed_len = current_len;
                // Invalidate all LayoutJob caches since highlight spans changed
                tab.highlight_cache.invalidate_from(0);
            }
        }
    }
    let line_height = font_size + 4.0;
    let total_lines = tab.rope.len_lines().max(1);
    let gutter_width = if show_line_numbers {
        let digits = format!("{}", total_lines).len();
        (digits as f32) * char_width + 24.0  // extra 4px for git bar
    } else {
        0.0
    };

    // Refresh git hunks periodically (throttled to every 5s)
    tab.maybe_refresh_git_hunks();

    // Update bracket matching each frame (fast scan near cursor)
    tab.update_bracket_match();

    // Compute which lines are hidden inside collapsed fold regions.
    // Cache the set and only rebuild when folded_lines changes (tracked via fold_generation).
    if tab.fold_generation != tab.fold_cache_generation {
        tab.fold_hidden_cache.clear();
        for (&fstart, &fend) in &tab.folded_lines {
            for l in (fstart + 1)..=fend {
                tab.fold_hidden_cache.insert(l);
            }
        }
        tab.fold_cache_generation = tab.fold_generation;
    }
    let hidden_count = tab.fold_hidden_cache.len();

    // ── Keyboard Input ──
    if has_focus {
        let mut needs_search_update = false;
        ui.input(|i| {
            let _shift = i.modifiers.shift;
            let ctrl = i.modifiers.command || i.modifiers.ctrl;

            for event in &i.events {
                match event {
                    // Character input
                    egui::Event::Text(text) if !ctrl => {
                        for ch in text.chars() {
                            tab.insert_char_multi(ch);
                        }
                        needs_search_update = true;
                    }

                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        let shift = modifiers.shift;
                        let ctrl = modifiers.command || modifiers.ctrl;
                        let _alt = modifiers.alt;

                        match key {
                            egui::Key::Escape => {
                                // Collapse extra cursors and block selection on Escape
                                if tab.has_multi_cursor() {
                                    tab.collapse_cursors();
                                }
                                tab.block_selection = None;
                            }
                            egui::Key::Enter => {
                                if !ctrl {
                                    tab.insert_newline_with_indent();
                                    needs_search_update = true;
                                }
                            }
                            egui::Key::Backspace => {
                                if ctrl {
                                    tab.delete_word_back();
                                } else {
                                    tab.delete_back_multi();
                                }
                                needs_search_update = true;
                            }
                            egui::Key::Delete => {
                                if ctrl {
                                    tab.delete_word_forward();
                                } else {
                                    tab.delete_forward();
                                }
                                needs_search_update = true;
                            }
                            egui::Key::Tab => {
                                if !shift {
                                    // 4 spaces for tab (or smart tab for indent)
                                    tab.insert_str("    ");
                                }
                            }
                            egui::Key::ArrowUp => tab.move_up(shift),
                            egui::Key::ArrowDown => tab.move_down(shift),
                            egui::Key::ArrowLeft => {
                                if ctrl { tab.move_word_left(shift); }
                                else { tab.move_left(shift); }
                            }
                            egui::Key::ArrowRight => {
                                if ctrl { tab.move_word_right(shift); }
                                else { tab.move_right(shift); }
                            }
                            egui::Key::Home => tab.move_home(shift),
                            egui::Key::End => tab.move_end(shift),
                            egui::Key::PageUp => {
                                let new_line = tab.cursor.line.saturating_sub(25);
                                let new = TextPosition::new(new_line, tab.cursor.col);
                                tab.move_cursor_to(new, shift);
                            }
                            egui::Key::PageDown => {
                                let new_line = (tab.cursor.line + 25).min(tab.rope.len_lines().saturating_sub(1));
                                let new = TextPosition::new(new_line, tab.cursor.col);
                                tab.move_cursor_to(new, shift);
                            }
                            egui::Key::A if ctrl => {
                                tab.select_all();
                            }
                            egui::Key::C if ctrl => {
                                tab.copy_to_clipboard();
                            }
                            egui::Key::X if ctrl => {
                                tab.cut_to_clipboard();
                                needs_search_update = true;
                            }
                            egui::Key::V if ctrl => {
                                tab.paste_from_clipboard();
                                needs_search_update = true;
                            }
                            _ => {}
                        }
                    }

                    egui::Event::Paste(text) => {
                        tab.insert_str(text);
                        needs_search_update = true;
                    }

                    egui::Event::Copy => {
                        tab.copy_to_clipboard();
                    }
                    egui::Event::Cut => {
                        tab.cut_to_clipboard();
                        needs_search_update = true;
                    }

                    _ => {}
                }
            }
        });

        if needs_search_update {
            // Search update handled by caller if find is open
        }
    }

    // Helper: check if a line has any search matches (avoids allocating a HashSet)
    let line_has_match = |line: usize| -> bool {
        find_matches
            .map(|(matches, _, _)| matches.iter().any(|(l, _)| *l == line))
            .unwrap_or(false)
    };

    // Prepare syntax highlighting — need the syntax reference
    let syntax_ref = if !tab.language.is_empty() {
        syntax_set.find_syntax_by_extension(&tab.language)
    } else {
        None
    };

    let highlight_theme = &theme_set.themes["base16-ocean.dark"];
    let highlighter = Highlighter::new(highlight_theme);

    // Ensure cache is sized
    tab.highlight_cache.ensure_size(total_lines + 1);

    let scroll_output = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available_width = ui.available_width();
            let mut cursor_rect: Option<egui::Rect> = None;

            // Virtualize: only render visible lines
            // Account for lines hidden by code folds in the height calculation
            let visible_total = total_lines.saturating_sub(hidden_count);
            let viewport = ui.clip_rect();
            let origin_y = ui.min_rect().min.y;
            let min_visible = ((viewport.min.y - origin_y) / line_height).floor().max(0.0) as usize;
            let max_visible = (((viewport.max.y - origin_y) / line_height).ceil() as usize + 1).min(total_lines);

            // Space before visible region
            if min_visible > 0 {
                ui.add_space(min_visible as f32 * line_height);
            }

            // Build highlight state up to min_visible (walk from last valid cached state)
            // This ensures multi-line state is correct even for scrolled-past lines
            if let Some(syn) = syntax_ref {
                ensure_highlight_state_up_to(
                    &mut tab.highlight_cache,
                    &tab.rope,
                    syn,
                    syntax_set,
                    &highlighter,
                    min_visible,
                );
            }

            for line_idx in min_visible..max_visible {
                // Skip lines hidden inside a fold region
                if tab.fold_hidden_cache.contains(&line_idx) { continue; }

                // Expand tabs to 4 spaces for display
                // Fetch raw line text once; derive display text from it (avoids two rope scans)
                let raw_line_text = tab.line_text(line_idx);
                let line_text = raw_line_text.replace('\t', "    ");

                ui.horizontal(|ui| {
                    // Line number gutter
                    if show_line_numbers {
                        let (gutter_id, gutter_rect) = ui.allocate_space(Vec2::new(gutter_width, line_height));
                        let gutter_response = ui.interact(gutter_rect, gutter_id, Sense::hover());
                        let gutter_color = if line_idx == tab.cursor.line {
                            theme.text_secondary
                        } else {
                            theme.text_muted
                        };
                        ui.painter().text(
                            gutter_rect.right_center() - Vec2::new(8.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            format!("{}", line_idx + 1),
                            FontId::monospace(font_size - 2.0),
                            gutter_color,
                        );

                        // Git gutter bar (3px on left edge of gutter)
                        if let Some(status) = tab.git_hunks.get(&line_idx) {
                            let bar_color = match status {
                                GitLineStatus::Added    => Color32::from_rgb(80, 200, 80),
                                GitLineStatus::Modified => Color32::from_rgb(220, 160, 0),
                                GitLineStatus::Deleted  => Color32::from_rgb(210, 60, 60),
                            };
                            ui.painter().rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(gutter_rect.left(), gutter_rect.top()),
                                    egui::pos2(gutter_rect.left() + 3.0, gutter_rect.bottom()),
                                ),
                                0.0,
                                bar_color,
                            );
                        }

                        // Git blame tooltip on gutter hover
                        if gutter_response.hovered() && tab.path.is_some() {
                            if let Some(blame_text) = tab.blame_for_line(line_idx) {
                                gutter_response.on_hover_text_at_pointer(blame_text);
                            }
                        }

                        // Code fold triangle indicator (reuse raw_line_text from above)
                        let trimmed_right = raw_line_text.trim_end();
                        let is_foldable = trimmed_right.ends_with('{')
                            || trimmed_right.ends_with('(')
                            || trimmed_right.ends_with('[');
                        let is_folded = tab.folded_lines.contains_key(&line_idx);
                        if is_foldable || is_folded {
                            let triangle = if is_folded { "▸" } else { "▾" };
                            let tri_rect = egui::Rect::from_min_size(
                                egui::pos2(gutter_rect.left() + 4.0, gutter_rect.top()),
                                egui::vec2(10.0, line_height),
                            );
                            let tri_id = ui.id().with(("fold_tri", line_idx));
                            let tri_resp = ui.interact(tri_rect, tri_id, Sense::click());
                            let tri_color = if tri_resp.hovered() { theme.accent } else { theme.text_muted };
                            ui.painter().text(
                                tri_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                triangle,
                                FontId::monospace(font_size - 4.0),
                                tri_color,
                            );
                            if tri_resp.clicked() {
                                if is_folded {
                                    tab.folded_lines.remove(&line_idx);
                                    tab.fold_generation += 1;
                                } else if let Some(fold_end) = find_fold_end(&tab.rope, line_idx) {
                                    tab.folded_lines.insert(line_idx, fold_end);
                                    tab.fold_generation += 1;
                                }
                            }
                        }
                    }

                    // Build layout job — prefer tree-sitter for supported languages
                    let layout_job = if use_ts_highlight && !tab.ts_spans.is_empty() {
                        // Compute byte offset of this line within the document
                        let line_byte_start = tab.rope.line_to_byte(line_idx);
                        highlight_line_ts(
                            &line_text,
                            line_byte_start,
                            &tab.ts_spans,
                            theme,
                            font_size,
                        )
                    } else if let Some(syn) = syntax_ref {
                        highlight_line_stateful(
                            &line_text,
                            &mut tab.highlight_cache,
                            line_idx,
                            syn,
                            syntax_set,
                            &highlighter,
                            font_size,
                            theme,
                        )
                    } else {
                        plain_layout(&line_text, font_size, theme)
                    };

                    let galley = ui.fonts(|f| f.layout_job(layout_job));

                    let content_width = (available_width - gutter_width).max(galley.size().x + 40.0);
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(content_width, line_height),
                        Sense::click_and_drag(),
                    );

                    // Current line highlight
                    if line_idx == tab.cursor.line {
                        ui.painter().rect_filled(
                            rect,
                            0.0,
                            Color32::from_rgba_premultiplied(255, 255, 255, 10),
                        );
                        cursor_rect = Some(rect);
                    }

                    // Selection highlight
                    if let Some(ref sel) = tab.selection {
                        if !sel.is_empty() && sel.contains_line(line_idx) {
                            let (start, end) = sel.ordered();
                            let sel_start_col = if line_idx > start.line { 0 } else { start.col };
                            let line_len = raw_line_text.len(); // reuse already-fetched text
                            let sel_end_col = if line_idx < end.line { line_len + 1 } else { end.col };

                            let x_start = rect.left() + sel_start_col as f32 * char_width;
                            let x_end = rect.left() + sel_end_col as f32 * char_width;
                            ui.painter().rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(x_start.max(rect.left()), rect.top()),
                                    egui::pos2(x_end.min(rect.right()), rect.bottom()),
                                ),
                                0.0,
                                Color32::from_rgba_premultiplied(100, 150, 255, 80),
                            );
                        }
                    }

                    // Search match highlights
                    if line_has_match(line_idx) {
                        if let Some((matches, current, query_len)) = &find_matches {
                            for (i, (ml, mc)) in matches.iter().enumerate() {
                                if *ml == line_idx {
                                    let x_start = rect.left() + (*mc as f32) * char_width;
                                    let x_end = x_start + (*query_len as f32) * char_width;
                                    let highlight_color = if i == *current {
                                        Color32::from_rgba_premultiplied(255, 180, 0, 120)
                                    } else {
                                        Color32::from_rgba_premultiplied(255, 255, 0, 60)
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

                    // Block/column selection highlight
                    if let Some((anchor, cursor)) = tab.block_selection {
                        let min_line = anchor.line.min(cursor.line);
                        let max_line = anchor.line.max(cursor.line);
                        let min_col = anchor.col.min(cursor.col);
                        let max_col = anchor.col.max(cursor.col);
                        if line_idx >= min_line && line_idx <= max_line {
                            let x_start = rect.left() + min_col as f32 * char_width;
                            let x_end = rect.left() + (max_col as f32 * char_width).max(x_start + 2.0);
                            ui.painter().rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(x_start, rect.top()),
                                    egui::pos2(x_end, rect.bottom()),
                                ),
                                0.0,
                                Color32::from_rgba_premultiplied(80, 160, 255, 70),
                            );
                        }
                    }

                    // Bracket match highlight (highlight the pair character positions)
                    if let Some((bp1, bp2)) = tab.bracket_match {
                        for bp in [bp1, bp2] {
                            if bp.line == line_idx {
                                let x = rect.left() + bp.col as f32 * char_width;
                                let brace_rect = egui::Rect::from_min_max(
                                    egui::pos2(x, rect.top() + 1.0),
                                    egui::pos2(x + char_width, rect.bottom() - 1.0),
                                );
                                ui.painter().rect(
                                    brace_rect,
                                    2.0,
                                    Color32::from_rgba_premultiplied(255, 210, 80, 30),
                                    egui::Stroke::new(1.0, Color32::from_rgb(255, 200, 60)),
                                );
                            }
                        }
                    }

                    // Draw text
                    ui.painter().galley(
                        egui::pos2(rect.left(), rect.top() + (line_height - galley.size().y) / 2.0),
                        galley,
                        theme.text,
                    );

                    // LSP diagnostic underlines (squiggles at bottom of line)
                    if let Some(diags) = tab.diagnostics.get(&line_idx) {
                        for diag in diags {
                            use lsp_types::DiagnosticSeverity;
                            let color = match diag.severity {
                                Some(DiagnosticSeverity::ERROR)   => theme.error,
                                Some(DiagnosticSeverity::WARNING) => theme.warning,
                                _                                 => theme.text_muted,
                            };
                            let col_start = diag.range.start.character as f32;
                            let col_end = (diag.range.end.character as f32).max(col_start + 1.0);
                            let x0 = rect.left() + col_start * char_width;
                            let x1 = rect.left() + col_end * char_width;
                            let y = rect.bottom() - 1.5;
                            // Squiggly underline via short segments
                            let step = 4.0f32;
                            let mut x = x0;
                            let mut up = true;
                            while x < x1 {
                                let next_x = (x + step).min(x1);
                                let y0 = if up { y - 1.5 } else { y };
                                let y1 = if up { y } else { y - 1.5 };
                                ui.painter().line_segment(
                                    [egui::pos2(x, y0), egui::pos2(next_x, y1)],
                                    egui::Stroke::new(1.0, color),
                                );
                                x = next_x;
                                up = !up;
                            }

                            // Hover tooltip showing message
                            let diag_msg = diag.message.clone();
                            let diag_rect = egui::Rect::from_min_max(
                                egui::pos2(x0, rect.top()),
                                egui::pos2(x1.max(x0 + 8.0), rect.bottom()),
                            );
                            let diag_resp = ui.interact(diag_rect, ui.id().with(("diag", line_idx, diag.range.start.character)), Sense::hover());
                            if diag_resp.hovered() {
                                diag_resp.on_hover_text(diag_msg);
                            }
                        }
                    }

                    // Primary cursor bar
                    if line_idx == tab.cursor.line && has_focus {
                        let cursor_x = rect.left() + tab.cursor.col as f32 * char_width;
                        ui.painter().line_segment(
                            [
                                egui::pos2(cursor_x, rect.top() + 2.0),
                                egui::pos2(cursor_x, rect.bottom() - 2.0),
                            ],
                            egui::Stroke::new(2.0, theme.accent),
                        );
                    }

                    // Extra cursor bars (multi-cursor)
                    if has_focus {
                        for &ec in &tab.extra_cursors {
                            if ec.line == line_idx {
                                let cursor_x = rect.left() + ec.col as f32 * char_width;
                                ui.painter().line_segment(
                                    [
                                        egui::pos2(cursor_x, rect.top() + 2.0),
                                        egui::pos2(cursor_x, rect.bottom() - 2.0),
                                    ],
                                    egui::Stroke::new(2.0, Color32::from_rgb(100, 220, 200)),
                                );
                            }
                        }
                    }

                    // Click handling
                    if response.clicked() || response.drag_started() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let x_offset = (pos.x - rect.left()).max(0.0);
                            let col = (x_offset / char_width).round() as usize;
                            let line_len = tab.line_text(line_idx).len();
                            let col = col.min(line_len);
                            let new_pos = TextPosition::new(line_idx, col);
                            // Alt+Click → multi-cursor; regular click → move primary cursor
                            if ui.input(|i| i.modifiers.alt) {
                                tab.add_cursor(new_pos);
                            } else {
                                tab.move_cursor_to(new_pos, false);
                            }
                        }
                    }
                    // LSP hover tracking: update hover position when mouse is over text
                    if response.hovered() {
                        if let Some(ptr_pos) = ui.input(|i| i.pointer.hover_pos()) {
                            let x_offset = (ptr_pos.x - rect.left()).max(0.0);
                            let col = (x_offset / char_width) as u32;
                            let new_hover = (line_idx as u32, col);
                            if tab.hover_pos != Some(new_hover) {
                                tab.hover_pos = Some(new_hover);
                                tab.hover_start = Some(Instant::now());
                                tab.hover_text = None;
                                tab.hover_pending = false;
                            }
                        }
                    }

                    if response.dragged() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let x_offset = (pos.x - rect.left()).max(0.0);
                            let col = (x_offset / char_width).round() as usize;
                            let line_len = tab.line_text(line_idx).len();
                            let col = col.min(line_len);
                            let drag_pos = TextPosition::new(line_idx, col);

                            let alt_held = ui.input(|i| i.modifiers.alt);
                            if alt_held {
                                // Alt+drag → column/block selection
                                let anchor = match tab.block_selection {
                                    Some((anchor, _)) => anchor,
                                    None => tab.cursor,
                                };
                                tab.block_selection = Some((anchor, drag_pos));
                                tab.selection = None; // clear normal selection
                                tab.cursor = drag_pos;
                                tab.cursor_changed = true;
                                tab.sync_legacy_cursor();
                            } else {
                                // Regular drag → normal selection
                                tab.block_selection = None;
                                if tab.selection.is_none() {
                                    tab.selection = Some(Selection::new(tab.cursor));
                                }
                                if let Some(ref mut sel) = tab.selection {
                                    sel.cursor = drag_pos;
                                }
                                tab.cursor = drag_pos;
                                tab.cursor_changed = true;
                                tab.sync_legacy_cursor();
                            }
                        }
                    }
                });

                // Fold placeholder: render a single collapsed-region indicator after fold-start
                if let Some(&fold_end) = tab.folded_lines.get(&line_idx) {
                    let folded_count = fold_end.saturating_sub(line_idx);
                    ui.horizontal(|ui| {
                        if show_line_numbers {
                            ui.add_space(gutter_width);
                        }
                        let frame = egui::Frame::none()
                            .fill(Color32::from_rgba_premultiplied(100, 100, 140, 25))
                            .rounding(egui::Rounding::same(3.0))
                            .inner_margin(egui::Margin::symmetric(8.0, 1.0));
                        frame.show(ui, |ui| {
                            ui.colored_label(
                                theme.text_muted,
                                format!("▸  {} lines", folded_count),
                            );
                        });
                    });
                }
            }

            // Space after visible region (adjusted for hidden fold lines)
            if max_visible < total_lines {
                let visible_after = (total_lines - max_visible).saturating_sub(hidden_count);
                if visible_after > 0 {
                    ui.add_space(visible_after as f32 * line_height);
                }
            }
            let _ = visible_total; // suppress unused warning

            cursor_rect
        });

    // Scroll to cursor
    if tab.cursor_changed {
        tab.cursor_changed = false;
        if let Some(cursor_rect) = scroll_output.inner {
            ui.scroll_to_rect(cursor_rect, Some(egui::Align::Center));
        }
    }
}

// ── Stateful Syntax Highlighting ──────────────────────────────────────────

/// Walk highlight state forward from last cached state up to `target_line`
fn ensure_highlight_state_up_to(
    cache: &mut HighlightCache,
    rope: &Rope,
    syntax: &syntect::parsing::SyntaxReference,
    syntax_set: &SyntaxSet,
    highlighter: &Highlighter,
    target_line: usize,
) {
    // Find the last line with a valid cached state before target
    let start_from = {
        let mut last_valid = 0usize;
        for i in (0..target_line.min(cache.states.len())).rev() {
            if cache.states[i].is_some() {
                last_valid = i;
                break;
            }
        }
        last_valid
    };

    // Get or create the initial state
    let mut cur_state = if start_from == 0 && cache.states.first().and_then(|s| s.as_ref()).is_none() {
        LineHighlightState {
            parse_state: ParseState::new(syntax),
            highlight_state: HighlightState::new(highlighter, syntect::parsing::ScopeStack::new()),
        }
    } else if start_from < cache.states.len() {
        if let Some(ref s) = cache.states[start_from] {
            s.clone()
        } else {
            LineHighlightState {
                parse_state: ParseState::new(syntax),
                highlight_state: HighlightState::new(highlighter, syntect::parsing::ScopeStack::new()),
            }
        }
    } else {
        LineHighlightState {
            parse_state: ParseState::new(syntax),
            highlight_state: HighlightState::new(highlighter, syntect::parsing::ScopeStack::new()),
        }
    };

    // Walk lines from start_from to target_line, computing and caching states.
    // Cap at 500 lines per frame to prevent freeze on large jumps; the next frame
    // continues from where we left off because states are cached incrementally.
    let walk_to = target_line.min(rope.len_lines()).min(start_from + 500);
    for line_idx in start_from..walk_to {
        // Cache state at START of this line before processing
        cache.ensure_size(line_idx + 2);
        if cache.states[line_idx].is_none() {
            cache.states[line_idx] = Some(cur_state.clone());
        }

        // Process the line to get state at start of NEXT line
        let line_text = {
            let line = rope.line(line_idx);
            line.to_string()
        };
        if let Ok(ops) = cur_state.parse_state.parse_line(&line_text, syntax_set) {
            // Consume iterator to advance highlight state (3-tuple: style, text, range)
            for _ in RangedHighlightIterator::new(
                &mut cur_state.highlight_state,
                &ops,
                &line_text,
                highlighter,
            ) {}
        }
    }
}

/// Highlight a single line using cached parse state (stateful, correct multi-line)
fn highlight_line_stateful(
    line_text: &str,
    cache: &mut HighlightCache,
    line_idx: usize,
    _syntax: &syntect::parsing::SyntaxReference,
    syntax_set: &SyntaxSet,
    highlighter: &Highlighter,
    font_size: f32,
    theme: &ThemeColors,
) -> LayoutJob {
    let mut job = LayoutJob::default();

    if line_text.is_empty() {
        job.append(" ", 0.0, TextFormat {
            font_id: FontId::monospace(font_size),
            color: theme.text,
            ..Default::default()
        });
        return job;
    }

    // Check per-line LayoutJob cache — skip syntect entirely if line unchanged
    if line_idx < cache.job_cache.len() {
        if let Some((ref cached_text, ref cached_job)) = cache.job_cache[line_idx] {
            if cached_text == line_text {
                return cached_job.clone();
            }
        }
    }

    // Get or compute the state at the start of this line
    let state = if line_idx < cache.states.len() {
        cache.states[line_idx].clone()
    } else {
        None
    };

    let Some(mut line_state) = state else {
        return plain_layout(line_text, font_size, theme);
    };

    // Highlight with newline suffix so syntect gets correct EOL state
    let line_with_nl = if line_text.ends_with('\n') {
        line_text.to_string()
    } else {
        format!("{}\n", line_text)
    };

    if let Ok(ops) = line_state.parse_state.parse_line(&line_with_nl, syntax_set) {
        let ranges: Vec<_> = RangedHighlightIterator::new(
            &mut line_state.highlight_state,
            &ops,
            &line_with_nl,
            highlighter,
        ).collect();

        // Cache updated state for next line
        cache.ensure_size(line_idx + 2);
        cache.states[line_idx + 1] = Some(line_state);

        for (style, text, _range) in ranges {
            let text = text.trim_end_matches('\n');
            if text.is_empty() { continue; }
            let color = Color32::from_rgba_premultiplied(
                style.foreground.r,
                style.foreground.g,
                style.foreground.b,
                style.foreground.a,
            );
            job.append(text, 0.0, TextFormat {
                font_id: FontId::monospace(font_size),
                color,
                ..Default::default()
            });
        }

        if job.sections.is_empty() {
            job.append(line_text, 0.0, TextFormat {
                font_id: FontId::monospace(font_size),
                color: theme.text,
                ..Default::default()
            });
        }
    } else {
        return plain_layout(line_text, font_size, theme);
    }

    // Store in job cache so future frames skip syntect for this unchanged line
    if line_idx < cache.job_cache.len() {
        cache.job_cache[line_idx] = Some((line_text.to_string(), job.clone()));
    }

    job
}

fn plain_layout(line: &str, font_size: f32, theme: &ThemeColors) -> LayoutJob {
    let mut job = LayoutJob::default();
    let text = if line.is_empty() { " " } else { line };
    job.append(text, 0.0, TextFormat {
        font_id: FontId::monospace(font_size),
        color: theme.text,
        ..Default::default()
    });
    job
}

fn detect_language(path: &std::path::Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string()
}

/// Build breadcrumbs by scanning backward from the cursor to find enclosing scopes.
/// Returns e.g. ["my_file.rs", "impl MyStruct", "fn my_method"]
fn build_breadcrumbs(tab: &EditorTab) -> Vec<String> {
    let mut crumbs = Vec::new();

    // File name crumb
    if let Some(ref path) = tab.path {
        if let Some(name) = path.file_name() {
            crumbs.push(name.to_string_lossy().to_string());
        }
    } else {
        crumbs.push(tab.title.clone());
    }

    // Scan upward from cursor to find enclosing scopes (for Rust/Python/JS/TS)
    let cursor_line = tab.cursor.line;
    let total_lines = tab.rope.len_lines();
    let scan_limit = cursor_line.min(total_lines);

    let keywords = ["impl ", "fn ", "struct ", "enum ", "trait ", "mod ", "class ", "def ", "function ", "async fn "];

    let mut scopes: Vec<String> = Vec::new();
    let mut brace_depth: i32 = 0;
    let mut last_depth_at: Vec<(i32, String)> = Vec::new(); // (brace_depth, label)

    for line_idx in 0..=scan_limit {
        if line_idx >= total_lines { break; }
        let line_text = tab.rope.line(line_idx).to_string();
        let trimmed = line_text.trim();

        // Count braces on this line
        let open_count = trimmed.chars().filter(|&c| c == '{').count() as i32;
        let close_count = trimmed.chars().filter(|&c| c == '}').count() as i32;

        // Check if this line declares a scope
        let is_scope_line = keywords.iter().any(|kw| trimmed.starts_with(kw) || trimmed.contains(kw));

        if is_scope_line && open_count > 0 {
            // Extract the label: first keyword match
            let label = keywords.iter()
                .filter_map(|kw| {
                    let pos = trimmed.find(kw)?;
                    let after = &trimmed[pos + kw.len()..];
                    // Take up to first `(`, `{`, `<`, or whitespace after an ident
                    let ident: String = after.chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '<' || *c == '>')
                        .collect();
                    if ident.is_empty() { None } else { Some(format!("{}{}", kw.trim_end(), ident)) }
                })
                .next()
                .unwrap_or_else(|| trimmed.chars().take(30).collect());
            last_depth_at.push((brace_depth, label));
        }

        brace_depth += open_count - close_count;

        // Remove scopes that are now closed
        last_depth_at.retain(|(d, _)| *d < brace_depth);
    }

    // Build the scope chain from last_depth_at (ascending depth order)
    for (_, label) in &last_depth_at {
        scopes.push(label.clone());
    }

    crumbs.extend(scopes);
    crumbs
}

/// Parse `git diff --unified=0` output and populate a map of line_idx → GitLineStatus.
/// Line indices are 0-based (subtract 1 from the diff's 1-based line numbers).
fn parse_git_diff_hunks(diff: &str, hunks: &mut HashMap<usize, GitLineStatus>) {
    for line in diff.lines() {
        if !line.starts_with("@@") { continue; }
        // Format: @@ -orig_start[,orig_count] +new_start[,new_count] @@
        let inner = line.trim_start_matches('@').trim_start().trim_end_matches('@').trim();
        let parts: Vec<&str> = inner.split_whitespace().collect();
        if parts.len() < 2 { continue; }

        let old_part = parts[0].trim_start_matches('-');
        let new_part = parts[1].trim_start_matches('+');

        let (_old_start, old_count) = parse_hunk_range(old_part);
        let (new_start, new_count) = parse_hunk_range(new_part);

        if new_count == 0 {
            // Pure deletion — mark the line before the deletion point
            if new_start > 0 {
                hunks.entry(new_start - 1).or_insert(GitLineStatus::Deleted);
            }
        } else if old_count == 0 {
            // Pure addition — all new lines are Added
            for l in new_start..new_start + new_count {
                hunks.insert(l, GitLineStatus::Added);
            }
        } else {
            // Mixed: first min(old,new) lines are Modified, rest are Added
            let modified_count = old_count.min(new_count);
            for l in new_start..new_start + modified_count {
                hunks.insert(l, GitLineStatus::Modified);
            }
            for l in new_start + modified_count..new_start + new_count {
                hunks.insert(l, GitLineStatus::Added);
            }
        }
    }
}

fn parse_hunk_range(s: &str) -> (usize, usize) {
    if let Some(comma) = s.find(',') {
        let start: usize = s[..comma].parse().unwrap_or(1);
        let count: usize = s[comma + 1..].parse().unwrap_or(1);
        // Git uses 1-based lines; convert to 0-based
        (start.saturating_sub(1), count)
    } else {
        let start: usize = s.parse().unwrap_or(1);
        (start.saturating_sub(1), 1)
    }
}

/// Parse `git blame --porcelain` output into a blame cache.
/// Porcelain format: each hunk starts with "<40-char hash> <orig-line> <final-line> [<lines>]"
/// followed by key-value pairs (author, author-time, summary, etc.), ending with a tab-prefixed line.
fn parse_blame_porcelain(text: &str, cache: &mut HashMap<usize, BlameInfo>) {
    let mut lines = text.lines().peekable();
    let mut current_hash = String::new();
    let mut current_author = String::new();
    let mut current_date = String::new();
    let mut current_summary = String::new();
    let mut current_final_line: usize = 0;

    while let Some(line) = lines.next() {
        // Hunk header: 40-char hex hash followed by orig-line final-line [count]
        if line.len() >= 40 && line.chars().take(40).all(|c| c.is_ascii_hexdigit()) {
            let parts: Vec<&str> = line.splitn(4, ' ').collect();
            current_hash = parts[0][..8].to_string(); // short hash (8 chars)
            current_final_line = parts.get(2)
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1)
                .saturating_sub(1); // convert to 0-based

            // Reset per-hunk fields
            current_author.clear();
            current_date.clear();
            current_summary.clear();
        } else if line.starts_with("author ") {
            current_author = line[7..].trim().to_string();
        } else if line.starts_with("author-time ") {
            // Unix timestamp → "YYYY-MM-DD"
            if let Ok(ts) = line[12..].trim().parse::<i64>() {
                let days = ts / 86400;
                // Simple approximation: epoch 1970-01-01 + days
                let year = 1970 + days / 365;
                current_date = format!("{year}");
            }
        } else if line.starts_with("summary ") {
            current_summary = line[8..].trim().to_string();
        } else if line.starts_with('\t') {
            // This is the actual line content — end of this hunk's metadata
            if !current_hash.is_empty() {
                cache.insert(current_final_line, BlameInfo {
                    short_hash: current_hash.clone(),
                    author: current_author.clone(),
                    date: current_date.clone(),
                    summary: current_summary.clone(),
                });
            }
        }
    }
}
