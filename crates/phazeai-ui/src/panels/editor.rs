use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use floem::{
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, Modifiers},
    kurbo::{Circle, Point},
    reactive::{create_effect, create_memo, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    text::{Attrs, AttrsList, FamilyOwned, Stretch, Style as TextStyle, Weight},
    views::{
        canvas, container, dyn_stack,
        editor::{
            core::{
                buffer::rope_text::RopeText,
                cursor::{Cursor, CursorMode},
                editor::EditType,
                selection::{SelRegion, Selection},
            },
            id::EditorId,
            layout::{LineExtraStyle, TextLayoutLine},
            text::{default_dark_color, Document, SimpleStylingBuilder, Styling, WrapMethod},
            EditorStyle,
        },
        label, scroll, stack, text_editor, text_input, Decorators,
    },
    IntoView, Renderer,
};

use crate::lsp_bridge::DiagSeverity;
use crate::util::safe_get;
use lazy_static::lazy_static;
use syntect::{
    highlighting::{FontStyle, HighlightState, Highlighter, RangedHighlightIterator, ThemeSet},
    parsing::{ParseState, ScopeStack, SyntaxSet},
};

use phazeai_core::{llm::Message, Settings};

use crate::{
    components::icon::{icons, phaze_icon},
    theme::PhazeTheme,
};

// ── Syntect globals (lazy_static → 'static lifetimes) ────────────────────────

lazy_static! {
    static ref SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
    static ref THEME_SET: ThemeSet = ThemeSet::load_defaults();
}

// ── Syntax Highlighting Styling ───────────────────────────────────────────────

/// A `Styling` implementation that uses syntect for per-line syntax highlighting.
/// Wraps an inner `Rc<dyn Styling>` (the SimpleStylingBuilder output) for
/// font/layout settings and adds color spans from syntect on top.
struct SyntaxStyle {
    inner: Rc<dyn Styling>,
    highlighter: Highlighter<'static>,
    parse_state_proto: ParseState,
    doc: Option<Rc<dyn Document>>,
    /// Cached incremental states, one entry per 16-line block.
    states: RefCell<Vec<(ParseState, HighlightState)>>,
    /// Diagnostic lines for this file: (0-based line index, severity).
    diag_lines: Vec<(usize, DiagSeverity)>,
    /// Word highlight ranges as byte offsets into the document.
    highlight_ranges: Vec<(usize, usize)>,
    /// Git gutter lines: (0-based line index, status).
    /// Status: 0 = added, 1 = modified, 2 = deleted.
    git_lines: Vec<(usize, u8)>,
    /// Git gutter colors from the active theme palette.
    git_color_added: floem::peniko::Color,
    git_color_modified: floem::peniko::Color,
    git_color_deleted: floem::peniko::Color,
    /// 0-based index of the currently active (cursor) line — receives a subtle background highlight.
    current_line: usize,
    /// Foldable regions: `(start_line, end_line)` pairs detected from braces/brackets.
    foldable_ranges: Vec<(usize, usize)>,
    /// Set of fold-start lines that are currently collapsed (hidden).
    folded_starts: HashSet<usize>,
    /// All find-bar match byte ranges `(start, end)` — highlighted with a distinct box.
    find_match_ranges: Vec<(usize, usize)>,
    /// The matching bracket pair `(open_byte, close_byte)` for the bracket under cursor.
    matching_bracket: Option<(usize, usize)>,
    /// Bracket pairs with depths: `(open_byte, close_byte, depth)` for colorization.
    bracket_pairs: Vec<(usize, usize, usize)>,
    /// Character width in pixels (approximated from font_size) for indent guide placement.
    char_width_px: f64,
    /// Inline git blame: `(0-based line, "author • date")` — only for the current cursor line.
    blame_line: Option<(usize, String)>,
    /// Bracket pair guides: (open_line, open_col_chars, close_line, depth) for vertical lines.
    bracket_pair_guides: Vec<(usize, usize, usize, usize)>,
    /// Last known rope length for cache invalidation. If rope length changes,
    /// the entire states cache is cleared to prevent stale highlighting.
    last_rope_len: std::cell::Cell<usize>,
}

impl SyntaxStyle {
    /// Create a `SyntaxStyle` for the given file extension.
    /// Falls back to plain-text if no matching grammar is found.
    fn for_extension(ext: &str, inner: Rc<dyn Styling>) -> Self {
        let theme = &THEME_SET.themes["base16-ocean.dark"];
        let highlighter = Highlighter::new(theme);

        // Map common extensions → syntect scope names
        let syntax = match ext {
            "rs" => SYNTAX_SET.find_syntax_by_extension("rs"),
            "py" | "pyw" => SYNTAX_SET.find_syntax_by_extension("py"),
            "js" | "mjs" | "cjs" => SYNTAX_SET.find_syntax_by_extension("js"),
            "ts" | "tsx" => SYNTAX_SET.find_syntax_by_extension("ts"),
            "jsx" => SYNTAX_SET.find_syntax_by_extension("jsx"),
            "json" | "jsonc" => SYNTAX_SET.find_syntax_by_extension("json"),
            "toml" => SYNTAX_SET.find_syntax_by_extension("toml"),
            "md" | "mdx" | "markdown" => SYNTAX_SET.find_syntax_by_extension("md"),
            "html" | "htm" => SYNTAX_SET.find_syntax_by_extension("html"),
            "css" => SYNTAX_SET.find_syntax_by_extension("css"),
            "scss" | "sass" => SYNTAX_SET.find_syntax_by_extension("scss"),
            "c" | "h" => SYNTAX_SET.find_syntax_by_extension("c"),
            "cpp" | "cc" | "cxx" | "hpp" => SYNTAX_SET.find_syntax_by_extension("cpp"),
            "go" => SYNTAX_SET.find_syntax_by_extension("go"),
            "sh" | "bash" | "zsh" => SYNTAX_SET.find_syntax_by_extension("sh"),
            "yaml" | "yml" => SYNTAX_SET.find_syntax_by_extension("yaml"),
            "xml" => SYNTAX_SET.find_syntax_by_extension("xml"),
            "sql" => SYNTAX_SET.find_syntax_by_extension("sql"),
            "lua" => SYNTAX_SET.find_syntax_by_extension("lua"),
            "rb" => SYNTAX_SET.find_syntax_by_extension("rb"),
            "java" => SYNTAX_SET.find_syntax_by_extension("java"),
            "kt" | "kts" => SYNTAX_SET.find_syntax_by_extension("kt"),
            "swift" => SYNTAX_SET.find_syntax_by_extension("swift"),
            "cs" => SYNTAX_SET.find_syntax_by_extension("cs"),
            _ => None,
        }
        .or_else(|| SYNTAX_SET.find_syntax_plain_text().into())
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

        let parse_state_proto = ParseState::new(syntax);

        Self {
            inner,
            highlighter,
            parse_state_proto,
            doc: None,
            states: RefCell::new(Vec::new()),
            diag_lines: Vec::new(),
            highlight_ranges: Vec::new(),
            git_lines: Vec::new(),
            git_color_added: floem::peniko::Color::from_rgba8(80, 200, 80, 220),
            git_color_modified: floem::peniko::Color::from_rgba8(80, 160, 255, 220),
            git_color_deleted: floem::peniko::Color::from_rgba8(220, 60, 60, 220),
            current_line: 0,
            foldable_ranges: Vec::new(),
            folded_starts: HashSet::new(),
            find_match_ranges: Vec::new(),
            matching_bracket: None,
            bracket_pairs: Vec::new(),
            char_width_px: 8.4,
            blame_line: None,
            bracket_pair_guides: Vec::new(),
            last_rope_len: std::cell::Cell::new(0),
        }
    }

    fn set_doc(&mut self, doc: Rc<dyn Document>) {
        self.doc = Some(doc);
    }
}

// ── Word-highlight helpers ─────────────────────────────────────────────────

/// Returns (start_byte, end_byte, word) for the identifier/word under `offset`,
/// or `None` if the cursor is not on an identifier character.
fn word_at_offset(text: &str, offset: usize) -> Option<(usize, usize, String)> {
    if offset > text.len() {
        return None;
    }
    // Pick an offset that is a valid char boundary
    let offset = text[..offset]
        .char_indices()
        .next_back()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let ch = text[offset..].chars().next()?;
    if !ch.is_alphanumeric() && ch != '_' {
        return None;
    }

    // Walk backwards to word start
    let mut start = offset;
    for (i, c) in text[..offset].char_indices().rev() {
        if c.is_alphanumeric() || c == '_' {
            start = i;
        } else {
            break;
        }
    }

    // Walk forwards to word end
    let mut end = offset;
    for c in text[offset..].chars() {
        if c.is_alphanumeric() || c == '_' {
            end += c.len_utf8();
        } else {
            break;
        }
    }

    if start == end {
        return None;
    }
    Some((start, end, text[start..end].to_string()))
}

/// Finds all whole-word occurrences of `word` in `text`.
/// Returns a `Vec` of `(start_byte, end_byte)` pairs.
fn find_word_occurrences(text: &str, word: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut search_from = 0usize;
    while let Some(pos) = text[search_from..].find(word) {
        let abs_start = search_from + pos;
        let abs_end = abs_start + word.len();
        // Whole-word boundary check
        let before_ok = abs_start == 0 || {
            let c = text[..abs_start].chars().last().unwrap_or(' ');
            !c.is_alphanumeric() && c != '_'
        };
        let after_ok = abs_end >= text.len() || {
            let c = text[abs_end..].chars().next().unwrap_or(' ');
            !c.is_alphanumeric() && c != '_'
        };
        if before_ok && after_ok {
            ranges.push((abs_start, abs_end));
        }
        search_from = abs_start + word.len().max(1);
    }
    ranges
}

// ── Git diff parser ────────────────────────────────────────────────────────

/// Run `git diff HEAD -- <path>` and parse changed lines for the new file.
/// Returns `(line_0based, status)` where status: 0=added, 1=modified, 2=deleted_marker.
fn git_changed_lines(path: &std::path::Path) -> Vec<(usize, u8)> {
    // Determine git root (walk up from file's dir)
    let dir = path.parent().unwrap_or(path);
    let out = match std::process::Command::new("git")
        .args(["diff", "HEAD", "--", path.to_str().unwrap_or("")])
        .current_dir(dir)
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    if !out.status.success() && out.stdout.is_empty() {
        return vec![];
    }

    let diff = match std::str::from_utf8(&out.stdout) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let mut result: Vec<(usize, u8)> = Vec::new();
    let mut new_line: usize = 0;
    let mut hunk_active = false;
    let mut hunk_has_del = false;

    for line in diff.lines() {
        if line.starts_with("@@ ") {
            // "@@ -old_start[,old_count] +new_start[,new_count] @@"
            hunk_active = true;
            hunk_has_del = false;
            // Extract +new_start
            if let Some(plus_pos) = line.find('+') {
                let rest = &line[plus_pos + 1..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                new_line = digits.parse::<usize>().unwrap_or(1).saturating_sub(1);
            }
            continue;
        }
        if !hunk_active {
            continue;
        }

        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }

        if line.starts_with('-') {
            hunk_has_del = true;
            // Deletion in old file — no new_line advance, but mark the preceding new line
            if new_line > 0 {
                result.push((new_line.saturating_sub(1), 2)); // deleted indicator
            }
        } else if line.starts_with('+') {
            let status = if hunk_has_del { 1 } else { 0 }; // modified or added
            result.push((new_line, status));
            new_line += 1;
        } else {
            // Context line
            new_line += 1;
            hunk_has_del = false;
        }
    }

    // Deduplicate: prefer "modified" over "added" for same line
    result.sort_by_key(|(l, _)| *l);
    result.dedup_by_key(|(l, _)| *l);
    result
}

impl Styling for SyntaxStyle {
    fn id(&self) -> u64 {
        self.inner.id()
    }

    fn font_size(&self, edid: EditorId, line: usize) -> usize {
        self.inner.font_size(edid, line)
    }

    fn line_height(&self, edid: EditorId, line: usize) -> f32 {
        // Return 0 for lines collapsed inside an active fold.
        for &(fold_start, fold_end) in &self.foldable_ranges {
            if self.folded_starts.contains(&fold_start) && line > fold_start && line <= fold_end {
                return 0.0;
            }
        }
        self.inner.line_height(edid, line)
    }

    fn font_family(&self, edid: EditorId, line: usize) -> Cow<'_, [FamilyOwned]> {
        self.inner.font_family(edid, line)
    }

    fn weight(&self, edid: EditorId, line: usize) -> Weight {
        self.inner.weight(edid, line)
    }

    fn italic_style(&self, edid: EditorId, line: usize) -> TextStyle {
        self.inner.italic_style(edid, line)
    }

    fn stretch(&self, edid: EditorId, line: usize) -> Stretch {
        self.inner.stretch(edid, line)
    }

    fn indent_line(&self, edid: EditorId, line: usize, line_content: &str) -> usize {
        self.inner.indent_line(edid, line, line_content)
    }

    fn tab_width(&self, edid: EditorId, line: usize) -> usize {
        self.inner.tab_width(edid, line)
    }

    fn atomic_soft_tabs(&self, edid: EditorId, line: usize) -> bool {
        self.inner.atomic_soft_tabs(edid, line)
    }

    fn apply_attr_styles(
        &self,
        _edid: EditorId,
        _style: &EditorStyle,
        line: usize,
        default: Attrs,
        attrs: &mut AttrsList,
    ) {
        attrs.clear_spans();
        let Some(doc) = &self.doc else { return };

        // Invalidate cache when rope length changes (edit happened between saves)
        if let Some(doc) = &self.doc {
            let current_len = doc.rope_text().len();
            if current_len != self.last_rope_len.get() {
                self.states.borrow_mut().clear();
                self.last_rope_len.set(current_len);
            }
        }

        let mut states_cache = self.states.borrow_mut();
        // Rebuild cache up to the nearest 16-line boundary before `line`
        let start = (line >> 4).min(states_cache.len());
        states_cache.truncate(start);

        // Seed from the cached state or from scratch
        let mut states = states_cache.last().cloned().unwrap_or_else(|| {
            (
                self.parse_state_proto.clone(),
                HighlightState::new(&self.highlighter, ScopeStack::new()),
            )
        });

        let rope = doc.rope_text();
        for line_no in start..=line {
            let text = rope.line_content(line_no).to_string();
            if let Ok(ops) = states.0.parse_line(&text, &SYNTAX_SET) {
                if line_no == line {
                    for (style, _text, range) in
                        RangedHighlightIterator::new(&mut states.1, &ops, &text, &self.highlighter)
                    {
                        let mut attr = default.clone();
                        if style.font_style.contains(FontStyle::ITALIC) {
                            attr = attr.style(TextStyle::Italic);
                        }
                        if style.font_style.contains(FontStyle::BOLD) {
                            attr = attr.weight(Weight::BOLD);
                        }
                        attr = attr.color(floem::peniko::Color::from_rgba8(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                            style.foreground.a,
                        ));
                        attrs.add_span(range, attr);
                    }
                }
            }

            // Cache state every 16 lines
            if line_no & 0xF == 0xF {
                states_cache.push(states.clone());
            }
        }

        // ── Bracket pair colorization ──────────────────────────────────────
        // Assign cycling colors to bracket pairs based on their nesting depth.
        if !self.bracket_pairs.is_empty() {
            let rope = doc.rope_text();
            let line_start = rope.offset_of_line(line);
            let line_end = if line + 1 < rope.num_lines() {
                rope.offset_of_line(line + 1)
            } else {
                rope.len()
            };
            // 4 distinct bracket colors cycling by depth
            const BRACKET_COLORS: [(u8, u8, u8); 4] = [
                (255, 215, 0),   // gold
                (150, 220, 255), // sky blue
                (200, 130, 255), // violet
                (120, 240, 160), // mint
            ];
            for &(open_b, close_b, depth) in &self.bracket_pairs {
                let color = BRACKET_COLORS[depth % 4];
                let c = floem::peniko::Color::from_rgba8(color.0, color.1, color.2, 210);
                // Open bracket
                if open_b >= line_start && open_b < line_end {
                    let local = open_b - line_start;
                    attrs.add_span(local..local + 1, default.clone().color(c));
                }
                // Close bracket
                if close_b >= line_start && close_b < line_end {
                    let local = close_b - line_start;
                    attrs.add_span(local..local + 1, default.clone().color(c));
                }
            }
        }
    }

    fn apply_layout_styles(
        &self,
        edid: EditorId,
        style: &EditorStyle,
        line: usize,
        layout_line: &mut TextLayoutLine,
    ) {
        self.inner
            .apply_layout_styles(edid, style, line, layout_line);

        // Subtle background highlight on the current (cursor) line.
        if line == self.current_line {
            let line_h = self.inner.line_height(edid, line) as f64;
            layout_line.extra_style.push(LineExtraStyle {
                x: 0.0,
                y: 0.0,
                width: Some(10000.0),
                height: line_h,
                bg_color: Some(floem::peniko::Color::from_rgba8(255, 255, 255, 12)),
                under_line: None,
                wave_line: None,
            });
        }

        // Draw wave_line (error) or under_line (warning/info) for diagnostic lines.
        for &(diag_line, severity) in &self.diag_lines {
            if diag_line != line {
                continue;
            }
            let color = match severity {
                DiagSeverity::Error => floem::peniko::Color::from_rgba8(255, 85, 85, 230),
                DiagSeverity::Warning => floem::peniko::Color::from_rgba8(255, 200, 50, 200),
                DiagSeverity::Info => floem::peniko::Color::from_rgba8(80, 150, 255, 180),
                DiagSeverity::Hint => floem::peniko::Color::from_rgba8(120, 180, 120, 160),
            };
            layout_line.extra_style.push(LineExtraStyle {
                x: 0.0,
                y: 0.0,
                width: None, // full line width
                height: 2.0,
                bg_color: None,
                under_line: if matches!(
                    severity,
                    DiagSeverity::Warning | DiagSeverity::Info | DiagSeverity::Hint
                ) {
                    Some(color)
                } else {
                    None
                },
                wave_line: if severity == DiagSeverity::Error {
                    Some(color)
                } else {
                    None
                },
            });
        }

        // Draw git gutter decorations: a 3 px colored bar at x=0 on each changed line.
        for &(git_line, status) in &self.git_lines {
            if git_line != line {
                continue;
            }
            let color = match status {
                0 => self.git_color_added,
                1 => self.git_color_modified,
                _ => self.git_color_deleted,
            };
            let line_h = self.inner.line_height(edid, line) as f64;
            layout_line.extra_style.push(LineExtraStyle {
                x: 0.0,
                y: 0.0,
                width: Some(3.0),
                height: line_h,
                bg_color: Some(color),
                under_line: None,
                wave_line: None,
            });
        }

        // Draw fold indicator (bright = collapsed, dim = expanded) in gutter.
        self.paint_fold_indicator(edid, line, layout_line);

        // ── Indent guides ─────────────────────────────────────────────────
        // Draw a 1px vertical line at each indent level for the current line.
        if let Some(doc) = &self.doc {
            let rope = doc.rope_text();
            let line_start = rope.offset_of_line(line);
            let line_end = if line + 1 < rope.num_lines() {
                rope.offset_of_line(line + 1)
            } else {
                rope.len()
            };
            let line_text = rope.slice_to_cow(line_start..line_end).to_string();
            let leading_spaces = line_text
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .count();
            // Draw guide at each 4-space indent level
            if leading_spaces >= 4 {
                let line_h = self.inner.line_height(edid, line) as f64;
                let mut indent = 4usize;
                while indent <= leading_spaces {
                    let x = (indent as f64) * self.char_width_px;
                    layout_line.extra_style.push(LineExtraStyle {
                        x,
                        y: 0.0,
                        width: Some(1.0),
                        height: line_h,
                        bg_color: Some(floem::peniko::Color::from_rgba8(120, 120, 140, 35)),
                        under_line: None,
                        wave_line: None,
                    });
                    indent += 4;
                }
            }
        }

        // ── Bracket pair guides ───────────────────────────────────────────────
        // Draw 1px vertical lines at the open-bracket column from open_line+1
        // to close_line, connecting matching `{}`/`()`/`[]` pairs across lines.
        for &(open_line, open_col, close_line, depth) in &self.bracket_pair_guides {
            if line <= open_line || line > close_line {
                continue;
            }
            let x = (open_col as f64) * self.char_width_px + 1.0; // +1 to be inside the char
            let line_h = self.inner.line_height(edid, line) as f64;
            // Cycle through 4 colors by depth
            let color = match depth % 4 {
                0 => floem::peniko::Color::from_rgba8(255, 215, 0, 50), // gold
                1 => floem::peniko::Color::from_rgba8(86, 182, 194, 50), // sky blue
                2 => floem::peniko::Color::from_rgba8(198, 120, 221, 50), // violet
                _ => floem::peniko::Color::from_rgba8(152, 195, 121, 50), // mint
            };
            layout_line.extra_style.push(LineExtraStyle {
                x,
                y: 0.0,
                width: Some(1.0),
                height: line_h,
                bg_color: Some(color),
                under_line: None,
                wave_line: None,
            });
        }

        // ── Find-bar: highlight ALL matches ─────────────────────────────────
        // Distinct yellow boxes on every search match (not just the jump target).
        if !self.find_match_ranges.is_empty() {
            if let Some(doc) = &self.doc {
                let rope = doc.rope_text();
                let line_start = rope.offset_of_line(line);
                let line_end = if line + 1 < rope.num_lines() {
                    rope.offset_of_line(line + 1)
                } else {
                    rope.len()
                };
                let line_h = self.inner.line_height(edid, line) as f64;
                for &(start, end) in &self.find_match_ranges {
                    if end <= line_start || start >= line_end {
                        continue;
                    }
                    let local_start = start.saturating_sub(line_start);
                    let local_end = end.min(line_end).saturating_sub(line_start);
                    let x0 = layout_line.text.hit_position(local_start).point.x;
                    let x1 = layout_line.text.hit_position(local_end).point.x;
                    layout_line.extra_style.push(LineExtraStyle {
                        x: x0,
                        y: 0.0,
                        width: Some((x1 - x0).max(4.0)),
                        height: line_h,
                        bg_color: Some(floem::peniko::Color::from_rgba8(255, 230, 0, 55)),
                        under_line: None,
                        wave_line: None,
                    });
                }
            }
        }

        // ── Matching bracket highlight ─────────────────────────────────────
        // Draw a bright box on both the bracket under cursor and its match.
        if let Some((open_b, close_b)) = self.matching_bracket {
            if let Some(doc) = &self.doc {
                let rope = doc.rope_text();
                let line_start = rope.offset_of_line(line);
                let line_end = if line + 1 < rope.num_lines() {
                    rope.offset_of_line(line + 1)
                } else {
                    rope.len()
                };
                let line_h = self.inner.line_height(edid, line) as f64;
                let color = floem::peniko::Color::from_rgba8(255, 255, 255, 60);
                for byte_pos in [open_b, close_b] {
                    if byte_pos >= line_start && byte_pos < line_end {
                        let local = byte_pos - line_start;
                        let x0 = layout_line.text.hit_position(local).point.x;
                        let x1 = layout_line.text.hit_position(local + 1).point.x;
                        layout_line.extra_style.push(LineExtraStyle {
                            x: x0,
                            y: 0.0,
                            width: Some((x1 - x0).max(8.0)),
                            height: line_h,
                            bg_color: Some(color),
                            under_line: Some(floem::peniko::Color::from_rgba8(255, 255, 160, 200)),
                            wave_line: None,
                        });
                    }
                }
            }
        }

        // Draw word/symbol highlight boxes using pixel-accurate positions.
        if !self.highlight_ranges.is_empty() {
            if let Some(doc) = &self.doc {
                let rope = doc.rope_text();
                let line_start = rope.offset_of_line(line);
                let line_end = if line + 1 < rope.num_lines() {
                    rope.offset_of_line(line + 1)
                } else {
                    rope.len()
                };
                let line_h = self.inner.line_height(edid, line) as f64;

                for &(hl_start, hl_end) in &self.highlight_ranges {
                    if hl_end <= line_start || hl_start >= line_end {
                        continue;
                    }
                    // Clamp to line boundaries (multi-line match safety)
                    let local_start = hl_start.saturating_sub(line_start);
                    let local_end = hl_end.min(line_end).saturating_sub(line_start);
                    // Use layout hit_position for pixel-accurate x coords
                    let x0 = layout_line.text.hit_position(local_start).point.x;
                    let x1 = layout_line.text.hit_position(local_end).point.x;
                    layout_line.extra_style.push(LineExtraStyle {
                        x: x0,
                        y: 0.0,
                        width: Some((x1 - x0).max(2.0)),
                        height: line_h,
                        bg_color: Some(floem::peniko::Color::from_rgba8(100, 160, 255, 50)),
                        under_line: None,
                        wave_line: None,
                    });
                }
            }
        }
    }

    fn paint_caret(&self, edid: EditorId, line: usize) -> bool {
        self.inner.paint_caret(edid, line)
    }
}

