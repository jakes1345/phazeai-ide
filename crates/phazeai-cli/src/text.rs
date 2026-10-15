//! Width-aware text helpers: display width, safe truncation and word
//! wrapping of styled spans.
//!
//! Everything here measures in terminal *cells* (via `unicode-width`), never
//! in bytes, so multi-byte and wide (CJK / emoji) characters are handled
//! correctly and nothing ever slices a `str` off a char boundary.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Make text from tools, files or the model safe to draw: strips ANSI escape
/// sequences (colours from command output would otherwise corrupt the
/// screen), expands tabs to the next 4-column stop, and drops other control
/// characters (`\r` progress-bar redraws, bells…). Newlines are kept.
pub fn sanitize(s: &str) -> String {
    if !s.chars().any(|c| c.is_control() && c != '\n') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut col = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                out.push('\n');
                col = 0;
            }
            '\t' => {
                let n = 4 - (col % 4);
                out.extend(std::iter::repeat_n(' ', n));
                col += n;
            }
            '\u{1b}' => match chars.peek() {
                // CSI: ESC [ params… final byte in @..~
                Some('[') => {
                    chars.next();
                    for d in chars.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] … terminated by BEL or ESC \
                Some(']') => {
                    chars.next();
                    while let Some(d) = chars.next() {
                        if d == '\u{7}' {
                            break;
                        }
                        if d == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Two-char escapes (ESC c, ESC =, …).
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            c if c.is_control() => {}
            c => {
                out.push(c);
                col += char_width(c);
            }
        }
    }
    out
}

/// Display width of a string in terminal cells.
pub fn str_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Display width of a single char (control chars count as 0).
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Total display width of a slice of spans.
pub fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| str_width(&s.content)).sum()
}

/// Truncate `s` so it fits in `max` cells, appending `…` when anything was cut.
pub fn truncate_to_width(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > max - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// First line of `s`, truncated to `max` cells (with `…` if the text had more
/// lines or was too wide).
pub fn first_line_truncated(s: &str, max: usize) -> String {
    let mut lines = s.lines();
    let first = lines.next().unwrap_or("");
    if lines.next().is_some() && str_width(first) < max {
        format!("{first}…")
    } else {
        truncate_to_width(first, max)
    }
}

/// Truncate styled spans to fit in `max` cells, appending `…` (in the style of
/// the last kept span) when anything was cut.
pub fn truncate_spans(spans: &[Span<'static>], max: usize) -> Vec<Span<'static>> {
    if spans_width(spans) <= max {
        return spans.to_vec();
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut w = 0;
    let budget = max.saturating_sub(1);
    let mut last_style = Style::default();
    'outer: for span in spans {
        let mut buf = String::new();
        last_style = span.style;
        for c in span.content.chars() {
            let cw = char_width(c);
            if w + cw > budget {
                if !buf.is_empty() {
                    out.push(Span::styled(buf, span.style));
                }
                break 'outer;
            }
            buf.push(c);
            w += cw;
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, span.style));
        }
    }
    if max > 0 {
        out.push(Span::styled("…", last_style));
    }
    out
}

/// How [`wrap_spans`] breaks lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapMode {
    /// Break at spaces; only split a word when it is wider than the line.
    /// Leading spaces on continuation lines are dropped.
    Word,
    /// Break at exactly `width` cells, preserving every character
    /// (used for code, where whitespace is significant).
    Char,
}

#[derive(Clone)]
struct Token {
    kind: TokenKind,
    pieces: Vec<(String, Style)>,
    width: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Word,
    Space,
    Newline,
}

fn tokenize(spans: &[Span<'static>], mode: WrapMode) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    for span in spans {
        for c in span.content.chars() {
            let kind = if c == '\n' {
                TokenKind::Newline
            } else if mode == WrapMode::Word && (c == ' ' || c == '\t') {
                TokenKind::Space
            } else {
                TokenKind::Word
            };
            let c = if c == '\t' { ' ' } else { c };
            let needs_new = match tokens.last() {
                None => true,
                Some(t) => t.kind != kind || kind == TokenKind::Newline,
            };
            if needs_new {
                tokens.push(Token {
                    kind,
                    pieces: Vec::new(),
                    width: 0,
                });
            }
            let tok = tokens.last_mut().expect("token just pushed");
            if kind == TokenKind::Newline {
                continue;
            }
            tok.width += char_width(c);
            match tok.pieces.last_mut() {
                Some((text, style)) if *style == span.style => text.push(c),
                _ => tok.pieces.push((c.to_string(), span.style)),
            }
        }
    }
    tokens
}

