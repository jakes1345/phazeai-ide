//! Multi-line prompt editor and persistent prompt history.
//!
//! The cursor is a byte index that is always kept on a char boundary, so
//! editing never panics on multi-byte input (the old single-line editor
//! advanced the cursor by one *byte* per character). Motion, deletion and
//! layout work on grapheme clusters, so "⚠️" and ZWJ emoji are one unit, and
//! widths are measured per cluster as the terminal draws them.

use std::path::PathBuf;

use unicode_segmentation::UnicodeSegmentation;

use crate::text::str_width;

#[derive(Default, Clone, Debug)]
pub struct InputBuffer {
    text: String,
    cursor: usize,
}

impl InputBuffer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Take the contents, leaving the buffer empty.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn insert_char(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        // Normalise Windows / old-Mac line endings from pastes.
        let s = crate::text::sanitize(&s.replace("\r\n", "\n").replace('\r', "\n"));
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    fn prev_boundary(&self, pos: usize) -> usize {
        self.text[..pos]
            .grapheme_indices(true)
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next_boundary(&self, pos: usize) -> usize {
        self.text[pos..]
            .graphemes(true)
            .next()
            .map(|g| pos + g.len())
            .unwrap_or(pos)
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.prev_boundary(self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let end = self.next_boundary(self.cursor);
        self.text.replace_range(self.cursor..end, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.prev_boundary(self.cursor);
    }

    pub fn right(&mut self) {
        self.cursor = self.next_boundary(self.cursor);
    }

    pub fn word_left(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(|c: char| c.is_whitespace());
        let start = trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        self.cursor = start;
    }

    pub fn word_right(&mut self) {
        let after = &self.text[self.cursor..];
        let mut seen_word = false;
        let mut offset = after.len();
        for (i, c) in after.char_indices() {
            if c.is_whitespace() {
                if seen_word {
                    offset = i;
                    break;
                }
            } else {
                seen_word = true;
            }
        }
        self.cursor += offset;
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len())
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    /// Delete from the start of the current line to the cursor (Ctrl+U).
    pub fn kill_to_line_start(&mut self) {
        let start = self.line_start();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Delete from the cursor to the end of the current line (Ctrl+K).
    pub fn kill_to_line_end(&mut self) {
        let end = self.line_end();
        self.text.replace_range(self.cursor..end, "");
    }

    /// Delete the word before the cursor (Ctrl+W / Alt+Backspace).
    pub fn delete_word_back(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.text.replace_range(self.cursor..end, "");
    }

    /// (line index, char column) of the cursor in logical lines.
    fn cursor_line_col(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let line = before.matches('\n').count();
        let col = before[self.line_start()..].chars().count();
        (line, col)
    }

    pub fn line_count(&self) -> usize {
        self.text.matches('\n').count() + 1
    }

    pub fn on_first_line(&self) -> bool {
        !self.text[..self.cursor].contains('\n')
    }

    pub fn on_last_line(&self) -> bool {
        !self.text[self.cursor..].contains('\n')
    }

    fn move_to_line(&mut self, target: usize, col: usize) {
        let mut start = 0;
        for (i, line) in self.text.split('\n').enumerate() {
            if i == target {
                let byte_col = line
                    .char_indices()
                    .nth(col)
                    .map(|(b, _)| b)
                    .unwrap_or(line.len());
                self.cursor = start + byte_col;
                return;
            }
            start += line.len() + 1;
        }
    }

    /// Move the cursor up one logical line. Returns false when already on the
    /// first line (so the caller can fall back to history navigation).
    pub fn up(&mut self) -> bool {
        let (line, col) = self.cursor_line_col();
        if line == 0 {
            return false;
        }
        self.move_to_line(line - 1, col);
        true
    }

    /// Move the cursor down one logical line. Returns false on the last line.
    pub fn down(&mut self) -> bool {
        let (line, col) = self.cursor_line_col();
        if line + 1 >= self.line_count() {
            return false;
        }
        self.move_to_line(line + 1, col);
        true
    }

    /// A copy with every character after the first `keep` chars replaced by
    /// `•` (cursor preserved), for displaying secrets.
    pub fn masked(&self, keep: usize) -> InputBuffer {
        let cursor_chars = self.text[..self.cursor].chars().count();
        let text: String = self
            .text
            .chars()
            .enumerate()
            .map(|(i, c)| if i < keep || c == '\n' { c } else { '•' })
            .collect();
        let cursor = text
            .char_indices()
            .nth(cursor_chars)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        InputBuffer { text, cursor }
    }

    /// Soft-wrap the text into rows of at most `width` cells and locate the
    /// cursor. Returns (rows, cursor_row, cursor_col).
    pub fn layout(&self, width: usize) -> (Vec<String>, usize, usize) {
        let width = width.max(1);
        let mut rows: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cur_w = 0;
        let mut cursor_pos = None;

        for (i, g) in self.text.grapheme_indices(true) {
            // The cursor normally sits on a cluster boundary; should it land
            // inside one (column arithmetic counts chars), draw it there.
            let at_cursor = i <= self.cursor && self.cursor < i + g.len();
            if at_cursor {
                cursor_pos = Some((rows.len(), cur_w));
            }
            if g.ends_with('\n') {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
                continue;
            }
            let gw = str_width(g);
            if cur_w + gw > width {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
                if at_cursor {
                    cursor_pos = Some((rows.len(), 0));
                }
            }
            cur.push_str(g);
            cur_w += gw;
        }
        let (mut row, mut col) = cursor_pos.unwrap_or((rows.len(), cur_w));
        rows.push(cur);
        // A cursor sitting exactly at the right edge goes to the next row.
        if col >= width {
            row += 1;
            col = 0;
            if row >= rows.len() {
                rows.push(String::new());
            }
        }
        (rows, row, col)
    }
}

/// Prompt history with draft preservation and optional on-disk persistence.
#[derive(Default)]
pub struct History {
    entries: Vec<String>,
    /// Index into `entries` while browsing; `None` when editing a fresh line.
    pos: Option<usize>,
    /// What the user was typing before they started browsing.
    draft: String,
    path: Option<PathBuf>,
}

/// Maximum entries kept on disk.
const HISTORY_LIMIT: usize = 1000;

impl History {
    /// Load history from `path` (one JSON string per line, so multi-line
    /// prompts survive). Missing or unreadable files yield empty history.
    pub fn load(path: Option<PathBuf>) -> Self {
        let entries = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| {
                s.lines()
                    .filter_map(|l| serde_json::from_str::<String>(l).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Self {
            entries,
            pos: None,
            draft: String::new(),
            path,
        }
    }

    /// Default location: next to the config file.
    pub fn default_path() -> Option<PathBuf> {
        phazeai_core::Settings::config_path()
            .parent()
            .map(|p| p.join("cli_history"))
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn is_browsing(&self) -> bool {
        self.pos.is_some()
    }

    /// Record a submitted prompt (skips blanks and immediate duplicates).
    pub fn push(&mut self, entry: &str) {
        self.pos = None;
        self.draft.clear();
        if entry.trim().is_empty() || self.entries.last().map(String::as_str) == Some(entry) {
            return;
        }
        self.entries.push(entry.to_string());
        if self.entries.len() > HISTORY_LIMIT {
            let excess = self.entries.len() - HISTORY_LIMIT;
            self.entries.drain(..excess);
        }
        self.persist();
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let body: String = self
            .entries
            .iter()
            .filter_map(|e| serde_json::to_string(e).ok())
            .map(|l| l + "\n")
            .collect();
        if let Err(e) = std::fs::write(path, body) {
            tracing::warn!("failed to save prompt history: {e}");
        }
    }

    /// Step back in history. `current` is the text in the editor, saved as the
    /// draft when browsing starts. Returns the entry to show, if any.
    pub fn older(&mut self, current: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        let pos = match self.pos {
            None => {
                self.draft = current.to_string();
                self.entries.len() - 1
            }
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.pos = Some(pos);
        Some(&self.entries[pos])
    }

    /// Step forward. Returns the next entry, or the saved draft when moving
    /// past the newest entry. `None` when not browsing.
    pub fn newer(&mut self) -> Option<String> {
        let pos = self.pos?;
        if pos + 1 >= self.entries.len() {
            self.pos = None;
            Some(std::mem::take(&mut self.draft))
        } else {
            self.pos = Some(pos + 1);
            Some(self.entries[pos + 1].clone())
        }
    }

    /// Stop browsing (e.g. the user edited the recalled entry).
    pub fn reset(&mut self) {
        self.pos = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(s: &str) -> InputBuffer {
        let mut b = InputBuffer::default();
        b.set(s);
        b
    }

    #[test]
    fn multibyte_editing_never_panics() {
        let mut b = InputBuffer::default();
        for c in "héllo 日本".chars() {
            b.insert_char(c);
        }
        assert_eq!(b.text(), "héllo 日本");
        b.left();
        b.left();
        b.backspace();
        assert_eq!(b.text(), "héllo日本");
        b.right();
        b.delete();
        assert_eq!(b.text(), "héllo日");
        b.home();
        b.right();
        b.right();
        b.insert_char('X');
        assert_eq!(b.text(), "héXllo日");
    }

    #[test]
    fn word_motion_and_deletion() {
        let mut b = buf("cargo test --all");
        b.word_left();
        assert_eq!(&b.text()[b.cursor()..], "--all");
        b.word_left();
        assert_eq!(&b.text()[b.cursor()..], "test --all");
        b.word_right();
        assert_eq!(&b.text()[b.cursor()..], " --all");
        b.end();
        b.delete_word_back();
        assert_eq!(b.text(), "cargo test ");
    }

    #[test]
    fn kill_line_operations_are_line_local() {
        let mut b = buf("first\nsecond line");
        b.word_left();
        b.kill_to_line_start();
        assert_eq!(b.text(), "first\nline");
        b.kill_to_line_end();
        assert_eq!(b.text(), "first\n");
    }

    #[test]
    fn vertical_motion_keeps_column() {
        let mut b = buf("abcdef\nxy\nlonger line");
        assert!(b.on_last_line());
        assert!(b.up());
        assert_eq!(&b.text()[b.cursor()..], "\nlonger line"); // clamped to "xy" end
        assert!(b.up());
        // Column 2 (the end of "xy") carries over to the first line.
        assert_eq!(&b.text()[b.cursor()..], "cdef\nxy\nlonger line");
        assert!(!b.up());
        assert!(b.down());
        assert!(b.down());
        assert!(!b.down());
    }

    #[test]
    fn paste_normalises_line_endings() {
        let mut b = InputBuffer::default();
        b.insert_str("a\r\nb\rc");
        assert_eq!(b.text(), "a\nb\nc");
        assert_eq!(b.line_count(), 3);
    }

    #[test]
    fn layout_wraps_and_finds_cursor() {
        let b = buf("abcdefgh\nij");
        let (rows, r, c) = b.layout(5);
        assert_eq!(rows, vec!["abcde", "fgh", "ij"]);
        assert_eq!((r, c), (2, 2));

        let mut b = buf("abcde");
        let (rows, r, c) = b.layout(5);
        assert_eq!(rows, vec!["abcde", ""]);
        assert_eq!((r, c), (1, 0));

        b.home();
        let (_, r, c) = b.layout(5);
        assert_eq!((r, c), (0, 0));
    }

    #[test]
    fn layout_measures_emoji_sequences_like_the_terminal() {
        // "⚠️" is one 2-cell glyph, so after "⚠️ note" the cursor is at column 7
        // (per-char widths put it at 6, one cell left of where it is drawn).
        let b = buf("⚠\u{FE0F} note");
        let (rows, r, c) = b.layout(20);
        assert_eq!(rows, vec!["⚠\u{FE0F} note"]);
        assert_eq!((r, c), (0, 7));
        // A ZWJ family is 2 cells and never split across rows.
        let family = "👨\u{200D}👩\u{200D}👧";
        let b = buf(&format!("ab{family}"));
        let (rows, r, c) = b.layout(3);
        assert_eq!(rows, vec!["ab".to_string(), family.to_string()]);
        assert_eq!((r, c), (1, 2));
    }

    #[test]
    fn cursor_motion_and_deletion_treat_emoji_sequences_as_one() {
        let family = "👨\u{200D}👩\u{200D}👧";
        let mut b = buf(&format!("{family}x"));
        b.left();
        b.left();
        assert_eq!(b.cursor(), 0);
        b.right();
        assert_eq!(b.cursor(), family.len());
        b.backspace();
        assert_eq!(b.text(), "x");
        let mut b = buf(&format!("{family}x"));
        b.home();
        b.delete();
        assert_eq!(b.text(), "x");
    }

    #[test]
    fn masking_hides_secret_but_keeps_cursor() {
        let mut b = buf("/key sk-abc");
        b.left();
        let m = b.masked(5);
        assert_eq!(m.text(), "/key ••••••");
        assert_eq!(m.text()[m.cursor()..].chars().count(), 1);
    }

    #[test]
    fn history_preserves_draft() {
        let mut h = History::load(None);
        h.push("one");
        h.push("two");
        h.push("two"); // duplicate ignored
        h.push("   "); // blank ignored
        assert_eq!(h.entries(), ["one", "two"]);

        assert_eq!(h.older("draft"), Some("two"));
        assert_eq!(h.older("ignored"), Some("one"));
        assert_eq!(h.older("ignored"), Some("one"));
        assert_eq!(h.newer().as_deref(), Some("two"));
        assert_eq!(h.newer().as_deref(), Some("draft"));
        assert_eq!(h.newer(), None);
    }

    #[test]
    fn history_round_trips_multiline_entries() {
        let dir = std::env::temp_dir().join(format!("phazeai-hist-{}", std::process::id()));
        let path = dir.join("hist");
        let mut h = History::load(Some(path.clone()));
        h.push("line one\nline two");
        h.push("single");
        let h2 = History::load(Some(path));
        assert_eq!(h2.entries(), ["line one\nline two", "single"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