// ── apply_layout_styles fold indicator insertion ─────────────────────────────

impl SyntaxStyle {
    /// Called at the end of `apply_layout_styles` to paint a small colored
    /// square in the gutter area indicating foldable regions.
    fn paint_fold_indicator(&self, edid: EditorId, line: usize, layout_line: &mut TextLayoutLine) {
        for &(fold_start, _fold_end) in &self.foldable_ranges {
            if fold_start != line {
                continue;
            }
            let is_folded = self.folded_starts.contains(&fold_start);
            // Bright when collapsed (▶), dim when expanded (▼).
            let color = if is_folded {
                floem::peniko::Color::from_rgba8(100, 180, 255, 220)
            } else {
                floem::peniko::Color::from_rgba8(100, 180, 255, 80)
            };
            let line_h = self.inner.line_height(edid, line) as f64;
            let sq = 6.0_f64;
            layout_line.extra_style.push(LineExtraStyle {
                x: -12.0, // negative x → gutter area
                y: (line_h - sq).max(0.0) * 0.5,
                width: Some(sq),
                height: sq,
                bg_color: Some(color),
                under_line: None,
                wave_line: None,
            });
            break;
        }
    }
}

// ── Fold range detection ──────────────────────────────────────────────────────

/// Detects foldable regions from source text using brace/bracket matching.
/// Returns `Vec<(start_line, end_line)>` where `start_line` has the `{`
/// and `end_line` has the matching `}`.  Ranges with start==end are excluded.
fn detect_fold_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut stack: Vec<usize> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut in_string = false;
    let mut string_char = '"';
    let mut prev_char = '\0';

    for (line_idx, line) in text.lines().enumerate() {
        for ch in line.chars() {
            if in_string {
                if ch == string_char && prev_char != '\\' {
                    in_string = false;
                }
            } else {
                match ch {
                    '"' | '\'' => {
                        in_string = true;
                        string_char = ch;
                    }
                    '{' => stack.push(line_idx),
                    '}' => {
                        if let Some(start) = stack.pop() {
                            if line_idx > start {
                                ranges.push((start, line_idx));
                            }
                        }
                    }
                    _ => {}
                }
            }
            prev_char = ch;
        }
        // Strings don't span lines in this simplified parser.
        if in_string {
            in_string = false;
        }
        prev_char = '\0';
    }
    // Sort by start line for consistent ordering.
    ranges.sort_by_key(|&(s, _)| s);
    ranges
}

// ── Bracket pair detection ────────────────────────────────────────────────────

/// Detects all bracket pairs in `text` and returns `(open_byte, close_byte, depth)`.
/// Only `{`, `(`, `[` pairs are tracked; depth is 0-based nesting level.
fn compute_bracket_pairs(text: &str) -> Vec<(usize, usize, usize)> {
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (byte_pos, depth)
    let mut result: Vec<(usize, usize, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut string_char = '"';
    let mut in_line_comment = false;
    let mut prev = '\0';
    let mut byte_pos = 0usize;

    for ch in text.chars() {
        let ch_len = ch.len_utf8();

        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
        } else if in_string {
            if ch == string_char && prev != '\\' {
                in_string = false;
            }
        } else {
            match ch {
                '/' if prev == '/' => {
                    in_line_comment = true;
                }
                '"' | '\'' => {
                    in_string = true;
                    string_char = ch;
                }
                '(' | '[' | '{' => {
                    stack.push((byte_pos, depth));
                    depth += 1;
                }
                ')' | ']' | '}' => {
                    if let Some((open_pos, open_depth)) = stack.pop() {
                        result.push((open_pos, byte_pos, open_depth));
                        depth = open_depth;
                    }
                }
                _ => {}
            }
        }

        prev = ch;
        byte_pos += ch_len;
    }
    result
}

/// Given text and a byte offset, return `(open_byte, close_byte)` for the bracket at
/// that offset (or `None` if no bracket is there).
fn find_bracket_match(
    _text: &str,
    offset: usize,
    pairs: &[(usize, usize, usize)],
) -> Option<(usize, usize)> {
    for &(open, close, _depth) in pairs {
        if open == offset || close == offset {
            return Some((open, close));
        }
    }
    // Also check offset-1 (cursor might be just after the bracket)
    if offset > 0 {
        let before = offset - 1;
        for &(open, close, _depth) in pairs {
            if open == before || close == before {
                return Some((open, close));
            }
        }
    }
    None
}

// ── Tab state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct TabState {
    path: PathBuf,
    name: String,
    dirty: RwSignal<bool>,
}

// ── Editor panel ──────────────────────────────────────────────────────────────