struct LineBuilder {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    width: usize,
    first_prefix: Vec<Span<'static>>,
    rest_prefix: Vec<Span<'static>>,
    first_avail: usize,
    rest_avail: usize,
}

impl LineBuilder {
    fn avail(&self) -> usize {
        if self.lines.is_empty() {
            self.first_avail
        } else {
            self.rest_avail
        }
    }

    fn push_piece(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        match self.current.last_mut() {
            Some(last) if last.style == style => {
                let mut s = last.content.to_string();
                s.push_str(text);
                last.content = s.into();
            }
            _ => self.current.push(Span::styled(text.to_string(), style)),
        }
    }

    fn push_token(&mut self, tok: &Token) {
        for (text, style) in &tok.pieces {
            self.push_piece(text, *style);
        }
        self.width += tok.width;
    }

    fn flush(&mut self) {
        let prefix = if self.lines.is_empty() {
            self.first_prefix.clone()
        } else {
            self.rest_prefix.clone()
        };
        let mut spans = prefix;
        spans.append(&mut self.current);
        self.lines.push(Line::from(spans));
        self.width = 0;
    }

    /// Push a token that is wider than a full line, splitting it by chars.
    fn push_split(&mut self, tok: &Token) {
        for (text, style) in &tok.pieces {
            for c in text.chars() {
                let cw = char_width(c);
                if self.width + cw > self.avail() && self.width > 0 {
                    self.flush();
                }
                let mut buf = [0u8; 4];
                self.push_piece(c.encode_utf8(&mut buf), *style);
                self.width += cw;
            }
        }
    }
}