/// Full multi-tab code editor with syntect syntax highlighting.
///
/// `ai_thinking` drives the sentient-gutter glow on the left edge.
/// `lsp_cmd` notifies the LSP server on every edit (did_change).
/// `active_cursor` is written with (path, 0-based line, 0-based col) whenever
///   the active editor's cursor moves — read by the completion popup.
#[allow(clippy::too_many_arguments)]
pub fn editor_panel(
    open_file: RwSignal<Option<PathBuf>>,
    theme: RwSignal<PhazeTheme>,
    ai_thinking: RwSignal<bool>,
    lsp_cmd: tokio::sync::mpsc::UnboundedSender<crate::lsp_bridge::LspCommand>,
    active_cursor: RwSignal<Option<(PathBuf, u32, u32)>>,
    pending_completion: RwSignal<Option<(String, usize)>>,
    diagnostics: RwSignal<Vec<crate::lsp_bridge::DiagEntry>>,
    ext_goto_line: RwSignal<u32>,
    comment_toggle_nonce: RwSignal<u64>,
    initial_tabs: Vec<PathBuf>,
    open_tabs_out: RwSignal<Vec<PathBuf>>,
    vim_motion: RwSignal<Option<crate::app::VimMotion>>,
    ghost_text: RwSignal<Option<String>>,
    auto_save: RwSignal<bool>,
    workspace_root: std::path::PathBuf,
    font_size: RwSignal<u32>,
    word_wrap: RwSignal<bool>,
    ctrl_d_nonce: RwSignal<u64>,
    fold_nonce: RwSignal<u64>,
    unfold_nonce: RwSignal<u64>,
    move_line_up_nonce: RwSignal<u64>,
    move_line_down_nonce: RwSignal<u64>,
    duplicate_line_nonce: RwSignal<u64>,
    delete_line_nonce: RwSignal<u64>,
    active_blame: RwSignal<String>,
    col_cursor_up_nonce: RwSignal<u64>,
    col_cursor_down_nonce: RwSignal<u64>,
    sticky_lines_out: RwSignal<Vec<String>>,
    transform_upper_nonce: RwSignal<u64>,
    transform_lower_nonce: RwSignal<u64>,
    join_line_nonce: RwSignal<u64>,
    sort_lines_nonce: RwSignal<u64>,
    vim_visual_mode: RwSignal<bool>,
    vim_marks: RwSignal<std::collections::HashMap<char, (std::path::PathBuf, usize)>>,
    vim_last_motion: RwSignal<Option<crate::app::VimMotion>>,
    expand_selection_nonce: RwSignal<u64>,
    shrink_selection_nonce: RwSignal<u64>,
    relative_line_numbers: RwSignal<bool>,
    yank_ring: RwSignal<Vec<String>>,
    tab_size: RwSignal<u32>,
    line_ending_out: RwSignal<&'static str>,
    lsp_folding_ranges: RwSignal<Vec<(u32, u32)>>,
    transform_title_nonce: RwSignal<u64>,
    format_selection_nonce: RwSignal<u64>,
    save_no_format_nonce: RwSignal<u64>,
    fold_all_nonce: RwSignal<u64>,
    unfold_all_nonce: RwSignal<u64>,
    code_lens_sig: RwSignal<Vec<crate::lsp_bridge::CodeLensEntry>>,
    code_lens_visible: RwSignal<bool>,
    organize_imports_on_save: RwSignal<bool>,
    inlay_hints: RwSignal<Vec<crate::lsp_bridge::InlayHintEntry>>,
    inlay_hints_toggle: RwSignal<bool>,
) -> impl IntoView {
    let tabs: RwSignal<Vec<TabState>> = create_rw_signal(vec![]);
    let active_idx: RwSignal<Option<usize>> = create_rw_signal(None);

    // ── Restore session tabs on first mount ──────────────────────────────────
    // Open all paths from the previous session as background tabs before the
    // file-open memo runs. Guard with a one-shot flag so this only fires once.
    {
        let init_tabs = initial_tabs;
        let batch_done = create_rw_signal(false);
        create_effect(move |_| {
            if batch_done.get_untracked() {
                return;
            }
            batch_done.set(true);
            for p in &init_tabs {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.to_string_lossy().to_string());
                let p = p.clone();
                tabs.update(|list| {
                    if !list.iter().any(|t| t.path == p) {
                        list.push(TabState {
                            path: p,
                            name,
                            dirty: create_rw_signal(false),
                        });
                    }
                });
            }
            tabs.update(|list| disambiguate_tab_names(list));
            let n = tabs.get_untracked().len();
            if n > 0 {
                active_idx.set(Some(n - 1));
            }
        });
    }

    // ── Write back open tab paths whenever the tab list changes ─────────────
    create_effect(move |_| {
        let paths: Vec<PathBuf> = tabs.get().into_iter().map(|t| t.path.clone()).collect();
        open_tabs_out.set(paths);
    });

    // ── Ghost text (FIM) channel ─────────────────────────────────────────────
    // Background threads write suggestions here; create_signal_from_channel
    // wires the receiver into Floem's reactive system.
    let (fim_tx, fim_rx) = std::sync::mpsc::sync_channel::<String>(4);
    let fim_signal = create_signal_from_channel(fim_rx);
    // Forward channel values to the shared ghost_text signal.
    create_effect(move |_| {
        if let Some(text) = fim_signal.get() {
            ghost_text.set(if text.is_empty() { None } else { Some(text) });
        }
    });
    // Generation counter: incremented on every cursor move to cancel stale requests.
    let fim_gen: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    // Vim yank register — shared across all tabs (yy copies here, p/P paste from here).
    let vim_register: RwSignal<String> = create_rw_signal(String::new());

    let docs: Rc<RefCell<HashMap<String, Rc<dyn Document>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let docs_for_stack = docs.clone();
    let docs_for_save = docs.clone();
    let docs_for_find = docs.clone();

    // ── Find in file (Ctrl+F) ────────────────────────────────────────────────
    let find_open: RwSignal<bool> = create_rw_signal(false);
    let find_query: RwSignal<String> = create_rw_signal(String::new());
    let find_case: RwSignal<bool> = create_rw_signal(false); // case-sensitive toggle
    let find_regex_mode: RwSignal<bool> = create_rw_signal(false); // regex toggle
                                                                   // Incremented to trigger a cursor jump to `find_jump_offset`.
    let find_jump_nonce: RwSignal<u64> = create_rw_signal(0u64);
    let find_jump_offset: RwSignal<usize> = create_rw_signal(0usize);
    let find_cur_match: RwSignal<usize> = create_rw_signal(0usize);

    // ── Replace (Ctrl+H) ─────────────────────────────────────────────────────
    let replace_query: RwSignal<String> = create_rw_signal(String::new());
    let replace_open: RwSignal<bool> = create_rw_signal(false);
    // Incremented to trigger "replace current match" in the active editor.
    let replace_nonce: RwSignal<u64> = create_rw_signal(0u64);
    // Incremented to trigger "replace all matches" in the active editor.
    let replace_all_nonce: RwSignal<u64> = create_rw_signal(0u64);
    let find_whole_word: RwSignal<bool> = create_rw_signal(false);

    // Compute match offsets reactively (for display + navigation)
    let find_match_offsets = create_memo({
        let docs_for_find = docs_for_find.clone();
        move |_| -> Vec<usize> {
            let q = find_query.get();
            if q.is_empty() {
                return vec![];
            }
            let case = find_case.get();
            let use_regex = find_regex_mode.get();
            let whole_word = find_whole_word.get();
            let Some(idx) = active_idx.get() else {
                return vec![];
            };
            let list = tabs.get();
            let Some(tab) = list.get(idx) else {
                return vec![];
            };
            let key = tab.path.to_string_lossy().to_string();
            let reg = docs_for_find.borrow();
            let Some(doc) = reg.get(&key) else {
                return vec![];
            };
            let text = doc.text().to_string();
            let mut offs = vec![];
            if use_regex {
                // Regex mode using the regex crate (if available) or simple literal fallback.
                // Build pattern with optional case-insensitive flag.
                let pattern = if case { q.clone() } else { format!("(?i){q}") };
                if let Ok(re) = regex::Regex::new(&pattern) {
                    for m in re.find_iter(&text) {
                        offs.push(m.start());
                    }
                }
            } else if case {
                // Case-sensitive literal search
                let mut start = 0usize;
                while let Some(pos) = text[start..].find(q.as_str()) {
                    offs.push(start + pos);
                    start += pos + q.len().max(1);
                }
            } else {
                // Case-insensitive literal search
                let q_lo = q.to_lowercase();
                let t_lo = text.to_lowercase();
                let mut start = 0usize;
                while let Some(pos) = t_lo[start..].find(&q_lo) {
                    offs.push(start + pos);
                    start += pos + q_lo.len().max(1);
                }
            }
            // Apply whole-word filter: each match position must have word boundaries.
            if whole_word && !use_regex {
                offs.retain(|&start| {
                    let end = start + q.len();
                    let before_ok = start == 0 || {
                        let c = text[..start].chars().last().unwrap_or(' ');
                        !c.is_alphanumeric() && c != '_'
                    };
                    let after_ok = end >= text.len() || {
                        let c = text[end..].chars().next().unwrap_or(' ');
                        !c.is_alphanumeric() && c != '_'
                    };
                    before_ok && after_ok
                });
            }
            offs
        }
    });

    // ── Go-to line (Ctrl+G) ──────────────────────────────────────────────────
    let goto_open: RwSignal<bool> = create_rw_signal(false);
    let goto_query: RwSignal<String> = create_rw_signal(String::new());
    // Nonce — when incremented, the active editor recreates at `goto_line`.
    let goto_nonce: RwSignal<u64> = create_rw_signal(0u64);
    let goto_line: RwSignal<usize> = create_rw_signal(1usize);

    // Wire external goto_line (from LSP go-to-definition) into the local mechanism.
    // When IdeState.goto_line becomes nonzero we jump to that line, then reset to 0.
    create_effect(move |_| {
        let line = ext_goto_line.get();
        if line > 0 {
            goto_line.set(line as usize);
            goto_nonce.update(|v| *v += 1);
            ext_goto_line.set(0);
        }
    });

    // React to file-open requests from the explorer
    let _ = create_memo(move |_| {
        let path = open_file.get();
        if let Some(p) = path {
            // Don't open tabs for files that no longer exist
            if !p.exists() {
                return;
            }
            let existing = tabs.get().iter().position(|t| t.path == p);
            if let Some(idx) = existing {
                active_idx.set(Some(idx));
            } else {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "untitled".to_string());
                let new_idx = std::cell::Cell::new(0usize);
                tabs.update(|list| {
                    list.push(TabState {
                        path: p.clone(),
                        name,
                        dirty: create_rw_signal(false),
                    });
                    disambiguate_tab_names(list);
                    new_idx.set(list.len() - 1);
                });
                active_idx.set(Some(new_idx.get()));
            }
        }
    });

    // Ctrl+S save handler
    let lsp_cmd_for_save = lsp_cmd.clone();
    let save_fn = Rc::new(move || {
        let Some(idx) = active_idx.get() else { return };
        let tab_list = tabs.get();
        let Some(tab) = tab_list.get(idx) else { return };
        let key = tab.path.to_string_lossy().to_string();
        let registry = docs_for_save.borrow();
        let Some(doc) = registry.get(&key) else {
            return;
        };
        let content = doc.text().to_string();
        if std::fs::write(&tab.path, content).is_ok() {
            tab.dirty.set(false);
            // Send textDocument/didSave so LSP servers that rely on it (e.g. rust-analyzer
            // doesn't need it, but gopls, pylsp, etc. do) get the save notification.
            let _ = lsp_cmd_for_save.send(crate::lsp_bridge::LspCommand::SaveFile {
                path: tab.path.clone(),
            });
            // Organize imports if enabled
            if organize_imports_on_save.get_untracked() {
                let _ = lsp_cmd_for_save.send(crate::lsp_bridge::LspCommand::OrganizeImports {
                    path: tab.path.clone(),
                });
            }
            // Run formatter in background — file is already saved to disk
            let path = tab.path.clone();
            std::thread::spawn(move || {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let formatter: Option<(&str, Vec<String>)> = match ext {
                    "rs" => Some(("rustfmt", vec![path.to_string_lossy().to_string()])),
                    "js" | "ts" | "jsx" | "tsx" | "json" => Some((
                        "prettier",
                        vec!["--write".to_string(), path.to_string_lossy().to_string()],
                    )),
                    "py" => Some(("black", vec![path.to_string_lossy().to_string()])),
                    _ => None,
                };
                if let Some((cmd, args)) = formatter {
                    let _ = std::process::Command::new(cmd).args(&args).status();
                }
            });
        }
    });
    let save_fn_bar = save_fn.clone();
    let save_fn_key = save_fn.clone();
    let save_fn_auto = save_fn.clone();

    // ── Auto-save debounce ─────────────────────────────────────────────────
    // A global cancel-token counter; each keystroke increments it and starts
    // a 1.5 s timer thread.  When the timer fires it checks the token still
    // matches → only the LAST edit per 1.5 s window triggers a save.
    let auto_save_gen: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let (auto_save_tx, auto_save_rx) = std::sync::mpsc::sync_channel::<()>(1);

    // React to the auto-save channel signal on the UI thread (safe to call Rc save_fn).
    let auto_save_sig = create_signal_from_channel(auto_save_rx);
    {
        create_effect(move |_| {
            if auto_save_sig.get().is_some() {
                save_fn_auto();
            }
        });
    }

    let tab_bar = tab_bar_view(tabs, active_idx, theme, save_fn_bar, diagnostics);

    // ── Breadcrumbs bar ────────────────────────────────────────────────────
    // Shows:  WorkspaceRoot  ›  sub/dir/path  ›  filename
    // Derived reactively from the active tab's path relative to workspace_root.
    let ws_root = workspace_root.clone();
    let breadcrumbs = {
        let crumb_theme = theme;
        container(
            floem::views::dyn_stack(
                move || {
                    // Build crumb segments from the active tab path
                    let Some(idx) = active_idx.get() else {
                        return vec![];
                    };
                    let tab_list = tabs.get();
                    let Some(tab) = tab_list.get(idx) else {
                        return vec![];
                    };
                    let path = &tab.path;

                    // Try to make path relative to workspace root
                    let rel = path.strip_prefix(&ws_root).unwrap_or(path);
                    let components: Vec<String> = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect();

                    // (text, is_last)
                    let mut result: Vec<(String, bool)> = Vec::new();
                    for (i, c) in components.iter().enumerate() {
                        result.push((c.clone(), i == components.len() - 1));
                    }
                    result
                },
                |(s, _)| s.clone(),
                move |(name, is_last)| {
                    let n2 = name.clone();
                    stack((
                        label(move || n2.clone()).style(move |s| {
                            let p = crumb_theme.get().palette;
                            s.font_size(11.0).color(if is_last {
                                p.text_primary
                            } else {
                                p.text_muted
                            })
                        }),
                        label(move || if is_last { "" } else { "  ›  " }).style(move |s| {
                            let p = crumb_theme.get().palette;
                            s.font_size(10.0)
                                .color(p.text_disabled)
                                .apply_if(is_last, |s| s.display(floem::style::Display::None))
                        }),
                    ))
                    .style(|s| s.items_center())
                },
            )
            .style(|s| s.flex_row().items_center()),
        )
        .style(move |s| {
            let t = crumb_theme.get();
            let p = &t.palette;
            s.height(22.0)
                .width_full()
                .padding_horiz(12.0)
                .background(p.bg_deep)
                .border_bottom(1.0)
                .border_color(p.border)
                .items_center()
        })
    };

    // ── Sentient gutter — thin accent strip, no overflowing circles ─────
    let sentient_gutter = canvas(move |cx, size| {
        let t = theme.get();
        let p = &t.palette;
        cx.fill(&floem::kurbo::Rect::ZERO.with_size(size), p.bg_base, 0.0);
        let w = size.width;
        let h = size.height;
        // Simple 2px accent line when AI is thinking
        if ai_thinking.get() {
            cx.fill(
                &floem::kurbo::Rect::new(0.0, 0.0, w, h),
                p.accent.with_alpha(0.35),
                0.0,
            );
        } else {
            // Subtle accent dot at top
            cx.fill(
                &Circle::new(Point::new(w * 0.5, 6.0), 2.0),
                p.accent.with_alpha(0.25),
                0.0,
            );
        }
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.width(4.0)
            .height_full()
            .min_width(4.0)
            .background(p.bg_base)
    });

    // ── Editor body ─────────────────────────────────────────────────────────
    // Key by path only — editors are NEVER recreated on font-size or goto-line
    // changes.  Font-size updates call editor.update_styling() reactively.
    // Goto-line uses the same nonce-effect pattern as find-cursor-jump.
    // This preserves the undo/redo stack across zoom and navigation.
    let editor_body = dyn_stack(
        move || tabs.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(_i, tab)| format!("{}", tab.path.to_string_lossy()),
        move |(i, tab)| {
            let is_active = move || active_idx.get() == Some(i);
            let key = tab.path.to_string_lossy().to_string();
            let dirty = tab.dirty;

            // Read font_size once for initial construction (not tracked).
            let initial_fs = font_size.get_untracked() as usize;

            // Preserve unsaved edits across tab switches by reading doc registry first.
            let content = {
                let reg = docs_for_stack.borrow();
                reg.get(&key)
                    .map(|d| d.text().to_string())
                    .unwrap_or_else(|| std::fs::read_to_string(&tab.path).unwrap_or_default())
            };

            // ── .editorconfig: read and apply for this tab ────────────────
            {
                let ec = read_editorconfig(&tab.path, &workspace_root);
                if active_idx.get_untracked() == Some(i) {
                    if let Some(size) = ec.indent_size {
                        tab_size.set(size);
                    }
                    if let Some(eol) = ec.end_of_line {
                        line_ending_out.set(eol);
                    }
                }
            }

            // ── Auto-detect indentation from first 2000 bytes ─────────────
            {
                let sample_len = content.len().min(2000);
                let sample_end = content.floor_char_boundary(sample_len);
                let mut tab_count = 0usize;
                let mut space2 = 0usize;
                let mut space4 = 0usize;
                let mut space8 = 0usize;
                for line in content[..sample_end].lines() {
                    if line.starts_with('\t') {
                        tab_count += 1;
                    } else if line.starts_with("        ") {
                        space8 += 1;
                    } else if line.starts_with("    ") {
                        space4 += 1;
                    } else if line.starts_with("  ") {
                        space2 += 1;
                    }
                }
                let detected = if tab_count > space2 + space4 + space8 {
                    4u32 // tabs → treat as 4
                } else if space2 > space4 && space2 > space8 {
                    2
                } else if space8 > space4 {
                    8
                } else if space4 > 0 {
                    4
                } else {
                    0 // no evidence — keep current
                };
                if detected > 0 {
                    tab_size.set(detected);
                }
            }

            let tab_ext = tab
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string();

            // `use_wrap` passed at call time so this closure doesn't capture `word_wrap`.
            let make_base_styling = |fs: usize, use_wrap: bool| -> Rc<dyn Styling> {
                let wrap = if use_wrap {
                    WrapMethod::EditorWidth
                } else {
                    WrapMethod::None
                };
                Rc::new(
                    SimpleStylingBuilder::default()
                        .wrap(wrap)
                        .font_size(fs)
                        .font_family(vec![
                            FamilyOwned::Name("JetBrains Mono".to_string()),
                            FamilyOwned::Name("Fira Code".to_string()),
                            FamilyOwned::Name("Cascadia Code".to_string()),
                            FamilyOwned::Monospace,
                        ])
                        .build(),
                )
            };

            // Build the raw editor first so we can grab the doc and cursor.
            let raw_editor = text_editor(content);
            let cursor_sig = raw_editor.editor().cursor; // RwSignal<Cursor>
            let editor_ref = raw_editor.editor().clone(); // Clone for reactive updates
            let doc = raw_editor.doc().clone();
            // Clone doc ref for the LSP update callback (same Rc — UI-thread only).
            let doc_for_lsp = doc.clone();
            let lsp_ver: RwSignal<i32> = create_rw_signal(0i32);
            let lsp_path = tab.path.clone();
            let lsp_tx = lsp_cmd.clone();

            // ── Goto-line cursor jump (reactive effect, no editor recreation) ─
            {
                let last_nonce = create_rw_signal(0u64);
                let doc_for_goto = doc.clone();
                create_effect(move |_| {
                    let nonce = goto_nonce.get();
                    if nonce == 0 || nonce == last_nonce.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_nonce.set(nonce);
                    let rope = doc_for_goto.rope_text();
                    let line_0 = goto_line.get().saturating_sub(1);
                    let max_line = rope.num_lines().saturating_sub(1);
                    let offset = rope.offset_of_line(line_0.min(max_line));
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(offset)),
                        None,
                        None,
                    ));
                });
            }

            // ── Find-in-file cursor jump effect ──────────────────────────
            // When `find_jump_nonce` increments and this tab is active, jump
            // cursor to `find_jump_offset` WITHOUT recreating the editor.
            {
                let last_nonce = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = find_jump_nonce.get();
                    if nonce == 0 || nonce == last_nonce.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_nonce.set(nonce);
                    let offset = find_jump_offset.get();
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(offset)),
                        None,
                        None,
                    ));
                });
            }

            // ── Cursor position tracking → active_cursor signal ──────────
            // ── Word/symbol highlight under cursor ───────────────────────
            // On every cursor move in the active tab, find the word under
            // the cursor and all whole-word occurrences in the document.
            // Stores byte-offset ranges in `word_hl`; the styling effect
            // below picks them up and draws soft highlight boxes.
            let word_hl: RwSignal<Vec<(usize, usize)>> = create_rw_signal(vec![]);
            // Tracks the 0-based line index of the cursor for current-line highlighting.
            let current_line_sig: RwSignal<usize> = create_rw_signal(0usize);

            // ── Code folding per-tab state ─────────────────────────────────
            // (foldable_ranges, folded_starts): detected ranges + which starts are collapsed.
            #[allow(clippy::type_complexity)]
            let fold_state: RwSignal<(Vec<(usize, usize)>, HashSet<usize>)> =
                create_rw_signal((Vec::new(), HashSet::new()));

            // Bracket pairs for colorization: (open_byte, close_byte, depth)
            let bracket_pairs_sig: RwSignal<Vec<(usize, usize, usize)>> = create_rw_signal(vec![]);
            // Matching bracket for the bracket under cursor: (open_byte, close_byte)
            let matching_bracket_sig: RwSignal<Option<(usize, usize)>> = create_rw_signal(None);

            // Fires whenever cursor moves in the active editor; converts the
            // byte offset to (line, col) and writes to the shared signal so
            // Ctrl+Space can pass the right position to the LSP.
            {
                let track_path = tab.path.clone();
                let track_doc = doc.clone();
                create_effect(move |_| {
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    let cursor = cursor_sig.get();
                    let offset = cursor.offset();
                    let rope = track_doc.rope_text();
                    let line = rope.line_of_offset(offset) as u32;
                    let col = (offset - rope.offset_of_line(line as usize)) as u32;
                    active_cursor.set(Some((track_path.clone(), line, col)));
                    // Keep current_line_sig in sync so the current-line highlight reacts.
                    current_line_sig.set(line as usize);
                });
            }
            {
                let doc_for_hl = doc.clone();
                create_effect(move |_| {
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    let offset = cursor_sig.get().offset();
                    let rope = doc_for_hl.rope_text();
                    let len = rope.len();
                    if len == 0 {
                        word_hl.set(vec![]);
                        return;
                    }
                    // Avoid searching huge files (> 2 MB) on every keystroke.
                    let ranges = if len < 2_000_000 {
                        let text = rope.slice_to_cow(0..len).to_string();
                        if let Some((_, _, word)) = word_at_offset(&text, offset) {
                            if word.len() >= 2 {
                                find_word_occurrences(&text, &word)
                            } else {
                                vec![]
                            }
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    };
                    word_hl.set(ranges);
                });
            }

            // ── Ctrl+D — select next occurrence (multi-cursor) ────────────
            // Each Ctrl+D press finds the next occurrence of the word/selection
            // under the cursor and adds it as an additional selected region.
            {
                let doc_for_ctd = doc.clone();
                let last_nonce_ctd = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = ctrl_d_nonce.get();
                    if nonce == 0 || nonce == last_nonce_ctd.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_nonce_ctd.set(nonce);

                    let rope = doc_for_ctd.rope_text();
                    let len = rope.len();
                    if len == 0 {
                        return;
                    }
                    let text = rope.slice_to_cow(0..len).to_string();
                    let cur = cursor_sig.get();
                    let cur_offset = cur.offset();

                    // Determine the word/selection to search for.
                    // If cursor has a selection (start != end in first region), use that text.
                    // Otherwise use the word under cursor.
                    let (search_start, search_end, search_word) = {
                        let first_region = if let CursorMode::Insert(s) = &cur.mode {
                            s.regions().first().copied()
                        } else {
                            None
                        };
                        if let Some(r) = first_region.filter(|r| r.start != r.end) {
                            let (s, e) = (r.start.min(r.end), r.start.max(r.end));
                            let w = text.get(s..e).unwrap_or("").to_string();
                            (s, e, w)
                        } else if let Some((ws, we, w)) = word_at_offset(&text, cur_offset) {
                            (ws, we, w)
                        } else {
                            return;
                        }
                    };
                    if search_word.is_empty() {
                        return;
                    }

                    // Find the NEXT occurrence after search_end (wrap around).
                    let next_start = {
                        let after = text[search_end..]
                            .find(&search_word)
                            .map(|p| search_end + p);
                        let before = if after.is_none() {
                            text[..search_start].find(&search_word)
                        } else {
                            None
                        };
                        after.or(before)
                    };
                    let Some(ns) = next_start else {
                        return;
                    };
                    let ne = ns + search_word.len();

                    // Build new selection: current region + next occurrence.
                    let mut new_sel = Selection::new();
                    new_sel.add_region(SelRegion::new(search_start, search_end, None));
                    new_sel.add_region(SelRegion::new(ns, ne, None));
                    cursor_sig.set(Cursor::new(CursorMode::Insert(new_sel), None, None));
                });
            }

            // ── Fold range detection (on load + after each save) ──────────
            // Text is extracted on the UI thread (Rc<Document> is not Send);
            // brace-matching runs in a background thread.
            {
                let doc_for_fold = doc.clone();
                // Use sync_channel + create_signal_from_channel to avoid Scope leak.
                let (fold_tx, fold_rx) =
                    std::sync::mpsc::sync_channel::<Vec<(usize, usize)>>(1);
                let fold_result_sig = create_signal_from_channel(fold_rx);
                // Receive effect: merges brace-based and LSP ranges when result arrives.
                create_effect(move |_| {
                    if let Some(brace_ranges) = fold_result_sig.get() {
                        let lsp_ranges: Vec<(usize, usize)> = lsp_folding_ranges
                            .get_untracked()
                            .into_iter()
                            .map(|(s, e)| (s as usize, e as usize))
                            .collect();
                        let mut merged = brace_ranges;
                        for lsp_r in &lsp_ranges {
                            if !merged.iter().any(|b| b.0 == lsp_r.0) {
                                merged.push(*lsp_r);
                            }
                        }
                        merged.sort_by_key(|r| r.0);
                        fold_state.update(|(r, _f)| *r = merged);
                    }
                });
                // Trigger effect: re-runs on every save, spawns background detection.
                create_effect(move |_| {
                    let _dirty = safe_get(dirty, false); // re-runs when file is saved
                    let rope = doc_for_fold.rope_text();
                    let len = rope.len();
                    let text = if len == 0 || len > 500_000 {
                        String::new()
                    } else {
                        rope.slice_to_cow(0..len).to_string()
                    };
                    let tx = fold_tx.clone();
                    if text.is_empty() {
                        let _ = tx.send(vec![]);
                        return;
                    }
                    std::thread::spawn(move || {
                        let _ = tx.send(detect_fold_ranges(&text));
                    });
                });
            }

            // ── Fold/unfold keyboard shortcut effect ──────────────────────
            // Ctrl+Shift+[ folds the block at cursor; Ctrl+Shift+] unfolds it.
            {
                let doc_for_fold_toggle = doc.clone();
                let last_fold_nonce: RwSignal<u64> = create_rw_signal(0u64);
                let last_unfold_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let fn_ = fold_nonce.get();
                    let ufn = unfold_nonce.get();
                    if active_idx.get() != Some(i) {
                        return;
                    }

                    // Fold: collapse the block whose opening `{` is on the cursor line.
                    if fn_ != 0 && fn_ != last_fold_nonce.get_untracked() {
                        last_fold_nonce.set(fn_);
                        let offset = cursor_sig.get_untracked().offset();
                        let rope = doc_for_fold_toggle.rope_text();
                        let cur_line = rope.line_of_offset(offset);
                        fold_state.update(|(ranges, folded)| {
                            for &(start, _end) in ranges.iter() {
                                if start == cur_line {
                                    folded.insert(start);
                                    break;
                                }
                            }
                        });
                    }

                    // Unfold: expand the collapsed block containing the cursor line.
                    if ufn != 0 && ufn != last_unfold_nonce.get_untracked() {
                        last_unfold_nonce.set(ufn);
                        let offset = cursor_sig.get_untracked().offset();
                        let rope = doc_for_fold_toggle.rope_text();
                        let cur_line = rope.line_of_offset(offset);
                        fold_state.update(|(ranges, folded)| {
                            for &(start, end) in ranges.iter() {
                                if start == cur_line || (cur_line > start && cur_line <= end) {
                                    folded.remove(&start);
                                    break;
                                }
                            }
                        });
                    }
                });
            }

            // ── Bracket pair detection ────────────────────────────────────
            // Runs compute_bracket_pairs on file content changes in a background
            // thread. Updates bracket_pairs_sig, which drives both colorization
            // and matching-bracket highlight via the styling effect.
            {
                let doc_for_bp = doc.clone();
                // Use sync_channel + create_signal_from_channel to avoid Scope leak.
                let (bp_tx, bp_rx) =
                    std::sync::mpsc::sync_channel::<Vec<(usize, usize, usize)>>(1);
                let bp_result_sig = create_signal_from_channel(bp_rx);
                // Receive effect: applies bracket pairs when result arrives.
                create_effect(move |_| {
                    if let Some(pairs) = bp_result_sig.get() {
                        bracket_pairs_sig.set(pairs);
                    }
                });
                // Trigger effect: re-runs on dirty, spawns background detection.
                create_effect(move |_| {
                    let _dirty = safe_get(dirty, false); // re-runs on every save
                    let rope = doc_for_bp.rope_text();
                    let len = rope.len();
                    let text = if len == 0 || len > 300_000 {
                        String::new()
                    } else {
                        rope.slice_to_cow(0..len).to_string()
                    };
                    let tx = bp_tx.clone();
                    if text.is_empty() {
                        let _ = tx.send(vec![]);
                        return;
                    }
                    std::thread::spawn(move || {
                        let _ = tx.send(compute_bracket_pairs(&text));
                    });
                });
            }

            // ── Matching bracket detection ────────────────────────────────
            // On every cursor move for the active tab, checks if the cursor
            // is adjacent to a bracket and highlights both it and its pair.
            {
                create_effect(move |_| {
                    let offset = cursor_sig.get().offset();
                    let pairs = bracket_pairs_sig.get();
                    if active_idx.get() != Some(i) {
                        matching_bracket_sig.set(None);
                        return;
                    }
                    let result = find_bracket_match("", offset, &pairs);
                    matching_bracket_sig.set(result);
                });
            }

            // ── Auto-close bracket insertion ─────────────────────────────
            // When the user types an opening bracket, automatically insert the
            // matching closing bracket and keep the cursor between the pair.
            // Uses a suppress flag so our own insert doesn't re-trigger.
            {
                let doc_for_ac = doc.clone();
                // Suppress re-entry after our own edit.
                let ac_suppress: RwSignal<bool> = create_rw_signal(false);
                // Previous cursor offset for delta detection.
                let ac_prev: RwSignal<usize> = create_rw_signal(0usize);
                create_effect(move |_| {
                    let cur_pos = cursor_sig.get().offset();
                    // Skip if this was triggered by our own bracket insert.
                    if ac_suppress.get_untracked() {
                        ac_suppress.set(false);
                        return;
                    }
                    let prev = ac_prev.get_untracked();
                    ac_prev.set(cur_pos);
                    // Only act when cursor advanced by exactly 1 (character typed).
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    if cur_pos == 0 || cur_pos != prev + 1 {
                        return;
                    }
                    let rope = doc_for_ac.rope_text();
                    if cur_pos > rope.len() {
                        return;
                    }
                    let prefix = rope.slice_to_cow(0..cur_pos);
                    if let Some(open_ch) = prefix.chars().last() {
                        let close = match open_ch {
                            '(' => Some(')'),
                            '[' => Some(']'),
                            '{' => Some('}'),
                            // Auto-close quotes only when the next char is whitespace/EOF
                            // and the char before the quote is not a backslash (escape).
                            '"' => {
                                let prev_is_escape = cur_pos >= 2 && {
                                    let s = rope.slice_to_cow((cur_pos - 2)..(cur_pos - 1));
                                    s.starts_with('\\')
                                };
                                if prev_is_escape {
                                    None
                                } else {
                                    Some('"')
                                }
                            }
                            '\'' => {
                                // Don't close in Rust lifetime positions: `'a` at word boundary
                                let prev_is_alpha = cur_pos >= 2 && {
                                    let s = rope.slice_to_cow((cur_pos - 2)..(cur_pos - 1));
                                    s.chars()
                                        .next()
                                        .map(|c| c.is_alphanumeric() || c == '_')
                                        .unwrap_or(false)
                                };
                                if prev_is_alpha {
                                    None
                                } else {
                                    Some('\'')
                                }
                            }
                            _ => None,
                        };
                        if let Some(close_ch) = close {
                            // Only auto-close when next char is whitespace or end-of-file.
                            let next_is_ok = cur_pos >= rope.len() || {
                                let nc = rope
                                    .slice_to_cow(cur_pos..(cur_pos + 1).min(rope.len()))
                                    .chars()
                                    .next()
                                    .unwrap_or('\0');
                                nc.is_whitespace()
                            };
                            if next_is_ok {
                                ac_suppress.set(true);
                                doc_for_ac.edit_single(
                                    Selection::caret(cur_pos),
                                    &close_ch.to_string(),
                                    EditType::InsertChars,
                                );
                                // Keep cursor between the pair (don't advance past closing bracket).
                                cursor_sig.set(Cursor::new(
                                    CursorMode::Insert(Selection::caret(cur_pos)),
                                    None,
                                    None,
                                ));
                            }
                        }
                    }
                });
            }

            // ── Auto-surround (wrap selection with bracket / quote) ───────
            // When the user has a non-empty selection and types an opening
            // bracket or quote, the selection is WRAPPED rather than replaced.
            // Works for ( [ { " and ' — skips if selection is > 50 KB.
            {
                let doc_for_surr = doc.clone();
                let surr_suppress: RwSignal<bool> = create_rw_signal(false);
                // Previous selection: (byte_start, byte_end, text).
                // We save this while the selection is active so we can use it
                // after the bracket keystroke replaces the selection.
                let surr_prev_sel: RwSignal<Option<(usize, usize, String)>> =
                    create_rw_signal(None);

                create_effect(move |_| {
                    let cur = cursor_sig.get();
                    let cur_pos = cur.offset();

                    if surr_suppress.get_untracked() {
                        surr_suppress.set(false);
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        surr_prev_sel.set(None);
                        return;
                    }

                    // ── Detect surround: previous had selection, now caret at sel_start+1 ──
                    // When selection [s,e] is replaced by a bracket, cursor lands at s+1.
                    // The regular auto-close delta check (cur_pos != prev+1) guards against
                    // double-insertion because prev_offset = e and e != s+1 for len>0 selns.
                    if let Some((sel_start, _sel_end, ref sel_text)) = surr_prev_sel.get_untracked()
                    {
                        if !sel_text.is_empty() && cur_pos == sel_start + 1 {
                            let rope = doc_for_surr.rope_text();
                            if cur_pos <= rope.len() {
                                let typed_ch = rope.slice_to_cow(sel_start..cur_pos).chars().next();
                                let close_opt = typed_ch.and_then(|c| match c {
                                    '(' => Some(')'),
                                    '[' => Some(']'),
                                    '{' => Some('}'),
                                    '"' => Some('"'),
                                    '\'' => Some('\''),
                                    _ => None,
                                });
                                if let Some(close_ch) = close_opt {
                                    surr_suppress.set(true);
                                    let insert_text = format!("{sel_text}{close_ch}");
                                    let ins_len = insert_text.len();
                                    doc_for_surr.edit_single(
                                        Selection::caret(cur_pos),
                                        &insert_text,
                                        EditType::InsertChars,
                                    );
                                    cursor_sig.set(Cursor::new(
                                        CursorMode::Insert(Selection::caret(cur_pos + ins_len)),
                                        None,
                                        None,
                                    ));
                                    surr_prev_sel.set(None);
                                    return;
                                }
                            }
                        }
                    }

                    // ── Update previous selection tracking for the next run ──
                    let sel_info = if let CursorMode::Insert(sel) = &cur.mode {
                        sel.regions().first().copied().and_then(|r| {
                            if r.start == r.end {
                                return None;
                            }
                            let s_b = r.start.min(r.end);
                            let e_b = r.start.max(r.end);
                            // Skip very large selections to avoid allocation pressure.
                            if e_b - s_b >= 50_000 {
                                return None;
                            }
                            let rope = doc_for_surr.rope_text();
                            let text = rope.slice_to_cow(s_b..e_b.min(rope.len())).to_string();
                            Some((s_b, e_b, text))
                        })
                    } else {
                        None
                    };
                    surr_prev_sel.set(sel_info);
                });
            }

            // ── Smart indent on Enter ────────────────────────────────────
            // After pressing Enter, indents the new line to match the previous
            // line's indentation (plus extra indent after `{`, `(`, `[`, `:`).
            {
                let doc_for_si = doc.clone();
                let si_suppress: RwSignal<bool> = create_rw_signal(false);
                let si_prev: RwSignal<usize> = create_rw_signal(0usize);
                create_effect(move |_| {
                    let cur_pos = cursor_sig.get().offset();
                    if si_suppress.get_untracked() {
                        si_suppress.set(false);
                        return;
                    }
                    let prev = si_prev.get_untracked();
                    si_prev.set(cur_pos);
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    if cur_pos == 0 {
                        return;
                    }
                    let rope = doc_for_si.rope_text();
                    let len = rope.len();
                    if cur_pos > len {
                        return;
                    }
                    let cur_line = rope.line_of_offset(cur_pos.min(len.saturating_sub(1)));
                    let cur_col = cur_pos.saturating_sub(rope.offset_of_line(cur_line));
                    // Only trigger when cursor is at the very start of a new line.
                    if cur_col != 0 || cur_line == 0 {
                        return;
                    }
                    let prev_clamped = prev.min(len.saturating_sub(1));
                    let prev_line = rope.line_of_offset(prev_clamped);
                    // Check that we moved exactly one line down (Enter was pressed).
                    if cur_line != prev_line + 1 {
                        return;
                    }
                    // Verify the char before cursor is a newline (not a cursor jump).
                    let before_cur = rope
                        .slice_to_cow(cur_pos.saturating_sub(1)..cur_pos)
                        .chars()
                        .next();
                    if before_cur != Some('\n') {
                        return;
                    }
                    // Derive indentation from the previous line.
                    let pl_start = rope.offset_of_line(prev_line);
                    let pl_end = rope.offset_of_line(cur_line);
                    let pl_text = rope.slice_to_cow(pl_start..pl_end).to_string();
                    let pl_trim = pl_text.trim_end_matches(['\n', '\r']);
                    let ws_len = pl_trim.len() - pl_trim.trim_start().len();
                    let indent = pl_trim[..ws_len].to_string();
                    let extra = if pl_trim.trim_end().ends_with('{')
                        || pl_trim.trim_end().ends_with('(')
                        || pl_trim.trim_end().ends_with('[')
                        || pl_trim.trim_end().ends_with(':')
                    {
                        "    " // 4-space extra indent after block-opening tokens
                    } else {
                        ""
                    };
                    let full_indent = format!("{indent}{extra}");
                    if full_indent.is_empty() {
                        return;
                    }
                    si_suppress.set(true);
                    doc_for_si.edit_single(
                        Selection::caret(cur_pos),
                        &full_indent,
                        EditType::InsertChars,
                    );
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(cur_pos + full_indent.len())),
                        None,
                        None,
                    ));
                });
            }

            // ── De-indent on `}` ──────────────────────────────────────────
            // When the user types `}` as the first non-whitespace character on
            // a line that has leading whitespace, remove one indent level so the
            // closing brace aligns with the matching opening block.
            {
                let doc_for_di = doc.clone();
                let di_suppress: RwSignal<bool> = create_rw_signal(false);
                let di_prev: RwSignal<usize> = create_rw_signal(0usize);
                create_effect(move |_| {
                    let cur_pos = cursor_sig.get().offset();
                    if di_suppress.get_untracked() {
                        di_suppress.set(false);
                        return;
                    }
                    let prev = di_prev.get_untracked();
                    di_prev.set(cur_pos);
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    // Only trigger on a single char advance (typing, not pasting).
                    if cur_pos == 0 || cur_pos != prev + 1 {
                        return;
                    }
                    let rope = doc_for_di.rope_text();
                    if cur_pos > rope.len() {
                        return;
                    }
                    // The last typed character must be `}`.
                    let prefix = rope.slice_to_cow(cur_pos.saturating_sub(1)..cur_pos);
                    if !prefix.starts_with('}') {
                        return;
                    }
                    // Find the line containing the `}`.
                    let cur_line = rope.line_of_offset(cur_pos.saturating_sub(1));
                    let line_start = rope.offset_of_line(cur_line);
                    let line_end = rope.offset_of_line(cur_line + 1).min(rope.len());
                    let line_text = rope.slice_to_cow(line_start..line_end).to_string();
                    // The `}` must be the first non-whitespace character.
                    let trimmed = line_text.trim_start_matches([' ', '\t']);
                    if !trimmed.starts_with('}') {
                        return;
                    }
                    let ws_len = line_text.len() - trimmed.len();
                    if ws_len == 0 {
                        return;
                    } // already at column 0 — nothing to do
                      // Remove one indent level: prefer 4 spaces, then 2, then 1 tab.
                    let ws_prefix = &line_text[..ws_len];
                    let remove = if ws_prefix.ends_with("    ") {
                        4
                    } else if ws_prefix.ends_with("  ") {
                        2
                    } else if ws_prefix.ends_with('\t') {
                        1
                    } else {
                        return;
                    };
                    di_suppress.set(true);
                    doc_for_di.edit_single(
                        Selection::region(line_start, line_start + remove),
                        "",
                        EditType::Delete,
                    );
                    // Adjust cursor to compensate for the removed whitespace.
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(cur_pos - remove)),
                        None,
                        None,
                    ));
                });
            }

            // ── Completion insertion effect ───────────────────────────────
            // When `pending_completion` is set and this tab is active, delete the
            // already-typed prefix (prefix_byte_len) and insert the completion text.
            {
                let doc_for_comp = doc.clone();
                create_effect(move |_| {
                    let Some((text, prefix_byte_len)) = pending_completion.get() else {
                        return;
                    };
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    // Consume immediately to prevent re-run.
                    pending_completion.set(None);
                    let cursor_offset = cursor_sig.get().offset();
                    // Selection covers prefix already typed by the user (to replace it).
                    let start = cursor_offset.saturating_sub(prefix_byte_len);
                    let sel = Selection::region(start, cursor_offset);
                    doc_for_comp.edit_single(sel, &text, EditType::InsertChars);
                });
            }

            // ── Replace current match ─────────────────────────────────────
            {
                let doc_for_repl = doc.clone();
                let last_repl_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = replace_nonce.get();
                    if nonce == 0 || nonce == last_repl_nonce.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_repl_nonce.set(nonce);
                    let offsets = find_match_offsets.get();
                    let cur = find_cur_match.get();
                    let Some(&start) = offsets.get(cur) else {
                        return;
                    };
                    let q = find_query.get();
                    let end = start + q.len();
                    let sel = Selection::region(start, end);
                    let replacement = replace_query
                        .get()
                        .replace("\\n", "\n")
                        .replace("\\t", "\t");
                    doc_for_repl.edit_single(sel, &replacement, EditType::InsertChars);
                });
            }

            // ── Replace all matches ───────────────────────────────────────
            {
                let doc_for_repl_all = doc.clone();
                let last_repl_all_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = replace_all_nonce.get();
                    if nonce == 0 || nonce == last_repl_all_nonce.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_repl_all_nonce.set(nonce);
                    let offsets = find_match_offsets.get();
                    if offsets.is_empty() {
                        return;
                    }
                    let q = find_query.get();
                    let replacement = replace_query
                        .get()
                        .replace("\\n", "\n")
                        .replace("\\t", "\t");
                    // Replace from last to first to preserve earlier offsets.
                    for &start in offsets.iter().rev() {
                        let end = start + q.len();
                        let sel = Selection::region(start, end);
                        doc_for_repl_all.edit_single(sel, &replacement, EditType::InsertChars);
                    }
                });
            }

            // ── Comment toggle (Ctrl+/) ──────────────────────────────────
            // When `comment_toggle_nonce` increments and this tab is active,
            // insert or remove the line-comment prefix for the file's language.
            {
                let doc_for_comment = doc.clone();
                let last_nonce = create_rw_signal(0u64);
                let ext_for_comment = tab_ext.clone();
                create_effect(move |_| {
                    let nonce = comment_toggle_nonce.get();
                    if nonce == 0 || nonce == last_nonce.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_nonce.set(nonce);

                    let prefix = match ext_for_comment.as_str() {
                        "rs" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "c" | "cpp" | "cc" | "h"
                        | "hpp" | "cs" | "java" | "go" | "swift" | "kt" | "scala" | "dart"
                        | "groovy" => "// ",
                        "py" | "pyw" | "rb" | "sh" | "bash" | "zsh" | "yaml" | "yml" | "toml"
                        | "r" | "jl" | "tf" | "dockerfile" | "makefile" => "# ",
                        "lua" => "-- ",
                        "hs" | "elm" => "-- ",
                        "sql" => "-- ",
                        _ => "// ",
                    };

                    let rope = doc_for_comment.rope_text();
                    let offset = cursor_sig.get().offset();
                    let line = rope.line_of_offset(offset);
                    let line_start = rope.offset_of_line(line);
                    let line_end = if line + 1 < rope.num_lines() {
                        rope.offset_of_line(line + 1).saturating_sub(1)
                    } else {
                        rope.len()
                    };
                    let line_text = rope.slice_to_cow(line_start..line_end).to_string();
                    let stripped = line_text.trim_start();
                    let leading_ws = line_text.len() - stripped.len();

                    if stripped.starts_with(prefix) {
                        // Remove the comment prefix
                        let remove_start = line_start + leading_ws;
                        let remove_end = remove_start + prefix.len();
                        doc_for_comment.edit_single(
                            Selection::region(remove_start, remove_end),
                            "",
                            EditType::Delete,
                        );
                    } else {
                        // Insert the comment prefix after leading whitespace
                        let insert_at = line_start + leading_ws;
                        doc_for_comment.edit_single(
                            Selection::caret(insert_at),
                            prefix,
                            EditType::InsertChars,
                        );
                    }
                });
            }

            // ── Move line up (Alt+Up) ─────────────────────────────────────
            {
                let doc_for_mlu = doc.clone();
                let last_mlu_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = move_line_up_nonce.get();
                    if nonce == 0 || nonce == last_mlu_nonce.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_mlu_nonce.set(nonce);

                    let rope = doc_for_mlu.rope_text();
                    let offset = cursor_sig.get().offset();
                    let cur_line = rope.line_of_offset(offset);
                    if cur_line == 0 {
                        return;
                    }

                    let cur_start = rope.offset_of_line(cur_line);
                    let cur_end = if cur_line + 1 < rope.num_lines() {
                        rope.offset_of_line(cur_line + 1)
                    } else {
                        rope.len()
                    };
                    let prev_start = rope.offset_of_line(cur_line - 1);
                    let cur_text = rope.slice_to_cow(cur_start..cur_end).to_string();
                    let prev_text = rope.slice_to_cow(prev_start..cur_start).to_string();

                    let cur_col = offset - cur_start;
                    // Delete both lines and rewrite swapped
                    doc_for_mlu.edit_single(
                        Selection::region(prev_start, cur_end),
                        &format!("{cur_text}{prev_text}"),
                        EditType::InsertChars,
                    );
                    let new_offset =
                        prev_start + cur_col.min(cur_text.trim_end_matches('\n').len());
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(new_offset)),
                        None,
                        None,
                    ));
                    dirty.set(true);
                });
            }

            // ── Move line down (Alt+Down) ────────────────────────────────
            {
                let doc_for_mld = doc.clone();
                let last_mld_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = move_line_down_nonce.get();
                    if nonce == 0 || nonce == last_mld_nonce.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_mld_nonce.set(nonce);

                    let rope = doc_for_mld.rope_text();
                    let offset = cursor_sig.get().offset();
                    let cur_line = rope.line_of_offset(offset);
                    if cur_line + 1 >= rope.num_lines() {
                        return;
                    }

                    let cur_start = rope.offset_of_line(cur_line);
                    let next_start = rope.offset_of_line(cur_line + 1);
                    let next_end = if cur_line + 2 < rope.num_lines() {
                        rope.offset_of_line(cur_line + 2)
                    } else {
                        rope.len()
                    };
                    let cur_text = rope.slice_to_cow(cur_start..next_start).to_string();
                    let next_text = rope.slice_to_cow(next_start..next_end).to_string();

                    let cur_col = offset - cur_start;
                    doc_for_mld.edit_single(
                        Selection::region(cur_start, next_end),
                        &format!("{next_text}{cur_text}"),
                        EditType::InsertChars,
                    );
                    let new_line_start = cur_start + next_text.len();
                    let new_offset =
                        new_line_start + cur_col.min(cur_text.trim_end_matches('\n').len());
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(new_offset)),
                        None,
                        None,
                    ));
                    dirty.set(true);
                });
            }

            // ── Duplicate line (Alt+Shift+Down) ──────────────────────────
            {
                let doc_for_dup = doc.clone();
                let last_dup_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = duplicate_line_nonce.get();
                    if nonce == 0 || nonce == last_dup_nonce.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_dup_nonce.set(nonce);

                    let rope = doc_for_dup.rope_text();
                    let offset = cursor_sig.get().offset();
                    let cur_line = rope.line_of_offset(offset);
                    let cur_start = rope.offset_of_line(cur_line);
                    let cur_end = if cur_line + 1 < rope.num_lines() {
                        rope.offset_of_line(cur_line + 1)
                    } else {
                        rope.len()
                    };
                    let line_text = rope.slice_to_cow(cur_start..cur_end).to_string();
                    let cur_col = offset - cur_start;

                    // If last line has no trailing newline, prepend one
                    let insert_text = if line_text.ends_with('\n') {
                        line_text.clone()
                    } else {
                        format!("\n{line_text}")
                    };
                    doc_for_dup.edit_single(
                        Selection::caret(cur_end),
                        &insert_text,
                        EditType::InsertChars,
                    );
                    // Place cursor on the duplicated (new) line at the same column
                    let new_line_start = cur_end + if line_text.ends_with('\n') { 0 } else { 1 };
                    let new_offset =
                        new_line_start + cur_col.min(line_text.trim_end_matches('\n').len());
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(new_offset)),
                        None,
                        None,
                    ));
                    dirty.set(true);
                });
            }
            // ── Delete line (Ctrl+Shift+K) ──────────────────────────────
            {
                let doc_for_del = doc.clone();
                let last_del_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = delete_line_nonce.get();
                    if nonce == 0 || nonce == last_del_nonce.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_del_nonce.set(nonce);

                    let rope = doc_for_del.rope_text();
                    let offset = cursor_sig.get().offset();
                    let cur_line = rope.line_of_offset(offset);
                    let cur_start = rope.offset_of_line(cur_line);
                    let cur_end = if cur_line + 1 < rope.num_lines() {
                        rope.offset_of_line(cur_line + 1)
                    } else {
                        rope.len()
                    };
                    // If deleting the very last line, also eat preceding newline
                    let del_start = if cur_line + 1 >= rope.num_lines() && cur_start > 0 {
                        cur_start - 1
                    } else {
                        cur_start
                    };
                    doc_for_del.edit_single(
                        Selection::region(del_start, cur_end),
                        "",
                        EditType::InsertChars,
                    );
                    let new_len = doc_for_del.rope_text().len();
                    let new_offset = del_start.min(new_len);
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(new_offset)),
                        None,
                        None,
                    ));
                    dirty.set(true);
                });
            }

            // ── Column cursor up (Ctrl+Alt+Up) ───────────────────────────
            // Adds a new cursor on the line above at the same byte column.
            {
                let doc_col = doc.clone();
                let last_ccu: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = col_cursor_up_nonce.get();
                    if nonce == 0 || nonce == last_ccu.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_ccu.set(nonce);
                    let rope = doc_col.rope_text();
                    let cur = cursor_sig.get();
                    let offset = cur.offset();
                    let cur_line = rope.line_of_offset(offset);
                    if cur_line == 0 {
                        return;
                    }
                    let cur_col = offset - rope.offset_of_line(cur_line);
                    let prev_line_start = rope.offset_of_line(cur_line - 1);
                    let prev_line_end = rope.offset_of_line(cur_line).saturating_sub(1);
                    let prev_line_len = prev_line_end.saturating_sub(prev_line_start);
                    let new_off = prev_line_start + cur_col.min(prev_line_len);
                    // Build multi-region selection: existing region + new one
                    // Build multi-region selection from existing cursor
                    let existing_sel = match &cur.mode {
                        CursorMode::Insert(s) => s.clone(),
                        _ => Selection::caret(offset),
                    };
                    let mut regions: Vec<SelRegion> = existing_sel.regions().to_vec();
                    regions.push(SelRegion::new(new_off, new_off, None));
                    let mut sel = Selection::new();
                    for r in regions {
                        sel.add_region(r);
                    }
                    cursor_sig.set(Cursor::new(CursorMode::Insert(sel), None, None));
                });
            }

            // ── Column cursor down (Ctrl+Alt+Down) ──────────────────────
            // Adds a new cursor on the line below at the same byte column.
            {
                let doc_col = doc.clone();
                let last_ccd: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = col_cursor_down_nonce.get();
                    if nonce == 0 || nonce == last_ccd.get() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_ccd.set(nonce);
                    let rope = doc_col.rope_text();
                    let cur = cursor_sig.get();
                    let offset = cur.offset();
                    let cur_line = rope.line_of_offset(offset);
                    if cur_line + 1 >= rope.num_lines() {
                        return;
                    }
                    let cur_col = offset - rope.offset_of_line(cur_line);
                    let next_line_start = rope.offset_of_line(cur_line + 1);
                    let next_line_end = if cur_line + 2 < rope.num_lines() {
                        rope.offset_of_line(cur_line + 2).saturating_sub(1)
                    } else {
                        rope.len()
                    };
                    let next_line_len = next_line_end.saturating_sub(next_line_start);
                    let new_off = next_line_start + cur_col.min(next_line_len);
                    let existing_sel = match &cur.mode {
                        CursorMode::Insert(s) => s.clone(),
                        _ => Selection::caret(offset),
                    };
                    let mut regions: Vec<SelRegion> = existing_sel.regions().to_vec();
                    regions.push(SelRegion::new(new_off, new_off, None));
                    let mut sel = Selection::new();
                    for r in regions {
                        sel.add_region(r);
                    }
                    cursor_sig.set(Cursor::new(CursorMode::Insert(sel), None, None));
                });
            }

            // ── Sticky scroll ────────────────────────────────────────────
            // Scans backward from cursor line to find enclosing scope
            // headers (fn, struct, impl, class, def, etc.) and publishes
            // them to `sticky_lines_out` so the view can pin them at top.
            {
                let doc_sticky = doc.clone();
                create_effect(move |_| {
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    let _ = cursor_sig.get(); // track cursor changes
                    let rope = doc_sticky.rope_text();
                    let offset = cursor_sig.get_untracked().offset();
                    let cur_line = rope.line_of_offset(offset);
                    let text = rope.slice_to_cow(0..rope.len()).to_string();
                    let lines: Vec<&str> = text.lines().collect();

                    // Patterns that start a scope block
                    fn is_scope_header(line: &str) -> bool {
                        let trimmed = line.trim();
                        trimmed.starts_with("fn ")
                            || trimmed.starts_with("pub fn ")
                            || trimmed.starts_with("async fn ")
                            || trimmed.starts_with("pub async fn ")
                            || trimmed.starts_with("struct ")
                            || trimmed.starts_with("pub struct ")
                            || trimmed.starts_with("enum ")
                            || trimmed.starts_with("pub enum ")
                            || trimmed.starts_with("impl ")
                            || trimmed.starts_with("trait ")
                            || trimmed.starts_with("pub trait ")
                            || trimmed.starts_with("mod ")
                            || trimmed.starts_with("pub mod ")
                            || trimmed.starts_with("class ")
                            || trimmed.starts_with("def ")
                            || trimmed.starts_with("interface ")
                            || trimmed.starts_with("function ")
                    }

                    fn indent_of(line: &str) -> usize {
                        line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
                    }

                    let cur_indent = lines.get(cur_line).map(|l| indent_of(l)).unwrap_or(0);

                    let mut headers: Vec<String> = Vec::new();
                    let mut last_indent = cur_indent;
                    for line_idx in (0..cur_line).rev() {
                        let line = match lines.get(line_idx) {
                            Some(l) => l,
                            None => continue,
                        };
                        let ind = indent_of(line);
                        if is_scope_header(line) && ind < last_indent {
                            headers.push(line.trim().to_string());
                            last_indent = ind;
                            if ind == 0 {
                                break;
                            }
                        }
                    }
                    headers.reverse();
                    sticky_lines_out.set(headers);
                });
            }

            // ── Vim motion effect ─────────────────────────────────────────
            // When `vim_motion` is set and this tab is active, execute the
            // corresponding cursor movement or edit, then clear the signal.
            // Visual anchor: the offset where a visual selection started.
            let vim_visual_anchor: RwSignal<Option<usize>> = create_rw_signal(None);
            {
                use crate::app::VimMotion;
                let doc_for_vim = doc.clone();
                create_effect(move |_| {
                    let Some(motion) = vim_motion.get() else {
                        return;
                    };
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    // Consume immediately so re-run returns early.
                    vim_motion.set(None);

                    let cur_offset = cursor_sig.get().offset();

                    let new_offset: usize = match motion {
                        VimMotion::Left => cur_offset.saturating_sub(1),
                        VimMotion::Right => {
                            let len = doc_for_vim.rope_text().len();
                            (cur_offset + 1).min(len)
                        }
                        VimMotion::Up => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            if line == 0 {
                                cur_offset
                            } else {
                                let col = cur_offset - rope.offset_of_line(line);
                                let prev_start = rope.offset_of_line(line - 1);
                                let prev_end = rope.offset_of_line(line).saturating_sub(1);
                                prev_start + col.min(prev_end.saturating_sub(prev_start))
                            }
                        }
                        VimMotion::Down => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let next = line + 1;
                            if next >= rope.num_lines() {
                                cur_offset
                            } else {
                                let col = cur_offset - rope.offset_of_line(line);
                                let next_start = rope.offset_of_line(next);
                                let next_end = if next + 1 < rope.num_lines() {
                                    rope.offset_of_line(next + 1).saturating_sub(1)
                                } else {
                                    rope.len()
                                };
                                next_start + col.min(next_end.saturating_sub(next_start))
                            }
                        }
                        VimMotion::WordForward => {
                            let rope = doc_for_vim.rope_text();
                            let len = rope.len();
                            let text = rope.slice_to_cow(cur_offset..len).to_string();
                            let mut skip_bytes = 0usize;
                            let mut chars = text.chars().peekable();
                            // Skip current-word chars
                            while let Some(&c) = chars.peek() {
                                if c.is_alphanumeric() || c == '_' {
                                    skip_bytes += c.len_utf8();
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                            // Skip whitespace
                            while let Some(&c) = chars.peek() {
                                if c.is_whitespace() {
                                    skip_bytes += c.len_utf8();
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                            (cur_offset + skip_bytes).min(len)
                        }
                        VimMotion::WordBackward => {
                            if cur_offset == 0 {
                                0
                            } else {
                                let rope = doc_for_vim.rope_text();
                                let text = rope.slice_to_cow(0..cur_offset).to_string();
                                let mut pos = text.len();
                                // Skip whitespace
                                while pos > 0 {
                                    let c = text[..pos].chars().last().unwrap_or(' ');
                                    if c.is_whitespace() {
                                        pos -= c.len_utf8();
                                    } else {
                                        break;
                                    }
                                }
                                // Skip word chars
                                while pos > 0 {
                                    let c = text[..pos].chars().last().unwrap_or(' ');
                                    if c.is_alphanumeric() || c == '_' {
                                        pos -= c.len_utf8();
                                    } else {
                                        break;
                                    }
                                }
                                pos
                            }
                        }
                        VimMotion::LineStart => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            rope.offset_of_line(line)
                        }
                        VimMotion::LineEnd => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let next = if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1)
                            } else {
                                rope.len()
                            };
                            next.saturating_sub(1)
                        }
                        VimMotion::DeleteChar => {
                            let len = doc_for_vim.rope_text().len();
                            let end = (cur_offset + 1).min(len);
                            doc_for_vim.edit_single(
                                Selection::region(cur_offset, end),
                                "",
                                EditType::Delete,
                            );
                            cur_offset.min(doc_for_vim.rope_text().len().saturating_sub(1))
                        }
                        VimMotion::DeleteLine => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let start = rope.offset_of_line(line);
                            let end = if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1)
                            } else {
                                rope.len()
                            };
                            doc_for_vim.edit_single(
                                Selection::region(start, end),
                                "",
                                EditType::Delete,
                            );
                            start.min(doc_for_vim.rope_text().len().saturating_sub(1))
                        }
                        VimMotion::EnterInsert => cur_offset,
                        VimMotion::EnterInsertAfter => {
                            let len = doc_for_vim.rope_text().len();
                            (cur_offset + 1).min(len)
                        }
                        VimMotion::EnterInsertNewlineBelow => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let insert_at = if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1)
                            } else {
                                rope.len()
                            };
                            doc_for_vim.edit_single(
                                Selection::caret(insert_at),
                                "\n",
                                EditType::InsertNewline,
                            );
                            insert_at
                        }
                        VimMotion::YankLine => {
                            // Copy the current line (including newline) into the vim register.
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let start = rope.offset_of_line(line);
                            let end = if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1)
                            } else {
                                rope.len()
                            };
                            let yanked = rope.slice_to_cow(start..end).to_string();
                            vim_register.set(yanked.clone());
                            // Also push to yank ring (cap 5)
                            yank_ring.update(|ring| {
                                ring.insert(0, yanked);
                                ring.truncate(5);
                            });
                            cur_offset // cursor stays in place after yank
                        }
                        VimMotion::Paste => {
                            // Paste yanked text AFTER the current line.
                            let text = vim_register.get_untracked();
                            if !text.is_empty() {
                                let rope = doc_for_vim.rope_text();
                                let line = rope.line_of_offset(cur_offset);
                                let insert_at = if line + 1 < rope.num_lines() {
                                    rope.offset_of_line(line + 1)
                                } else {
                                    // At last line — append after a newline
                                    let end = rope.len();
                                    doc_for_vim.edit_single(
                                        Selection::caret(end),
                                        "\n",
                                        EditType::InsertNewline,
                                    );
                                    end + 1
                                };
                                doc_for_vim.edit_single(
                                    Selection::caret(insert_at),
                                    &text,
                                    EditType::InsertChars,
                                );
                                insert_at
                            } else {
                                cur_offset
                            }
                        }
                        VimMotion::PasteBefore => {
                            // Paste yanked text BEFORE the current line.
                            let text = vim_register.get_untracked();
                            if !text.is_empty() {
                                let rope = doc_for_vim.rope_text();
                                let line = rope.line_of_offset(cur_offset);
                                let insert_at = rope.offset_of_line(line);
                                // Ensure pasted text ends with newline
                                let paste_text = if text.ends_with('\n') {
                                    text
                                } else {
                                    format!("{text}\n")
                                };
                                doc_for_vim.edit_single(
                                    Selection::caret(insert_at),
                                    &paste_text,
                                    EditType::InsertChars,
                                );
                                insert_at
                            } else {
                                cur_offset
                            }
                        }
                        VimMotion::GotoFileTop => 0,
                        VimMotion::GotoFileBottom => {
                            doc_for_vim.rope_text().len().saturating_sub(1)
                        }
                        VimMotion::HalfPageDown => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let col = cur_offset - rope.offset_of_line(line);
                            let target = (line + 20).min(rope.num_lines().saturating_sub(1));
                            let start = rope.offset_of_line(target);
                            let end = if target + 1 < rope.num_lines() {
                                rope.offset_of_line(target + 1).saturating_sub(1)
                            } else {
                                rope.len()
                            };
                            start + col.min(end.saturating_sub(start))
                        }
                        VimMotion::HalfPageUp => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let col = cur_offset - rope.offset_of_line(line);
                            let target = line.saturating_sub(20);
                            let start = rope.offset_of_line(target);
                            let end = if target + 1 < rope.num_lines() {
                                rope.offset_of_line(target + 1).saturating_sub(1)
                            } else {
                                rope.len()
                            };
                            start + col.min(end.saturating_sub(start))
                        }
                        VimMotion::InsertAtLineEnd => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1).saturating_sub(1)
                            } else {
                                rope.len()
                            }
                        }
                        VimMotion::InsertAtLineStart => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            rope.offset_of_line(line)
                        }
                        VimMotion::DeleteToLineEnd => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let end = if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1).saturating_sub(1)
                            } else {
                                rope.len()
                            };
                            if end > cur_offset {
                                doc_for_vim.edit_single(
                                    Selection::region(cur_offset, end),
                                    "",
                                    EditType::Delete,
                                );
                            }
                            cur_offset.min(doc_for_vim.rope_text().len().saturating_sub(1))
                        }
                        VimMotion::ChangeToLineEnd => {
                            // Enter insert mode set in app.rs key handler.
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let end = if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1).saturating_sub(1)
                            } else {
                                rope.len()
                            };
                            if end > cur_offset {
                                doc_for_vim.edit_single(
                                    Selection::region(cur_offset, end),
                                    "",
                                    EditType::Delete,
                                );
                            }
                            cur_offset.min(doc_for_vim.rope_text().len().saturating_sub(1))
                        }
                        VimMotion::ChangeWholeLine => {
                            // Delete line content (keep newline), position at line start.
                            // Enter insert mode set in app.rs key handler.
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let start = rope.offset_of_line(line);
                            let end = if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1).saturating_sub(1)
                            } else {
                                rope.len()
                            };
                            if end > start {
                                doc_for_vim.edit_single(
                                    Selection::region(start, end),
                                    "",
                                    EditType::Delete,
                                );
                            }
                            start.min(doc_for_vim.rope_text().len().saturating_sub(1))
                        }
                        // ── Replace char under cursor ─────────────────
                        VimMotion::ReplaceChar(ch) => {
                            let rope = doc_for_vim.rope_text();
                            let len = rope.len();
                            if cur_offset < len {
                                let ch_end = rope
                                    .slice_to_cow(cur_offset..len)
                                    .chars()
                                    .next()
                                    .map(|c| cur_offset + c.len_utf8())
                                    .unwrap_or(cur_offset + 1)
                                    .min(len);
                                doc_for_vim.edit_single(
                                    Selection::region(cur_offset, ch_end),
                                    &ch.to_string(),
                                    EditType::InsertChars,
                                );
                                dirty.set(true);
                            }
                            cur_offset.min(doc_for_vim.rope_text().len().saturating_sub(1))
                        }
                        // ── Jump to matching bracket ───────────────────
                        VimMotion::JumpMatchingBracket => {
                            let rope = doc_for_vim.rope_text();
                            let text = rope.slice_to_cow(0..rope.len()).to_string();
                            let bytes = text.as_bytes();
                            let len = bytes.len();
                            if cur_offset < len {
                                let ch = bytes[cur_offset] as char;
                                let (search, fwd) = match ch {
                                    '(' => (')', true),
                                    ')' => ('(', false),
                                    '[' => (']', true),
                                    ']' => ('[', false),
                                    '{' => ('}', true),
                                    '}' => ('{', false),
                                    _ => (ch, true),
                                };
                                if search != ch || fwd {
                                    let open = if fwd { ch } else { search };
                                    let close = if fwd { search } else { ch };
                                    let mut depth = 0i32;
                                    if fwd {
                                        for (idx, &b) in bytes.iter().enumerate().skip(cur_offset) {
                                            let c = b as char;
                                            if c == open {
                                                depth += 1;
                                            } else if c == close {
                                                depth -= 1;
                                                if depth == 0 {
                                                    break;
                                                }
                                                // continue
                                            }
                                            if depth == 0 {
                                                break;
                                            }
                                            let _ = idx;
                                        }
                                        // find forward
                                        let mut d = 0i32;
                                        let mut found = cur_offset;
                                        for (idx, &byte) in bytes[cur_offset..len]
                                            .iter()
                                            .enumerate()
                                            .map(|(i, b)| (i + cur_offset, b))
                                        {
                                            let c = byte as char;
                                            if c == open {
                                                d += 1;
                                            } else if c == close {
                                                d -= 1;
                                                if d == 0 {
                                                    found = idx;
                                                    break;
                                                }
                                            }
                                        }
                                        found
                                    } else {
                                        let mut d = 0i32;
                                        let mut found = cur_offset;
                                        for (idx, &byte) in
                                            bytes[..=cur_offset].iter().enumerate().rev()
                                        {
                                            let c = byte as char;
                                            if c == close {
                                                d += 1;
                                            } else if c == open {
                                                d -= 1;
                                                if d == 0 {
                                                    found = idx;
                                                    break;
                                                }
                                            }
                                        }
                                        found
                                    }
                                } else {
                                    cur_offset
                                }
                            } else {
                                cur_offset
                            }
                        }
                        // ── Change word (delete to next word boundary, enter insert) ──
                        VimMotion::ChangeWord => {
                            let rope = doc_for_vim.rope_text();
                            let len = rope.len();
                            let text = rope.slice_to_cow(0..rope.len()).to_string();
                            let bytes = text.as_bytes();
                            // Skip current word chars, then leading whitespace
                            let mut end = cur_offset;
                            // skip non-whitespace
                            while end < len
                                && bytes[end] != b' '
                                && bytes[end] != b'\t'
                                && bytes[end] != b'\n'
                            {
                                end += 1;
                            }
                            // skip trailing spaces/tabs (but not newline)
                            while end < len && (bytes[end] == b' ' || bytes[end] == b'\t') {
                                end += 1;
                            }
                            if end > cur_offset {
                                doc_for_vim.edit_single(
                                    Selection::region(cur_offset, end),
                                    "",
                                    EditType::Delete,
                                );
                                dirty.set(true);
                            }
                            cur_offset.min(doc_for_vim.rope_text().len().saturating_sub(1))
                        }
                        // ── Visual selection operations ───────────────
                        VimMotion::DeleteVisualSelection => {
                            let anchor = vim_visual_anchor.get_untracked().unwrap_or(cur_offset);
                            let (start, end) = (anchor.min(cur_offset), anchor.max(cur_offset));
                            let end = end.min(doc_for_vim.rope_text().len());
                            let text = doc_for_vim.rope_text().slice_to_cow(start..end).to_string();
                            yank_ring.update(|r| {
                                r.insert(0, text);
                                r.truncate(5);
                            });
                            if end > start {
                                doc_for_vim.edit_single(
                                    Selection::region(start, end),
                                    "",
                                    EditType::Delete,
                                );
                                dirty.set(true);
                            }
                            vim_visual_anchor.set(None);
                            start.min(doc_for_vim.rope_text().len())
                        }
                        VimMotion::YankVisualSelection => {
                            let anchor = vim_visual_anchor.get_untracked().unwrap_or(cur_offset);
                            let (start, end) = (anchor.min(cur_offset), anchor.max(cur_offset));
                            let end = end.min(doc_for_vim.rope_text().len());
                            let text = doc_for_vim.rope_text().slice_to_cow(start..end).to_string();
                            yank_ring.update(|r| {
                                r.insert(0, text);
                                r.truncate(5);
                            });
                            vim_visual_anchor.set(None);
                            cur_offset
                        }
                        VimMotion::ChangeVisualSelection => {
                            let anchor = vim_visual_anchor.get_untracked().unwrap_or(cur_offset);
                            let (start, end) = (anchor.min(cur_offset), anchor.max(cur_offset));
                            let end = end.min(doc_for_vim.rope_text().len());
                            if end > start {
                                doc_for_vim.edit_single(
                                    Selection::region(start, end),
                                    "",
                                    EditType::Delete,
                                );
                                dirty.set(true);
                            }
                            vim_visual_anchor.set(None);
                            start.min(doc_for_vim.rope_text().len())
                        }
                        // ── Visual mode start — anchor position ────────
                        VimMotion::VisualCharStart => {
                            vim_visual_anchor.set(Some(cur_offset));
                            cur_offset
                        }
                        VimMotion::VisualLineStart => {
                            let rope = doc_for_vim.rope_text();
                            let line = rope.line_of_offset(cur_offset);
                            let line_start = rope.offset_of_line(line);
                            vim_visual_anchor.set(Some(line_start));
                            // extend to end of line
                            if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1).saturating_sub(1)
                            } else {
                                rope.len()
                            }
                        }
                        // ── Set / goto vim mark ────────────────────────
                        VimMotion::SetMark(ch) => {
                            let path = tabs
                                .get_untracked()
                                .get(i)
                                .map(|t| t.path.clone())
                                .unwrap_or_default();
                            vim_marks.update(|m| {
                                m.insert(ch, (path, cur_offset));
                            });
                            cur_offset
                        }
                        VimMotion::GotoMark(ch) => {
                            if let Some(off) = vim_marks.get_untracked().get(&ch).map(|(_p, o)| *o)
                            {
                                off.min(doc_for_vim.rope_text().len())
                            } else {
                                cur_offset
                            }
                        }
                        // ── Repeat last edit ──────────────────────────
                        VimMotion::RepeatLastEdit => {
                            if let Some(last) = vim_last_motion.get_untracked() {
                                // Re-enqueue the last motion for next effect run.
                                vim_motion.set(Some(last));
                            }
                            cur_offset
                        }
                        // ── Ex mode — handled in app.rs key handler ───
                        VimMotion::EnterExMode => cur_offset,
                        // ── Expand / Shrink selection ─────────────────
                        // These are triggered via nonces, handled in separate effects below.
                        VimMotion::ExpandSelection | VimMotion::ShrinkSelection => cur_offset,
                    };

                    // Apply visual mode selection if active
                    let sel = if vim_visual_mode.get_untracked() {
                        let anchor = vim_visual_anchor.get_untracked().unwrap_or(new_offset);
                        Selection::region(anchor.min(new_offset), anchor.max(new_offset))
                    } else {
                        vim_visual_anchor.set(None);
                        Selection::caret(new_offset)
                    };
                    cursor_sig.set(Cursor::new(CursorMode::Insert(sel), None, None));
                });
            }

            // ── Expand / Shrink selection ─────────────────────────────────
            {
                let doc_for_es = doc.clone();
                let last_exp = create_rw_signal(0u64);
                let last_shr = create_rw_signal(0u64);
                create_effect(move |_| {
                    let exp_n = expand_selection_nonce.get();
                    let shr_n = shrink_selection_nonce.get();
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    if exp_n > 0 && exp_n != last_exp.get_untracked() {
                        last_exp.set(exp_n);
                        let rope = doc_for_es.rope_text();
                        let cur = cursor_sig.get_untracked();
                        let offset = cur.offset();
                        let len = rope.len();
                        // Expand: if at caret, select word; if word selected, select line
                        let (sel_start, sel_end) = if let CursorMode::Insert(ref s) = cur.mode {
                            if let Some(r) = s.regions().first().copied() {
                                (r.start.min(r.end), r.start.max(r.end))
                            } else {
                                (offset, offset)
                            }
                        } else {
                            (offset, offset)
                        };
                        let text = rope.slice_to_cow(0..rope.len()).to_string();
                        let bytes = text.as_bytes();
                        let new_sel = if sel_start == sel_end {
                            // Expand to word
                            let mut start = offset;
                            let mut end = offset;
                            while start > 0
                                && (bytes[start - 1].is_ascii_alphanumeric()
                                    || bytes[start - 1] == b'_')
                            {
                                start -= 1;
                            }
                            while end < len
                                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                            {
                                end += 1;
                            }
                            Selection::region(start, end)
                        } else {
                            // Expand to enclosing line
                            let line = rope.line_of_offset(sel_start);
                            let line_start = rope.offset_of_line(line);
                            let line_end = if line + 1 < rope.num_lines() {
                                rope.offset_of_line(line + 1).saturating_sub(1)
                            } else {
                                len
                            };
                            Selection::region(line_start, line_end)
                        };
                        cursor_sig.set(Cursor::new(CursorMode::Insert(new_sel), None, None));
                    }
                    if shr_n > 0 && shr_n != last_shr.get_untracked() {
                        last_shr.set(shr_n);
                        // Shrink to caret at start of selection
                        let cur = cursor_sig.get_untracked();
                        let offset = cur.offset();
                        cursor_sig.set(Cursor::new(
                            CursorMode::Insert(Selection::caret(offset)),
                            None,
                            None,
                        ));
                    }
                });
            }

            // ── Transform case (uppercase / lowercase) ───────────────────
            {
                let doc_for_tc = doc.clone();
                let last_upper = create_rw_signal(0u64);
                let last_lower = create_rw_signal(0u64);
                create_effect(move |_| {
                    let upper_n = transform_upper_nonce.get();
                    let lower_n = transform_lower_nonce.get();
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    if upper_n > 0 && upper_n != last_upper.get_untracked() {
                        last_upper.set(upper_n);
                        let cur = cursor_sig.get_untracked();
                        let offset = cur.offset();
                        let (sel_start, sel_end) = if let CursorMode::Insert(ref s) = cur.mode {
                            if let Some(r) = s.regions().first().copied() {
                                (r.start.min(r.end), r.start.max(r.end))
                            } else {
                                (offset, offset)
                            }
                        } else {
                            (offset, offset)
                        };
                        if sel_start < sel_end {
                            let text = doc_for_tc
                                .rope_text()
                                .slice_to_cow(sel_start..sel_end)
                                .to_uppercase();
                            doc_for_tc.edit_single(
                                Selection::region(sel_start, sel_end),
                                &text,
                                EditType::InsertChars,
                            );
                            dirty.set(true);
                        }
                    }
                    if lower_n > 0 && lower_n != last_lower.get_untracked() {
                        last_lower.set(lower_n);
                        let cur = cursor_sig.get_untracked();
                        let offset = cur.offset();
                        let (sel_start, sel_end) = if let CursorMode::Insert(ref s) = cur.mode {
                            if let Some(r) = s.regions().first().copied() {
                                (r.start.min(r.end), r.start.max(r.end))
                            } else {
                                (offset, offset)
                            }
                        } else {
                            (offset, offset)
                        };
                        if sel_start < sel_end {
                            let text = doc_for_tc
                                .rope_text()
                                .slice_to_cow(sel_start..sel_end)
                                .to_lowercase();
                            doc_for_tc.edit_single(
                                Selection::region(sel_start, sel_end),
                                &text,
                                EditType::InsertChars,
                            );
                            dirty.set(true);
                        }
                    }
                });
            }

            // ── Join lines ────────────────────────────────────────────────
            {
                let doc_for_jl = doc.clone();
                let last_jl = create_rw_signal(0u64);
                create_effect(move |_| {
                    let n = join_line_nonce.get();
                    if n == 0 || n == last_jl.get_untracked() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_jl.set(n);
                    let rope = doc_for_jl.rope_text();
                    let offset = cursor_sig.get_untracked().offset();
                    let line = rope.line_of_offset(offset);
                    if line + 1 >= rope.num_lines() {
                        return;
                    }
                    let cur_end = rope.offset_of_line(line + 1);
                    // Delete the newline and any leading whitespace on the next line
                    let next_content_start = {
                        let next_line_text = rope.slice_to_cow(cur_end..rope.len()).to_string();
                        let ws_len: usize = next_line_text
                            .chars()
                            .take_while(|c| *c == ' ' || *c == '\t')
                            .map(|c| c.len_utf8())
                            .sum();
                        // cur_end already points past the newline
                        // Actually cur_end = start of next line (past \n)
                        // We want to delete from (cur_end - 1) to (cur_end + ws_len)
                        ws_len
                    };
                    // Delete: newline char + leading whitespace on next line, replace with space
                    let del_start = cur_end.saturating_sub(1); // the '\n'
                    let del_end = cur_end + next_content_start;
                    doc_for_jl.edit_single(
                        Selection::region(del_start, del_end),
                        " ",
                        EditType::Delete,
                    );
                    dirty.set(true);
                });
            }

            // ── Sort lines (selected or whole file) ───────────────────────
            {
                let doc_for_sl = doc.clone();
                let last_sl = create_rw_signal(0u64);
                create_effect(move |_| {
                    let n = sort_lines_nonce.get();
                    if n == 0 || n == last_sl.get_untracked() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_sl.set(n);
                    let rope = doc_for_sl.rope_text();
                    let cur = cursor_sig.get_untracked();
                    let offset = cur.offset();
                    let (sel_start, sel_end) = if let CursorMode::Insert(ref s) = cur.mode {
                        if let Some(r) = s.regions().first().copied() {
                            (r.start.min(r.end), r.start.max(r.end))
                        } else {
                            (offset, offset)
                        }
                    } else {
                        (offset, offset)
                    };
                    // Determine line range to sort
                    let (range_start, range_end) = if sel_start < sel_end {
                        let first_line = rope.line_of_offset(sel_start);
                        let last_line = rope.line_of_offset(sel_end.saturating_sub(1));
                        let rs = rope.offset_of_line(first_line);
                        let re = if last_line + 1 < rope.num_lines() {
                            rope.offset_of_line(last_line + 1)
                        } else {
                            rope.len()
                        };
                        (rs, re)
                    } else {
                        // No selection: sort entire file
                        (0, rope.len())
                    };
                    let text = rope.slice_to_cow(range_start..range_end).to_string();
                    let mut lines: Vec<&str> = text.lines().collect();
                    lines.sort_unstable();
                    let sorted = lines.join("\n");
                    let sorted = if text.ends_with('\n') {
                        format!("{sorted}\n")
                    } else {
                        sorted
                    };
                    doc_for_sl.edit_single(
                        Selection::region(range_start, range_end),
                        &sorted,
                        EditType::InsertChars,
                    );
                    dirty.set(true);
                });
            }

            // ── Title case transform ──────────────────────────────────────
            {
                let doc_tt = doc.clone();
                let last_tt = create_rw_signal(0u64);
                create_effect(move |_| {
                    let n = transform_title_nonce.get();
                    if n == 0 || n == last_tt.get_untracked() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_tt.set(n);
                    let cur = cursor_sig.get_untracked();
                    let offset = cur.offset();
                    let (sel_start, sel_end) = if let CursorMode::Insert(ref s) = cur.mode {
                        if let Some(r) = s.regions().first().copied() {
                            (r.start.min(r.end), r.start.max(r.end))
                        } else {
                            (offset, offset)
                        }
                    } else {
                        (offset, offset)
                    };
                    if sel_start >= sel_end {
                        return;
                    }
                    let raw = doc_tt
                        .rope_text()
                        .slice_to_cow(sel_start..sel_end)
                        .to_string();
                    // Title-case: capitalize first letter of each word
                    let titled: String = raw
                        .split_inclusive(|c: char| c.is_whitespace() || c == '-' || c == '_')
                        .map(|word| {
                            let mut chars = word.chars();
                            match chars.next() {
                                None => String::new(),
                                Some(first) => {
                                    let upper: String = first.to_uppercase().collect();
                                    upper + chars.as_str()
                                }
                            }
                        })
                        .collect();
                    doc_tt.edit_single(
                        Selection::region(sel_start, sel_end),
                        &titled,
                        EditType::InsertChars,
                    );
                    dirty.set(true);
                });
            }

            // ── Format selection ──────────────────────────────────────────
            {
                let doc_fs = doc.clone();
                let last_fs = create_rw_signal(0u64);
                // Channel created once outside the effect to avoid Scope leak.
                // The channel is bounded(1); old results are discarded when a new
                // format request fires before the previous one is consumed.
                let (fmt_tx, fmt_rx) =
                    std::sync::mpsc::sync_channel::<(Option<String>, usize, usize)>(1);
                let fmt_result_sig = create_signal_from_channel(fmt_rx);
                // Receive effect: applies formatted text when result arrives.
                {
                    let doc_apply = doc_fs.clone();
                    create_effect(move |_| {
                        if let Some((Some(text), sel_start, sel_end)) = fmt_result_sig.get() {
                            doc_apply.edit_single(
                                Selection::region(sel_start, sel_end),
                                &text,
                                EditType::InsertChars,
                            );
                            dirty.set(true);
                        }
                    });
                }
                create_effect(move |_| {
                    let n = format_selection_nonce.get();
                    if n == 0 || n == last_fs.get_untracked() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_fs.set(n);
                    let cur = cursor_sig.get_untracked();
                    let offset = cur.offset();
                    let (sel_start, sel_end) = if let CursorMode::Insert(ref s) = cur.mode {
                        if let Some(r) = s.regions().first().copied() {
                            (r.start.min(r.end), r.start.max(r.end))
                        } else {
                            (offset, offset)
                        }
                    } else {
                        (offset, offset)
                    };
                    if sel_start >= sel_end {
                        return;
                    }
                    let rope = doc_fs.rope_text();
                    let sel_text = rope.slice_to_cow(sel_start..sel_end).to_string();
                    let ext = tabs
                        .get_untracked()
                        .get(i)
                        .and_then(|t| {
                            t.path
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_default();
                    let tx = fmt_tx.clone();
                    std::thread::spawn(move || {
                        let result: Option<String> = (|| {
                            let tmp = std::env::temp_dir().join(format!(
                                "phaze_fmt_{}.{}",
                                std::process::id(),
                                if ext.is_empty() {
                                    "txt".to_string()
                                } else {
                                    ext.clone()
                                }
                            ));
                            std::fs::write(&tmp, &sel_text).ok()?;
                            let (cmd, args): (&str, Vec<String>) = match ext.as_str() {
                                "rs" => ("rustfmt", vec![tmp.to_string_lossy().to_string()]),
                                "js" | "ts" | "jsx" | "tsx" => (
                                    "prettier",
                                    vec!["--write".to_string(), tmp.to_string_lossy().to_string()],
                                ),
                                "py" => ("black", vec![tmp.to_string_lossy().to_string()]),
                                _ => return None,
                            };
                            let ok = std::process::Command::new(cmd)
                                .args(&args)
                                .status()
                                .ok()?
                                .success();
                            if !ok {
                                return None;
                            }
                            let result = std::fs::read_to_string(&tmp).ok()?;
                            let _ = std::fs::remove_file(&tmp);
                            Some(result)
                        })();
                        let _ = tx.send((result, sel_start, sel_end));
                    });
                });
            }

            // ── Save without formatting ───────────────────────────────────
            {
                let doc_snf = doc.clone();
                let tab_path_snf = tab.path.clone();
                let tab_dirty_snf = tab.dirty;
                let lsp_cmd_snf = lsp_cmd.clone();
                let last_snf = create_rw_signal(0u64);
                create_effect(move |_| {
                    let n = save_no_format_nonce.get();
                    if n == 0 || n == last_snf.get_untracked() {
                        return;
                    }
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    last_snf.set(n);
                    let content = doc_snf.text().to_string();
                    if std::fs::write(&tab_path_snf, content).is_ok() {
                        tab_dirty_snf.set(false);
                        let _ = lsp_cmd_snf.send(crate::lsp_bridge::LspCommand::SaveFile {
                            path: tab_path_snf.clone(),
                        });
                    }
                });
            }

            // ── Fold all / Unfold all ─────────────────────────────────────
            {
                let last_fa = create_rw_signal(0u64);
                let last_ua = create_rw_signal(0u64);
                create_effect(move |_| {
                    let fa = fold_all_nonce.get();
                    let ua = unfold_all_nonce.get();
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    if fa > 0 && fa != last_fa.get_untracked() {
                        last_fa.set(fa);
                        fold_state.update(|(ranges, folded)| {
                            for (start, _end) in ranges.iter() {
                                folded.insert(*start);
                            }
                        });
                    }
                    if ua > 0 && ua != last_ua.get_untracked() {
                        last_ua.set(ua);
                        fold_state.update(|(_ranges, folded)| {
                            folded.clear();
                        });
                    }
                });
            }

            // ── Ghost text / FIM debounce ─────────────────────────────────
            // Fires on every cursor move for the active tab. Waits 300 ms
            // then sends a single-turn LLM request for an inline completion.
            {
                let doc_for_fim = doc.clone();
                let fim_gen2 = Arc::clone(&fim_gen);
                let fim_tx2 = fim_tx.clone();

                create_effect(move |_| {
                    if active_idx.get() != Some(i) {
                        return;
                    }
                    let offset = cursor_sig.get().offset();

                    // Bump generation: in-flight request with old gen is silently discarded.
                    let my_gen = fim_gen2.fetch_add(1, Ordering::SeqCst) + 1;
                    // Clear stale suggestion whenever cursor moves.
                    ghost_text.set(None);

                    // Extract prefix and suffix inside the reactive scope (UI thread).
                    let rope = doc_for_fim.rope_text();
                    let len = rope.len();
                    if len == 0 || offset == 0 {
                        return;
                    }

                    let prefix = rope.slice_to_cow(0..offset).to_string();
                    if prefix.trim().is_empty() {
                        return;
                    }
                    // Don't fire FIM when cursor is at start of a line (nothing typed yet on this line)
                    let current_line_prefix =
                        prefix.rsplit_once('\n').map(|(_, s)| s).unwrap_or(&prefix);
                    if current_line_prefix.trim().is_empty() {
                        return;
                    }
                    let suffix = rope.slice_to_cow(offset..len).to_string();

                    let gen_check = Arc::clone(&fim_gen2);
                    let tx = fim_tx2.clone();

                    std::thread::spawn(move || {
                        // 300 ms debounce — if cursor moved, gen will have changed.
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        if gen_check.load(Ordering::SeqCst) != my_gen {
                            return;
                        }

                        let settings = Settings::load();
                        let rt = match tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                        {
                            Ok(rt) => rt,
                            Err(_) => return,
                        };

                        let suggestion = rt.block_on(async move {
                            let client = match settings.build_llm_client() {
                                Ok(c) => c,
                                Err(_) => return String::new(),
                            };

                            // Trim to reasonable context window sizes.
                            let pre = if prefix.len() > 1500 {
                                let start = prefix.ceil_char_boundary(prefix.len() - 1500);
                                prefix[start..].to_string()
                            } else {
                                prefix
                            };
                            let suf = if suffix.len() > 400 {
                                let end = suffix.floor_char_boundary(400);
                                suffix[..end].to_string()
                            } else {
                                suffix
                            };

                            let prompt = format!(
                                "You are a code completion engine. \
                                 Complete the code at the <CURSOR> marker. \
                                 Return ONLY the text to insert at <CURSOR> — \
                                 no explanation, no markdown, no backticks. \
                                 Maximum 3 lines.\n\n\
                                 {pre}<CURSOR>{suf}"
                            );

                            let msgs = [Message::user(prompt)];
                            match client.chat(&msgs, &[]).await {
                                Ok(resp) => {
                                    let text = resp
                                        .message
                                        .content
                                        .trim()
                                        .trim_start_matches("```")
                                        .trim_end_matches("```")
                                        .trim()
                                        .to_string();
                                    // Discard if suspiciously long (hallucination)
                                    if text.lines().count() > 6 {
                                        String::new()
                                    } else {
                                        text
                                    }
                                }
                                Err(_) => String::new(),
                            }
                        });

                        // One last generation check before writing to channel.
                        if gen_check.load(Ordering::SeqCst) == my_gen && !suggestion.is_empty() {
                            let _ = tx.try_send(suggestion);
                        }
                    });
                });
            }

            // Skip syntax highlighting for large files (> 2 MB) for performance.
            let is_large_file = tab
                .path
                .metadata()
                .map(|m| m.len() > 2 * 1024 * 1024)
                .unwrap_or(false);

            // Build initial syntect-based styling for this file's language
            let base_styling = make_base_styling(initial_fs, word_wrap.get_untracked());
            let mut syn_style =
                SyntaxStyle::for_extension(if is_large_file { "" } else { &tab_ext }, base_styling);
            syn_style.set_doc(doc.clone());

            // ── Git gutter decorations ────────────────────────────────────
            // Re-runs on every save (dirty toggles false) and on first mount.
            // Calls `git diff HEAD` in a background thread; result is delivered
            // back to the UI thread via create_signal_from_channel.
            let git_changes: RwSignal<Vec<(usize, u8)>> = create_rw_signal(vec![]);
            {
                let git_path = tab.path.clone();
                let (git_tx, git_rx) =
                    std::sync::mpsc::sync_channel::<Vec<(usize, u8)>>(1);
                let git_result_sig = create_signal_from_channel(git_rx);
                // Receive effect: applies git change markers when result arrives.
                create_effect(move |_| {
                    if let Some(changes) = git_result_sig.get() {
                        git_changes.set(changes);
                    }
                });
                // Trigger effect: re-runs on save, spawns background git diff.
                create_effect(move |_| {
                    let _dirty = safe_get(dirty, false); // tracked — re-runs on save
                    let p = git_path.clone();
                    let tx = git_tx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(git_changed_lines(&p));
                    });
                });
            }

            // ── Git blame data (per-line author/date) ────────────────────
            // Fetched once on save; only the current cursor line's blame
            // is shown inline (to avoid visual clutter).
            let blame_data: RwSignal<Vec<(String, String)>> = create_rw_signal(vec![]);
            {
                let blame_path = tab.path.clone();
                let (blame_tx, blame_rx) =
                    std::sync::mpsc::sync_channel::<Vec<(String, String)>>(1);
                let blame_result_sig = create_signal_from_channel(blame_rx);
                // Receive effect: applies blame entries when result arrives.
                create_effect(move |_| {
                    if let Some(data) = blame_result_sig.get() {
                        blame_data.set(data);
                    }
                });
                // Trigger effect: re-runs on save, spawns background git blame.
                create_effect(move |_| {
                    let _dirty = safe_get(dirty, false);
                    let p = blame_path.clone();
                    let tx = blame_tx.clone();
                    std::thread::spawn(move || {
                        let dir = p.parent().unwrap_or(&p);
                        let out = std::process::Command::new("git")
                            .args(["blame", "--date=short", "--porcelain"])
                            .arg(&p)
                            .current_dir(dir)
                            .output();
                        let mut entries: Vec<(String, String)> = vec![];
                        if let Ok(output) = out {
                            if output.status.success() {
                                let text = String::from_utf8_lossy(&output.stdout);
                                let mut cur_author = String::new();
                                let mut cur_date = String::new();
                                for line in text.lines() {
                                    if let Some(rest) = line.strip_prefix("author ") {
                                        cur_author = rest.to_string();
                                    } else if let Some(rest) = line.strip_prefix("author-time ") {
                                        // porcelain gives unix timestamp; convert to date
                                        if let Ok(ts) = rest.parse::<i64>() {
                                            let secs_per_day = 86400;
                                            let days = ts / secs_per_day;
                                            let y = 1970 + (days / 365);
                                            cur_date = format!("{y}");
                                        }
                                    } else if let Some(rest) = line.strip_prefix("committer-time ")
                                    {
                                        // Use committer date as fallback
                                        let _ = rest;
                                    } else if line.starts_with('\t') {
                                        // Content line = one blame entry complete
                                        entries.push((cur_author.clone(), cur_date.clone()));
                                    }
                                }
                            }
                        }
                        let _ = tx.send(entries);
                    });
                });
            }

            // ── Reactive styling update: font-size + diagnostics + word highlights + folds ──
            // Rebuilds SyntaxStyle whenever any tracked signal changes.
            // Never recreates the editor view, so undo/redo history is preserved.
            {
                let doc_for_style = doc.clone();
                let ext_for_style = tab_ext.clone();
                let editor_for_style = editor_ref.clone();
                let path_for_diag = tab.path.clone();
                create_effect(move |_| {
                    let fs = font_size.get() as usize;
                    let use_wrap = word_wrap.get(); // tracked — triggers rebuild when toggled
                    let all_diags = diagnostics.get();
                    let hl_ranges = word_hl.get();
                    let git_chgs = git_changes.get();
                    let cur_line = current_line_sig.get();
                    let (fold_ranges, folded) = fold_state.get();
                    let bp_pairs = bracket_pairs_sig.get();
                    let match_brkt = matching_bracket_sig.get();
                    let find_offs = find_match_offsets.get();
                    let find_q = find_query.get();
                    let blame_entries = blame_data.get();
                    let my_diags: Vec<(usize, DiagSeverity)> = all_diags
                        .iter()
                        .filter(|d| d.path == path_for_diag)
                        .map(|d| (d.line.saturating_sub(1) as usize, d.severity))
                        .collect();
                    let new_base = make_base_styling(fs, use_wrap);
                    let mut new_style = SyntaxStyle::for_extension(
                        if is_large_file { "" } else { &ext_for_style },
                        new_base,
                    );
                    new_style.set_doc(doc_for_style.clone());
                    new_style.diag_lines = my_diags;
                    new_style.highlight_ranges = hl_ranges;
                    new_style.git_lines = git_chgs;
                    let pal = &theme.get().palette;
                    new_style.git_color_added = pal.git_added.with_alpha(0.86);
                    new_style.git_color_modified = pal.git_modified.with_alpha(0.86);
                    new_style.git_color_deleted = pal.git_deleted.with_alpha(0.86);
                    new_style.current_line = cur_line;
                    new_style.foldable_ranges = fold_ranges;
                    new_style.folded_starts = folded;
                    new_style.bracket_pairs = bp_pairs.clone();
                    // Build bracket pair guides from pairs that span multiple lines
                    if let Some(ref rope_doc) = new_style.doc {
                        let rope = rope_doc.rope_text();
                        let guides: Vec<(usize, usize, usize, usize)> = bp_pairs
                            .iter()
                            .filter_map(|&(open_b, close_b, depth)| {
                                let open_line = rope.line_of_offset(open_b);
                                let close_line = rope.line_of_offset(close_b);
                                if close_line <= open_line + 1 {
                                    return None; // same or adjacent line, no guide needed
                                }
                                let line_start = rope.offset_of_line(open_line);
                                let open_col = open_b.saturating_sub(line_start);
                                Some((open_line, open_col, close_line, depth))
                            })
                            .collect();
                        new_style.bracket_pair_guides = guides;
                    }
                    new_style.matching_bracket = match_brkt;
                    // Inline blame for the current cursor line
                    new_style.blame_line = if cur_line < blame_entries.len() {
                        let (ref author, ref date) = blame_entries[cur_line];
                        if author.is_empty() || author == "Not Committed Yet" {
                            None
                        } else {
                            Some((cur_line, format!("{author} • {date}")))
                        }
                    } else {
                        None
                    };
                    // Push blame to IdeState for status bar display
                    if let Some((_, ref text)) = new_style.blame_line {
                        active_blame.set(text.clone());
                    } else {
                        active_blame.set(String::new());
                    }
                    // Convert start offsets \u2192 (start, end) ranges using query length.
                    new_style.find_match_ranges = if find_q.is_empty() {
                        vec![]
                    } else {
                        find_offs.iter().map(|&s| (s, s + find_q.len())).collect()
                    };
                    editor_for_style.update_styling(Rc::new(new_style));
                });
            }

            // Store in registry for save + find
            docs_for_stack.borrow_mut().insert(key, doc);

            raw_editor
                .styling(syn_style)
                .editor_style(move |style| {
                    let t = theme.get();
                    let p = &t.palette;
                    default_dark_color(style)
                        .gutter_dim_color(p.text_disabled)
                        .gutter_accent_color(p.text_primary)
                        .gutter_current_color(p.bg_elevated)
                        .gutter_left_padding(6.0)
                        .gutter_right_padding(10.0)
                        .modal_relative_line(relative_line_numbers.get())
                })
                .update({
                    let as_gen = Arc::clone(&auto_save_gen);
                    let as_tx = auto_save_tx.clone();
                    move |_| {
                        dirty.set(true);
                        // Notify LSP server of content change (textDocument/didChange).
                        let text = doc_for_lsp.text().to_string();
                        let ver = lsp_ver.get();
                        lsp_ver.update(|v| *v += 1);
                        let _ = lsp_tx.send(crate::lsp_bridge::LspCommand::ChangeFile {
                            path: lsp_path.clone(),
                            text,
                            version: ver,
                        });
                        // Auto-save: debounce 1.5 s — each edit cancels the previous timer.
                        if auto_save.get_untracked() {
                            let gen = as_gen.fetch_add(1, Ordering::Relaxed) + 1;
                            let gen_ref = Arc::clone(&as_gen);
                            let tx = as_tx.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(1500));
                                if gen_ref.load(Ordering::Relaxed) == gen {
                                    let _ = tx.try_send(());
                                }
                            });
                        }
                    }
                })
                .style(move |s| {
                    s.size_full()
                        .apply_if(!is_active(), |s| s.display(floem::style::Display::None))
                })
        },
    )
    .style(|s| s.flex_grow(1.0).min_height(0.0).min_width(0.0).width_full());

    // ── Minimap — scaled-down document overview ────────────────────────────
    // Renders each line of the active document as a 1 px tall bar whose width
    // indicates line length.  Diagnostic lines get colored markers; the current
    // cursor line gets a bright accent indicator.
    let minimap_docs = docs_for_find.clone();
    let heatmap = canvas(move |cx, size| {
        let t = theme.get();
        let p = &t.palette;
        let h = size.height;
        let w = size.width;
        cx.fill(&floem::kurbo::Rect::ZERO.with_size(size), p.glass_bg, 0.0);

        // Left-edge separator
        cx.fill(&floem::kurbo::Rect::new(0.0, 0.0, 1.0, h), p.border, 0.0);

        // Get the active document text
        let active = active_idx.get();
        let tab_list = tabs.get();
        let doc_text = active.and_then(|idx| {
            let tab = tab_list.get(idx)?;
            let key = tab.path.to_string_lossy().to_string();
            let reg = minimap_docs.borrow();
            let doc = reg.get(&key)?;
            Some(doc.text().to_string())
        });

        let Some(text) = doc_text else {
            return;
        };
        let line_count = text.lines().count().max(1);
        let max_line_len = text.lines().map(|l| l.len()).max().unwrap_or(1).max(1);
        let scale_y = h / (line_count as f64);
        let line_h = scale_y.clamp(1.0, 3.0);

        // Get diagnostic info for the active tab
        let all_diags = diagnostics.get();
        let diag_path = active
            .and_then(|idx| tab_list.get(idx))
            .map(|t| t.path.clone());

        let _cur_line = active.map(|_| {
            find_cur_match.get(); // just for reactivity trigger
            0usize // placeholder — we use active_cursor below
        });

        for (i, line) in text.lines().enumerate() {
            let y = (i as f64) * scale_y;
            if y > h {
                break;
            }

            // Line length as width proportion
            let frac = (line.len() as f64) / (max_line_len as f64);
            let bar_w = 2.0 + frac * (w - 4.0);

            // Base color: slightly brighter for non-empty lines
            let alpha = if line.trim().is_empty() { 15 } else { 45 };
            let bar_color = floem::peniko::Color::from_rgba8(180, 190, 210, alpha);

            cx.fill(
                &floem::kurbo::Rect::new(2.0, y, 2.0 + bar_w, y + line_h),
                bar_color,
                0.0,
            );
        }

        // Overlay diagnostic markers
        if let Some(ref dp) = diag_path {
            for diag in &all_diags {
                if &diag.path != dp {
                    continue;
                }
                let dl = diag.line.saturating_sub(1) as usize;
                let y = (dl as f64) * scale_y;
                let color = match diag.severity {
                    crate::lsp_bridge::DiagSeverity::Error => p.error.with_alpha(0.9),
                    crate::lsp_bridge::DiagSeverity::Warning => p.warning.with_alpha(0.8),
                    _ => p.accent.with_alpha(0.5),
                };
                cx.fill(
                    &floem::kurbo::Rect::new(1.0, y, w, y + line_h.max(2.0)),
                    color,
                    0.0,
                );
            }
        }

        // Draw viewport indicator using current line from active_cursor
        if let Some((_, line_num, _)) = active_cursor.get() {
            let y = (line_num as f64) * scale_y;
            cx.fill(
                &floem::kurbo::Rect::new(0.0, y - 1.0, w, y + 2.0),
                p.accent.with_alpha(0.7),
                0.0,
            );
        }
    })
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let bg = if t.is_cosmic() { p.glass_bg } else { p.bg_deep };
        s.width(60.0).height_full().min_width(60.0).background(bg)
    });

    // ── Welcome screen ─────────────────────────────────────────────────────
    let welcome = container(
        stack((
            phaze_icon(icons::FILE, 48.0, move |p| p.text_muted, theme),
            label(|| "Open a file to start editing").style(move |s| {
                s.color(theme.get().palette.text_muted)
                    .font_size(13.0)
                    .margin_top(8.0)
            }),
            label(|| "Use the explorer panel on the left").style(move |s| {
                s.color(theme.get().palette.text_disabled)
                    .font_size(11.0)
                    .margin_top(4.0)
            }),
        ))
        .style(|s| s.flex_col().items_center()),
    )
    .style(move |s| {
        let has = !tabs.get().is_empty();
        s.flex_grow(1.0)
            .min_height(0.0)
            .items_center()
            .justify_center()
            .apply_if(has, |s| s.display(floem::style::Display::None))
    });

    let content_area = stack((welcome, editor_body))
        .style(|s| s.flex_grow(1.0).min_height(0.0).min_width(0.0).width_full());

    let editor_row = stack((sentient_gutter, content_area, heatmap))
        .style(|s| s.flex_grow(1.0).min_height(0.0).min_width(0.0).width_full());

    // ── Find bar (Ctrl+F) ─────────────────────────────────────────────────────
    let find_bar = {
        let match_label = label(move || {
            let offsets = find_match_offsets.get();
            let cur = find_cur_match.get();
            if offsets.is_empty() {
                if find_query.get().is_empty() {
                    "".to_string()
                } else {
                    "No matches".to_string()
                }
            } else {
                format!("{}/{}", cur + 1, offsets.len())
            }
        })
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let no_match = !find_query.get().is_empty() && find_match_offsets.get().is_empty();
            s.font_size(12.0)
                .color(if no_match { p.error } else { p.text_muted })
                .margin_left(8.0)
                .min_width(80.0)
        });

        let prev_btn = container(label(|| "↑"))
            .style(move |s| {
                s.padding_horiz(8.0)
                    .padding_vert(3.0)
                    .font_size(13.0)
                    .color(theme.get().palette.text_secondary)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .border_radius(4.0)
                    .hover(|s| s.background(theme.get().palette.bg_elevated))
            })
            .on_click_stop(move |_| {
                let offs = find_match_offsets.get();
                if offs.is_empty() {
                    return;
                }
                let cur = find_cur_match.get();
                let prev = if cur == 0 { offs.len() - 1 } else { cur - 1 };
                find_cur_match.set(prev);
                find_jump_offset.set(offs[prev]);
                find_jump_nonce.update(|n| *n += 1);
            });

        let next_btn = container(label(|| "↓"))
            .style(move |s| {
                s.padding_horiz(8.0)
                    .padding_vert(3.0)
                    .font_size(13.0)
                    .color(theme.get().palette.text_secondary)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .border_radius(4.0)
                    .hover(|s| s.background(theme.get().palette.bg_elevated))
            })
            .on_click_stop(move |_| {
                let offs = find_match_offsets.get();
                if offs.is_empty() {
                    return;
                }
                let cur = find_cur_match.get();
                let next = (cur + 1) % offs.len();
                find_cur_match.set(next);
                find_jump_offset.set(offs[next]);
                find_jump_nonce.update(|n| *n += 1);
            });

        let close_btn = container(label(|| "✕"))
            .style(move |s| {
                s.padding_horiz(8.0)
                    .padding_vert(3.0)
                    .font_size(12.0)
                    .color(theme.get().palette.text_muted)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .border_radius(4.0)
                    .hover(|s| s.background(theme.get().palette.bg_elevated))
            })
            .on_click_stop(move |_| {
                find_open.set(false);
                find_query.set(String::new());
            });

        // Case-sensitive toggle button (Aa)
        let case_btn = container(label(|| "Aa"))
            .style(move |s| {
                let p = theme.get().palette;
                s.padding_horiz(6.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .color(if find_case.get() {
                        p.bg_base
                    } else {
                        p.text_muted
                    })
                    .background(if find_case.get() {
                        p.accent
                    } else {
                        p.bg_elevated
                    })
                    .border(1.0)
                    .border_color(p.border)
            })
            .on_click_stop(move |_| {
                find_case.update(|v| *v = !*v);
            });

        // Regex toggle button (.*)
        // Whole-word toggle button (W)
        let word_btn = container(label(|| "W"))
            .style(move |s| {
                let p = theme.get().palette;
                s.padding_horiz(6.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .font_weight(Weight::BOLD)
                    .color(if find_whole_word.get() {
                        p.bg_base
                    } else {
                        p.text_muted
                    })
                    .background(if find_whole_word.get() {
                        p.accent
                    } else {
                        p.bg_elevated
                    })
                    .border(1.0)
                    .border_color(p.border)
            })
            .on_click_stop(move |_| {
                find_whole_word.update(|v| *v = !*v);
            });

        let regex_btn = container(label(|| ".*"))
            .style(move |s| {
                let p = theme.get().palette;
                s.padding_horiz(6.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .font_size(11.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .color(if find_regex_mode.get() {
                        p.bg_base
                    } else {
                        p.text_muted
                    })
                    .background(if find_regex_mode.get() {
                        p.accent
                    } else {
                        p.bg_elevated
                    })
                    .border(1.0)
                    .border_color(p.border)
            })
            .on_click_stop(move |_| {
                find_regex_mode.update(|v| *v = !*v);
            });

        let find_input = text_input(find_query).style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.width(220.0)
                .padding_horiz(8.0)
                .padding_vert(4.0)
                .font_size(13.0)
                .color(p.text_primary)
                .background(p.bg_elevated)
                .border(1.0)
                .border_color(p.border_focus)
                .border_radius(4.0)
        });

        let replace_input = text_input(replace_query)
            .placeholder("Replace…")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.width(180.0)
                    .padding_horiz(8.0)
                    .padding_vert(4.0)
                    .font_size(13.0)
                    .color(p.text_primary)
                    .background(p.bg_elevated)
                    .border(1.0)
                    .border_color(p.border)
                    .border_radius(4.0)
                    .apply_if(!replace_open.get(), |s| {
                        s.display(floem::style::Display::None)
                    })
            });

        let replace_btn = container(label(|| "Replace"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(8.0)
                    .padding_vert(3.0)
                    .font_size(12.0)
                    .color(p.text_secondary)
                    .background(p.bg_elevated)
                    .border(1.0)
                    .border_color(p.border)
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(p.accent_dim))
                    .apply_if(!replace_open.get(), |s| {
                        s.display(floem::style::Display::None)
                    })
            })
            .on_click_stop(move |_| {
                replace_nonce.update(|n| *n += 1);
            });

        let replace_all_btn = container(label(|| "All"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(8.0)
                    .padding_vert(3.0)
                    .font_size(12.0)
                    .color(p.accent)
                    .background(p.bg_elevated)
                    .border(1.0)
                    .border_color(p.border)
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(p.accent_dim))
                    .apply_if(!replace_open.get(), |s| {
                        s.display(floem::style::Display::None)
                    })
            })
            .on_click_stop(move |_| {
                replace_all_nonce.update(|n| *n += 1);
            });

        let replace_toggle = container(label(move || if replace_open.get() { "▾" } else { "▸" }))
            .style(move |s| {
                let t = theme.get();
                s.padding_horiz(6.0)
                    .padding_vert(3.0)
                    .font_size(11.0)
                    .color(t.palette.text_muted)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .border_radius(3.0)
                    .hover(|s| s.background(t.palette.bg_elevated))
            })
            .on_click_stop(move |_| replace_open.update(|v| *v = !*v));

        container(
            stack((
                replace_toggle,
                find_input,
                case_btn,
                word_btn,
                regex_btn,
                match_label,
                prev_btn,
                next_btn,
                replace_input,
                replace_btn,
                replace_all_btn,
                close_btn,
            ))
            .style(|s| s.items_center().gap(4.0)),
        )
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let shown = find_open.get();
            s.width_full()
                .height(36.0)
                .items_center()
                .padding_horiz(12.0)
                .background(p.bg_elevated)
                .border_bottom(1.0)
                .border_color(p.border)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        // Escape closes the find bar
        .on_event_stop(EventListener::KeyDown, move |ev| {
            if let Event::KeyDown(e) = ev {
                match &e.key.logical_key {
                    Key::Named(floem::keyboard::NamedKey::Escape) => {
                        find_open.set(false);
                        find_query.set(String::new());
                        replace_open.set(false);
                    }
                    Key::Named(floem::keyboard::NamedKey::Enter) => {
                        let offs = find_match_offsets.get();
                        if offs.is_empty() {
                            return;
                        }
                        let cur = find_cur_match.get();
                        let next = (cur + 1) % offs.len();
                        find_cur_match.set(next);
                        find_jump_offset.set(offs[next]);
                        find_jump_nonce.update(|n| *n += 1);
                    }
                    _ => {}
                }
            }
        })
    };

    // ── Goto-line overlay (Ctrl+G) ────────────────────────────────────────────
    let goto_overlay = {
        let input = text_input(goto_query).style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.width(160.0)
                .padding(8.0)
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_elevated)
                .border(1.0)
                .border_color(p.border_focus)
                .border_radius(6.0)
        });

        let hint = label(|| "Go to line…").style(move |s| {
            s.font_size(11.0)
                .color(theme.get().palette.text_muted)
                .margin_bottom(6.0)
        });

        let box_view = stack((hint, input))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.flex_col()
                    .padding(16.0)
                    .background(p.bg_panel)
                    .border(1.0)
                    .border_color(p.glass_border)
                    .border_radius(8.0)
                    .box_shadow_h_offset(0.0)
                    .box_shadow_v_offset(4.0)
                    .box_shadow_blur(30.0)
                    .box_shadow_color(p.glow)
                    .box_shadow_spread(0.0)
            })
            .on_event_stop(EventListener::KeyDown, move |ev| {
                if let Event::KeyDown(e) = ev {
                    match &e.key.logical_key {
                        Key::Named(floem::keyboard::NamedKey::Escape) => {
                            goto_open.set(false);
                            goto_query.set(String::new());
                        }
                        Key::Named(floem::keyboard::NamedKey::Enter) => {
                            let q = goto_query.get();
                            if let Ok(n) = q.trim().parse::<usize>() {
                                goto_line.set(n);
                                goto_nonce.update(|v| *v += 1);
                            }
                            goto_open.set(false);
                            goto_query.set(String::new());
                        }
                        _ => {}
                    }
                }
            });

        container(box_view)
            .style(move |s| {
                let shown = goto_open.get();
                s.absolute()
                    .inset(0)
                    .items_start()
                    .justify_center()
                    .padding_top(60.0)
                    .background(floem::peniko::Color::from_rgba8(0, 0, 0, 140))
                    .z_index(50)
                    .apply_if(!shown, |s| s.display(floem::style::Display::None))
            })
            .on_click_stop(move |_| {
                goto_open.set(false);
                goto_query.set(String::new());
            })
    };

    // ── Ghost text suggestion strip ─────────────────────────────────────────
    // Shown at the bottom of the editor content when an FIM suggestion is ready.
    // Displays the first line of the suggestion; Tab accepts the full text.
    let ghost_strip = container(
        stack((
            label(move || {
                ghost_text
                    .get()
                    .as_deref()
                    .map(|t| t.lines().next().unwrap_or("").to_string())
                    .unwrap_or_default()
            })
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(12.0)
                    .color(p.text_muted)
                    .font_family("JetBrains Mono, Fira Code, Cascadia Code, monospace".to_string())
                    .flex_grow(1.0)
            }),
            label(|| "  ↹ Tab to accept").style(move |s| {
                let p = theme.get().palette;
                s.font_size(10.0).color(p.accent).padding_left(8.0)
            }),
        ))
        .style(|s| s.flex_row().items_center().width_full()),
    )
    .style(move |s| {
        let shown = ghost_text.get().is_some();
        let p = theme.get().palette;
        s.width_full()
            .height(26.0)
            .padding_horiz(12.0)
            .padding_vert(4.0)
            .background(p.bg_elevated)
            .border_top(1.0)
            .border_color(p.border)
            .apply_if(!shown, |s| s.display(floem::style::Display::None))
    });

    // ── Sticky scroll bar ────────────────────────────────────────────────────
    // Shows the enclosing scope headers (fn/struct/impl) pinned just above the
    // editor body when the cursor is scrolled into a nested block.
    let sticky_bar = {
        let sticky_theme = theme;
        dyn_stack(
            move || sticky_lines_out.get(),
            |line| line.clone(),
            move |line| {
                let txt = label(move || line.clone()).style(move |s| {
                    let t = sticky_theme.get();
                    s.font_size(12.0)
                        .color(t.palette.text_muted)
                        .padding_horiz(12.0)
                        .padding_vert(2.0)
                        .font_family("JetBrains Mono".to_string())
                });
                container(txt).style(move |s| {
                    let t = sticky_theme.get();
                    s.width_full()
                        .background(t.palette.bg_elevated.with_alpha(0.92))
                        .border_bottom(1.0)
                        .border_color(t.palette.border)
                })
            },
        )
        .style(move |s| {
            let empty = sticky_lines_out.get().is_empty();
            s.flex_col()
                .width_full()
                .apply_if(empty, |s| s.display(floem::style::Display::None))
        })
    };

    // ── Code lens bar (shown above editor when there are entries) ────────────
    let code_lens_bar = {
        let cl_theme = theme;
        dyn_stack(
            move || {
                if !code_lens_visible.get() {
                    return vec![];
                }
                code_lens_sig.get()
            },
            |entry| entry.line,
            move |entry| {
                let t = cl_theme.get();
                let _p = &t.palette;
                label(move || format!("● {} ─ line {}", entry.label, entry.line + 1))
                    .style(move |s| {
                        let t2 = cl_theme.get();
                        s.padding_horiz(10.0)
                            .padding_vert(2.0)
                            .font_size(11.0)
                            .color(t2.palette.accent)
                            .cursor(floem::style::CursorStyle::Pointer)
                            .hover(|s| s.color(t2.palette.text_primary))
                    })
                    .on_click_stop(move |_| {
                        // Jump to the line containing this lens entry
                        ext_goto_line.set(entry.line + 1);
                    })
            },
        )
        .style(move |s| {
            let visible = code_lens_visible.get() && !code_lens_sig.get().is_empty();
            s.flex_row()
                .flex_wrap(floem::style::FlexWrap::Wrap)
                .width_full()
                .background(floem::peniko::Color::from_rgba8(0, 0, 0, 40))
                .border_bottom(1.0)
                .apply_if(!visible, |s| s.display(floem::style::Display::None))
        })
    };

    // ── Inlay hints bar — shows type annotations for the current cursor line ──
    let inlay_bar = {
        let ih_theme = theme;
        dyn_stack(
            move || {
                if !inlay_hints_toggle.get() {
                    return vec![];
                }
                let cur_line = active_cursor
                    .get()
                    .map(|(_, l, _)| l.saturating_sub(1))
                    .unwrap_or(0);
                inlay_hints
                    .get()
                    .into_iter()
                    .filter(|h| h.line == cur_line)
                    .collect::<Vec<_>>()
            },
            |h| format!("{}{}{}", h.line, h.col, h.label),
            move |h| {
                let _t = ih_theme.get();
                label(move || format!("  {} col:{} {}", "⟩", h.col, h.label)).style(move |s| {
                    let t2 = ih_theme.get();
                    s.padding_horiz(8.0)
                        .padding_vert(1.0)
                        .font_size(10.0)
                        .color(t2.palette.text_secondary)
                        .font_style(floem::text::Style::Italic)
                })
            },
        )
        .style(move |s| {
            let visible = inlay_hints_toggle.get() && {
                let cur_line = active_cursor
                    .get()
                    .map(|(_, l, _)| l.saturating_sub(1))
                    .unwrap_or(0);
                inlay_hints.get().iter().any(|h| h.line == cur_line)
            };
            s.flex_row()
                .width_full()
                .background(floem::peniko::Color::from_rgba8(80, 80, 160, 25))
                .border_bottom(1.0)
                .apply_if(!visible, |s| s.display(floem::style::Display::None))
        })
    };

    stack((
        tab_bar,
        breadcrumbs,
        sticky_bar,
        code_lens_bar,
        inlay_bar,
        find_bar,
        editor_row,
        ghost_strip,
        goto_overlay,
    ))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let bg = if t.is_cosmic() { p.glass_bg } else { p.bg_deep };
        s.flex_col()
            .flex_grow(1.0)
            .min_width(0.0)
            .height_full()
            .background(bg)
    })
    .on_event_stop(EventListener::KeyDown, move |event| {
        if let Event::KeyDown(e) = event {
            let ctrl = e.modifiers.contains(Modifiers::CONTROL);
            let shift = e.modifiers.contains(Modifiers::SHIFT);
            let alt = e.modifiers.contains(Modifiers::ALT);

            // Alt+Up — move line up
            if alt && !ctrl && !shift {
                if let Key::Named(floem::keyboard::NamedKey::ArrowUp) = e.key.logical_key {
                    move_line_up_nonce.update(|n| *n += 1);
                    return;
                }
                if let Key::Named(floem::keyboard::NamedKey::ArrowDown) = e.key.logical_key {
                    move_line_down_nonce.update(|n| *n += 1);
                    return;
                }
            }
            // Alt+Shift+Down — duplicate line
            if alt && shift && !ctrl {
                if let Key::Named(floem::keyboard::NamedKey::ArrowDown) = e.key.logical_key {
                    duplicate_line_nonce.update(|n| *n += 1);
                    return;
                }
            }

            if ctrl && !shift {
                if let Key::Character(ref ch) = e.key.logical_key {
                    match ch.as_str() {
                        "s" => save_fn_key(),
                        "=" | "+" => font_size.update(|s| *s = (*s + 1).min(32)),
                        "-" => font_size.update(|s| {
                            if *s > 8 {
                                *s -= 1;
                            }
                        }),
                        "0" => font_size.set(14),
                        "f" => {
                            find_open.set(true);
                            replace_open.set(false);
                            find_cur_match.set(0);
                        }
                        "h" => {
                            find_open.set(true);
                            replace_open.set(true);
                            find_cur_match.set(0);
                        }
                        "g" => {
                            goto_open.set(true);
                            goto_query.set(String::new());
                        }
                        _ => {}
                    }
                }
            }
        }
    })
}

// ── Tab bar ───────────────────────────────────────────────────────────────────

fn tab_bar_view(
    tabs: RwSignal<Vec<TabState>>,
    active_idx: RwSignal<Option<usize>>,
    theme: RwSignal<PhazeTheme>,
    _save_fn: Rc<dyn Fn()>,
    diagnostics: RwSignal<Vec<crate::lsp_bridge::DiagEntry>>,
) -> impl IntoView {
    let tab_list = dyn_stack(
        move || tabs.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(i, tab)| {
            let is_active = active_idx.get() == Some(i);
            let is_hovered = create_rw_signal(false);
            let name = tab.name.clone();
            let tab_name_for_close = tab.name.clone();
            let dirty = tab.dirty;
            let tab_path = tab.path.clone();

            let diag_color = move || -> Option<floem::peniko::Color> {
                let p = theme.get().palette;
                let diags = diagnostics.get();
                let has_err = diags.iter().any(|d| {
                    d.path == tab_path && d.severity == crate::lsp_bridge::DiagSeverity::Error
                });
                let has_warn = diags.iter().any(|d| {
                    d.path == tab_path && d.severity == crate::lsp_bridge::DiagSeverity::Warning
                });
                if has_err {
                    Some(p.error)
                } else if has_warn {
                    Some(p.warning)
                } else {
                    None
                }
            };

            container(
                stack((
                    container(label(|| "●")).style(move |s| {
                        s.font_size(8.0)
                            .color(theme.get().palette.accent)
                            .margin_right(4.0)
                            .apply_if(!safe_get(dirty, false), |s| s.display(floem::style::Display::None))
                    }),
                    container(label(|| "●")).style(move |s| {
                        let color = diag_color();
                        s.font_size(8.0)
                            .margin_right(4.0)
                            .apply_if(color.is_none(), |s| s.display(floem::style::Display::None))
                            .apply_if(color.is_some(), move |s| {
                                s.color(color.unwrap_or(floem::peniko::Color::TRANSPARENT))
                            })
                    }),
                    label(move || name.clone()).style(move |s| {
                        let t = theme.get();
                        s.font_size(13.0).color(if is_active {
                            t.palette.text_primary
                        } else {
                            t.palette.text_muted
                        })
                    }),
                    phaze_icon(icons::CLOSE, 10.0, move |p| p.text_muted, theme)
                        .style(move |s: floem::style::Style| {
                            s.margin_left(8.0)
                                .width(16.0)
                                .height(16.0)
                                .items_center()
                                .justify_center()
                                .border_radius(3.0)
                                .cursor(floem::style::CursorStyle::Pointer)
                                .hover(|s| s.background(theme.get().palette.bg_elevated))
                        })
                        .on_click_stop(move |_| {
                            // Check if the tab has unsaved changes before closing.
                            if dirty.get_untracked() {
                                let confirmed = rfd::MessageDialog::new()
                                    .set_title("Unsaved Changes")
                                    .set_description(format!(
                                        "\"{}\" has unsaved changes. Close without saving?",
                                        tab_name_for_close
                                    ))
                                    .set_buttons(rfd::MessageButtons::YesNo)
                                    .show();
                                if confirmed != rfd::MessageDialogResult::Yes {
                                    return;
                                }
                            }
                            tabs.update(|list| {
                                if i < list.len() {
                                    list.remove(i);
                                }
                            });
                            active_idx.update(|cur| {
                                let len = tabs.get().len();
                                if len == 0 {
                                    *cur = None;
                                } else {
                                    *cur = Some(cur.unwrap_or(0).min(len - 1));
                                }
                            });
                        }),
                ))
                .style(|s| s.items_center()),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let bg = if is_active {
                    p.bg_surface
                } else if is_hovered.get() {
                    p.bg_elevated
                } else {
                    p.bg_panel
                };
                s.height_full()
                    .padding_horiz(16.0)
                    .background(bg)
                    .border_right(1.0)
                    .border_color(p.border)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .min_width(0.0)
                    .apply_if(is_active, |s| {
                        s.border_top(1.0)
                            .border_color(p.accent)
                            .box_shadow_v_offset(-1.0)
                            .box_shadow_blur(12.0)
                            .box_shadow_color(p.glow)
                            .border_bottom(2.0)
                            .border_color(p.accent)
                    })
                    .items_center()
            })
            .on_click_stop(move |_| active_idx.set(Some(i)))
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                is_hovered.set(true)
            })
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
                is_hovered.set(false)
            })
        },
    )
    .style(|s| s.flex_row().min_width(0.0));

    // Tab overflow dropdown — shows a list of all open tabs when clicked
    let dropdown_open = create_rw_signal(false);
    let dropdown_hov = create_rw_signal(false);

    let tab_dropdown_btn = container(label(move || {
        let count = tabs.get().len();
        if count > 1 { format!("⋯ {count}") } else { "⋯".to_string() }
    }).style(move |s| {
        let p = theme.get().palette;
        s.font_size(11.0)
            .font_weight(floem::text::Weight::BOLD)
            .color(if dropdown_hov.get() { p.accent_hover } else { p.text_muted })
    }))
    .style(move |s| {
        let p = theme.get().palette;
        s.padding_horiz(8.0)
            .height_full()
            .items_center()
            .cursor(floem::style::CursorStyle::Pointer)
            .border_left(1.0)
            .border_color(p.border)
            .background(if dropdown_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| dropdown_open.update(|v| *v = !*v))
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| dropdown_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| dropdown_hov.set(false));

    // Dropdown panel listing all tabs
    let tab_dropdown_list = container(
        scroll(
            dyn_stack(
                move || tabs.get().into_iter().enumerate().collect::<Vec<_>>(),
                |(i, _)| *i,
                move |(i, tab)| {
                    let is_active = active_idx.get() == Some(i);
                    let name = tab.name.clone();
                    let dirty = tab.dirty;
                    let item_hov = create_rw_signal(false);
                    stack((
                        container(label(|| "●")).style(move |s| {
                            s.font_size(6.0)
                                .color(theme.get().palette.accent)
                                .margin_right(6.0)
                                .apply_if(!safe_get(dirty, false), |s| s.display(floem::style::Display::None))
                        }),
                        label(move || name.clone()).style(move |s| {
                            let p = theme.get().palette;
                            s.font_size(12.0).color(if is_active { p.accent } else { p.text_primary })
                        }),
                    ))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.items_center()
                            .width_full()
                            .padding_horiz(10.0)
                            .padding_vert(4.0)
                            .cursor(floem::style::CursorStyle::Pointer)
                            .background(if is_active {
                                p.accent_dim
                            } else if item_hov.get() {
                                p.bg_elevated
                            } else {
                                floem::peniko::Color::TRANSPARENT
                            })
                    })
                    .on_click_stop(move |_| {
                        active_idx.set(Some(i));
                        dropdown_open.set(false);
                    })
                    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| item_hov.set(true))
                    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| item_hov.set(false))
                },
            )
            .style(|s| s.flex_col().width_full()),
        )
        .style(|s| s.max_height(300.0).width_full()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.absolute()
            .inset_top(35.0)
            .inset_right(0.0)
            .width(240.0)
            .background(p.bg_panel)
            .border(1.0)
            .border_color(p.glass_border)
            .border_radius(6.0)
            .box_shadow_blur(16.0)
            .box_shadow_color(p.glow)
            .z_index(50)
            .flex_col()
            .apply_if(!dropdown_open.get(), |s| s.display(floem::style::Display::None))
    });

    // Wrap the tab list in scroll so overflow tabs are scrollable horizontally
    let tab_scroll = scroll(tab_list).style(move |s: floem::style::Style| {
        s.height_full()
            .flex_grow(1.0)
            .min_width(0.0)
    });

    container(
        stack((tab_scroll, tab_dropdown_btn, tab_dropdown_list))
            .style(|s| s.flex_row().width_full().height_full().position(floem::style::Position::Relative)),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let bar_bg = if t.is_cosmic() { p.glass_bg } else { p.bg_deep };
        s.height(35.0)
            .width_full()
            .background(bar_bg)
            .border_bottom(1.0)
            .border_color(p.border)
            .min_width(0.0)
    })
}