/// Wrap styled spans into lines no wider than `width` cells.
///
/// `first_prefix` is prepended to the first output line and `rest_prefix` to
/// every continuation line (hanging indent); prefix widths count towards
/// `width`. A `\n` inside the spans forces a line break. An empty input still
/// yields a single line containing just the first prefix.
pub fn wrap_spans(
    spans: &[Span<'static>],
    width: usize,
    first_prefix: &[Span<'static>],
    rest_prefix: &[Span<'static>],
    mode: WrapMode,
) -> Vec<Line<'static>> {
    let first_avail = width.saturating_sub(spans_width(first_prefix)).max(1);
    let rest_avail = width.saturating_sub(spans_width(rest_prefix)).max(1);
    let mut b = LineBuilder {
        lines: Vec::new(),
        current: Vec::new(),
        width: 0,
        first_prefix: first_prefix.to_vec(),
        rest_prefix: rest_prefix.to_vec(),
        first_avail,
        rest_avail,
    };

    let tokens = tokenize(spans, mode);
    let mut pending_space: Option<Token> = None;

    for tok in &tokens {
        match tok.kind {
            TokenKind::Newline => {
                pending_space = None;
                b.flush();
            }
            TokenKind::Space => {
                if b.width == 0 && !b.lines.is_empty() {
                    // Never start a continuation line with whitespace.
                    continue;
                }
                if b.width == 0 && b.lines.is_empty() {
                    // Leading whitespace of the very first line is kept.
                    b.push_token(tok);
                    continue;
                }
                pending_space = Some(tok.clone());
            }
            TokenKind::Word => {
                let space_w = pending_space.as_ref().map(|t| t.width).unwrap_or(0);
                if b.width + space_w + tok.width <= b.avail() {
                    if let Some(sp) = pending_space.take() {
                        b.push_token(&sp);
                    }
                    b.push_token(tok);
                } else if mode == WrapMode::Char {
                    if let Some(sp) = pending_space.take() {
                        b.push_token(&sp);
                    }
                    b.push_split(tok);
                } else if tok.width <= b.rest_avail {
                    pending_space = None;
                    if b.width > 0 {
                        b.flush();
                    }
                    b.push_token(tok);
                } else {
                    // A single word wider than the line: split it.
                    if let Some(sp) = pending_space.take() {
                        if b.width + sp.width < b.avail() {
                            b.push_token(&sp);
                        } else {
                            b.flush();
                        }
                    }
                    b.push_split(tok);
                }
            }
        }
    }
    if b.width > 0 || !b.current.is_empty() || b.lines.is_empty() {
        b.flush();
    }
    b.lines
}

/// Convenience: wrap plain text in a single style.
pub fn wrap_plain(text: &str, width: usize, style: Style, indent: &str) -> Vec<Line<'static>> {
    let prefix = vec![Span::raw(indent.to_string())];
    wrap_spans(
        &[Span::styled(text.to_string(), style)],
        width,
        &prefix,
        &prefix,
        WrapMode::Word,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn wrap_str(s: &str, width: usize) -> Vec<String> {
        wrap_spans(&[Span::raw(s.to_string())], width, &[], &[], WrapMode::Word)
            .iter()
            .map(text_of)
            .collect()
    }

    #[test]
    fn sanitize_strips_escapes_and_expands_tabs() {
        assert_eq!(sanitize("plain\ntext"), "plain\ntext");
        assert_eq!(sanitize("\u{1b}[31mred\u{1b}[0m ok"), "red ok");
        assert_eq!(sanitize("     1\tone"), "     1  one");
        assert_eq!(sanitize("a\tb\n\tc"), "a   b\n    c");
        assert_eq!(sanitize("50%\r100%\u{7}"), "50%100%");
        assert_eq!(sanitize("\u{1b}]0;title\u{7}after"), "after");
    }

    #[test]
    fn widths_count_cells_not_bytes() {
        assert_eq!(str_width("abc"), 3);
        assert_eq!(str_width("héllo"), 5);
        assert_eq!(str_width("日本"), 4);
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate_to_width("héllo wörld", 6), "héllo…");
        assert_eq!(truncate_to_width("short", 10), "short");
        assert_eq!(truncate_to_width("日本語テキスト", 5), "日本…");
        assert_eq!(truncate_to_width("abc", 0), "");
    }

    #[test]
    fn first_line_marks_hidden_lines() {
        assert_eq!(first_line_truncated("one\ntwo", 10), "one…");
        assert_eq!(first_line_truncated("single", 10), "single");
    }

    #[test]
    fn word_wrap_does_not_break_words() {
        let lines = wrap_str("the quick brown fox jumps", 10);
        assert_eq!(lines, vec!["the quick", "brown fox", "jumps"]);
        for l in &lines {
            assert!(str_width(l) <= 10);
        }
    }

    #[test]
    fn long_words_are_split() {
        let lines = wrap_str("abcdefghijklmnop", 5);
        assert_eq!(lines, vec!["abcde", "fghij", "klmno", "p"]);
    }

    #[test]
    fn newline_forces_break_and_empty_gives_one_line() {
        assert_eq!(wrap_str("a\nb", 10), vec!["a", "b"]);
        assert_eq!(wrap_str("", 10), vec![""]);
    }

    #[test]
    fn hanging_indent_applies_to_continuations() {
        let first = vec![Span::raw("• ")];
        let rest = vec![Span::raw("  ")];
        let lines = wrap_spans(
            &[Span::raw("alpha beta gamma delta")],
            12,
            &first,
            &rest,
            WrapMode::Word,
        );
        let texts: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(texts, vec!["• alpha beta", "  gamma", "  delta"]);
    }

    #[test]
    fn styles_survive_wrapping() {
        let bold = Style::default().add_modifier(ratatui::style::Modifier::BOLD);
        let spans = vec![Span::raw("plain "), Span::styled("bold words here", bold)];
        let lines = wrap_spans(&spans, 11, &[], &[], WrapMode::Word);
        assert_eq!(text_of(&lines[0]), "plain bold");
        assert_eq!(lines[0].spans[1].style, bold);
        assert_eq!(text_of(&lines[1]), "words here");
        assert_eq!(lines[1].spans[0].style, bold);
    }

    #[test]
    fn char_mode_preserves_whitespace() {
        let lines = wrap_spans(&[Span::raw("    let x = 1;")], 8, &[], &[], WrapMode::Char);
        let texts: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(texts, vec!["    let ", "x = 1;"]);
    }

    #[test]
    fn wide_chars_respect_width() {
        let lines = wrap_str("日本語のテキストです", 6);
        for l in &lines {
            assert!(str_width(l) <= 6, "{l:?} too wide");
        }
        assert_eq!(lines.concat(), "日本語のテキストです");
    }

    #[test]
    fn truncate_spans_keeps_styles() {
        let bold = Style::default().add_modifier(ratatui::style::Modifier::BOLD);
        let spans = vec![Span::raw("abc"), Span::styled("defgh", bold)];
        let out = truncate_spans(&spans, 5);
        let s: String = out.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(s, "abcd…");
    }
}