// ── Tab name disambiguation ───────────────────────────────────────────────────

/// For any tabs that share the same filename, update their `name` field to include
/// one parent directory segment: e.g. `"src/main.rs"` instead of `"main.rs"`.
/// Tabs with unique filenames keep their plain filename.
fn disambiguate_tab_names(list: &mut [TabState]) {
    // Count how many tabs share each base filename.
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for tab in list.iter() {
        let base = tab
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        *counts.entry(base).or_insert(0) += 1;
    }

    for tab in list.iter_mut() {
        let base = tab
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if counts.get(&base).copied().unwrap_or(0) > 1 {
            // Include one parent directory segment.
            let parent = tab
                .path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string());
            tab.name = if let Some(parent_name) = parent {
                format!("{}/{}", parent_name, base)
            } else {
                base
            };
        } else {
            tab.name = base;
        }
    }
}

// ── .editorconfig support ─────────────────────────────────────────────────────

#[derive(Default)]
pub struct EditorConfigSettings {
    pub indent_size: Option<u32>,
    pub use_tabs: Option<bool>,
    pub end_of_line: Option<&'static str>,
}

/// Walk up from `file_path`'s parent toward `workspace_root`, reading `.editorconfig`
/// files (innermost wins for each key).  Parses `[*]` and extension-specific sections.
pub fn read_editorconfig(
    file_path: &std::path::Path,
    workspace_root: &std::path::Path,
) -> EditorConfigSettings {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();

    let mut indent_size: Option<u32> = None;
    let mut use_tabs: Option<bool> = None;
    let mut end_of_line: Option<&'static str> = None;

    // Collect directories from file's parent up to workspace_root (inclusive).
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    let mut cur = file_path.parent().map(|p| p.to_path_buf());
    while let Some(dir) = cur {
        dirs.push(dir.clone());
        if dir == workspace_root {
            break;
        }
        // Stop at filesystem root
        let parent = dir.parent().map(|p| p.to_path_buf());
        if parent.as_deref() == Some(&dir) {
            break;
        }
        cur = parent;
    }

    // Process from outermost (workspace_root) to innermost (file's dir) so inner wins.
    dirs.reverse();

    for dir in &dirs {
        let ec_path = dir.join(".editorconfig");
        let Ok(content) = std::fs::read_to_string(&ec_path) else {
            continue;
        };

        let mut in_matching_section = false;
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let glob = &line[1..line.len() - 1];
                // Match [*], [*.ext], [*.{ext1,ext2}], or [**]
                in_matching_section = glob == "*"
                    || glob == "**"
                    || glob == format!("*.{ext}")
                    || glob.starts_with("*.{")
                        && glob.ends_with('}')
                        && glob[3..glob.len() - 1].split(',').any(|e| e.trim() == ext);
                continue;
            }
            if !in_matching_section {
                continue;
            }
            // Parse key = value
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim().to_lowercase();
                let val = line[eq + 1..].trim().to_lowercase();
                match key.as_str() {
                    "indent_style" => {
                        use_tabs = Some(val == "tab");
                    }
                    "indent_size" | "tab_width" => {
                        if let Ok(n) = val.parse::<u32>() {
                            indent_size = Some(n);
                        }
                    }
                    "end_of_line" => {
                        end_of_line = match val.as_str() {
                            "crlf" => Some("CRLF"),
                            "lf" => Some("LF"),
                            "cr" => Some("LF"), // treat CR as LF for display
                            _ => None,
                        };
                    }
                    _ => {}
                }
            }
        }
    }

    EditorConfigSettings {
        indent_size,
        use_tabs,
        end_of_line,
    }
}
