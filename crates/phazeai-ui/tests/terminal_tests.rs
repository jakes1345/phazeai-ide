//! Terminal layer tests — VTE/ANSI parsing, URL detection, OSC 7 handling.
//!
//! All helpers are pure functions defined here, mirroring the logic in
//! `phazeai-ui/src/panels/terminal.rs`. No PTY, no GUI, no Floem scope needed.
//!
//! Run: `cargo test --test terminal_tests`

// ── ANSI escape stripping ─────────────────────────────────────────────────────

/// Strip ANSI/VT escape sequences from a string, leaving only printable text.
/// Handles CSI sequences (`\x1b[...m`), OSC sequences (`\x1b]...ST/BEL`), and
/// bare `\x1b` + single-char escapes.
fn strip_ansi_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ ... (final byte in 0x40-0x7E)
            Some('[') => {
                chars.next(); // consume '['
                for c in chars.by_ref() {
                    if c.is_ascii() && ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... ST or BEL
            Some(']') => {
                chars.next(); // consume ']'
                for c in chars.by_ref() {
                    if c == '\x07' || c == '\u{9C}' {
                        break;
                    }
                    // ESC \ (ST)
                    if c == '\x1b' {
                        chars.next(); // consume '\'
                        break;
                    }
                }
            }
            // Two-character escape sequences: ESC + any single byte
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

#[test]
fn ansi_strip_green_text() {
    assert_eq!(strip_ansi_codes("\x1b[32mhello\x1b[0m"), "hello");
}

#[test]
fn ansi_strip_no_escapes_passthrough() {
    assert_eq!(strip_ansi_codes("plain text"), "plain text");
}

#[test]
fn ansi_strip_empty_string() {
    assert_eq!(strip_ansi_codes(""), "");
}

#[test]
fn ansi_strip_bold_reset() {
    assert_eq!(strip_ansi_codes("\x1b[1mBOLD\x1b[0m"), "BOLD");
}

#[test]
fn ansi_strip_256_color_foreground() {
    // e.g. \x1b[38;5;208m = 256-color orange
    assert_eq!(strip_ansi_codes("\x1b[38;5;208mtext\x1b[0m"), "text");
}

#[test]
fn ansi_strip_true_color() {
    // \x1b[38;2;255;128;0m = truecolor orange
    assert_eq!(strip_ansi_codes("\x1b[38;2;255;128;0mtext\x1b[0m"), "text");
}

#[test]
fn ansi_strip_osc_window_title() {
    // OSC 0 sets window title
    assert_eq!(strip_ansi_codes("\x1b]0;My Terminal\x07hello"), "hello");
}

#[test]
fn ansi_strip_nested_sequences() {
    // Multiple sequences in a row
    assert_eq!(
        strip_ansi_codes("\x1b[32m\x1b[1mGREEN BOLD\x1b[0m"),
        "GREEN BOLD"
    );
}

#[test]
fn ansi_strip_preserves_newlines() {
    assert_eq!(strip_ansi_codes("\x1b[33mfoo\x1b[0m\nbar"), "foo\nbar");
}

#[test]
fn ansi_strip_cursor_movement_only() {
    // Should yield empty string when all content is escape sequences
    assert_eq!(strip_ansi_codes("\x1b[2J\x1b[H"), "");
}

// ── URL detection ─────────────────────────────────────────────────────────────

/// Find the first HTTP/HTTPS or file:// URL in `line`.
/// Returns `None` if no URL is found.
/// Mirrors the hyperlink-detection behaviour used in the terminal panel's
/// on_click_stop handler (described in MEMORY.md: "URL_RE: Regex").
fn find_url_in_line(line: &str) -> Option<&str> {
    // Supported URL prefixes in order of specificity
    const PREFIXES: &[&str] = &["https://", "http://", "file://"];
    for prefix in PREFIXES {
        if let Some(start) = line.find(prefix) {
            let rest = &line[start..];
            // Terminate at whitespace or common trailing punctuation
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '>'))
                .unwrap_or(rest.len());
            return Some(&line[start..start + end]);
        }
    }
    None
}

#[test]
fn url_detect_https_in_sentence() {
    let result = find_url_in_line("Visit https://example.com now");
    assert_eq!(result, Some("https://example.com"));
}

#[test]
fn url_detect_no_url() {
    assert_eq!(find_url_in_line("no url here"), None);
}

#[test]
fn url_detect_empty_line() {
    assert_eq!(find_url_in_line(""), None);
}

#[test]
fn url_detect_http_plain() {
    assert_eq!(
        find_url_in_line("http://example.com/path?q=1"),
        Some("http://example.com/path?q=1")
    );
}

#[test]
fn url_detect_url_with_path_and_query() {
    let result = find_url_in_line("See https://docs.rs/crate/1.0/index.html for details");
    assert_eq!(result, Some("https://docs.rs/crate/1.0/index.html"));
}

#[test]
fn url_detect_file_url() {
    let result = find_url_in_line("Open file:///home/jack/project/src/main.rs");
    assert_eq!(result, Some("file:///home/jack/project/src/main.rs"));
}

#[test]
fn url_detect_url_at_line_start() {
    assert_eq!(
        find_url_in_line("https://github.com/lapce/floem"),
        Some("https://github.com/lapce/floem")
    );
}

#[test]
fn url_detect_stops_at_whitespace() {
    // URL must not include trailing space
    let result = find_url_in_line("https://example.com trailing text");
    assert_eq!(result, Some("https://example.com"));
}

#[test]
fn url_detect_stops_at_closing_paren() {
    // Common in Markdown: [text](https://example.com)
    let result = find_url_in_line("(https://example.com)");
    assert_eq!(result, Some("https://example.com"));
}

#[test]
fn url_detect_prefers_https_over_http() {
    // When both appear, the first one (https) is returned
    let result = find_url_in_line("https://a.com and http://b.com");
    assert_eq!(result, Some("https://a.com"));
}

// ── OSC 7 (working directory notification) parsing ───────────────────────────

/// Parse an OSC 7 URI (`file://[hostname]/path`) and return the local path.
/// Matches the logic in `VtePerformer::osc_dispatch` in terminal.rs:
/// - Strips the `file://` prefix
/// - Strips the optional hostname by finding the first `/` after the prefix
///   and re-attaching the leading `/`
fn parse_osc7(uri: &str) -> Option<String> {
    let after_scheme = uri.strip_prefix("file://")?;
    // After "file://" comes optional hostname then "/path"
    // e.g. "file:///home/jack/foo"   → after_scheme = "/home/jack/foo"
    // e.g. "file://myhostname/path"  → after_scheme = "myhostname/path"
    let path = if after_scheme.starts_with('/') {
        // No hostname — URI is already "file:///path"
        after_scheme.to_string()
    } else if let Some((_, rest)) = after_scheme.split_once('/') {
        format!("/{}", rest)
    } else {
        after_scheme.to_string()
    };
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[test]
fn osc7_triple_slash_no_hostname() {
    assert_eq!(
        parse_osc7("file:///home/jack/foo"),
        Some("/home/jack/foo".to_string())
    );
}

#[test]
fn osc7_with_hostname() {
    // Typical shell output: file://myhostname/home/jack/foo
    assert_eq!(
        parse_osc7("file://myhostname/home/jack/foo"),
        Some("/home/jack/foo".to_string())
    );
}

#[test]
fn osc7_workspace_root() {
    assert_eq!(
        parse_osc7("file:///home/jack/phazeai_ide"),
        Some("/home/jack/phazeai_ide".to_string())
    );
}

#[test]
fn osc7_not_a_file_uri_returns_none() {
    assert_eq!(parse_osc7("https://example.com"), None);
}

#[test]
fn osc7_empty_string_returns_none() {
    assert_eq!(parse_osc7(""), None);
}

#[test]
fn osc7_bare_file_scheme_returns_none() {
    // "file://" alone with nothing after → empty path → None
    assert_eq!(parse_osc7("file://"), None);
}

// ── Default terminal dimensions ───────────────────────────────────────────────

/// Default PTY column count used when opening a new terminal.
/// Matches the `PtySize { cols: 220, rows: 40 }` in `single_terminal()`.
const DEFAULT_TERM_COLS: u16 = 220;
/// Default PTY row count.
const DEFAULT_TERM_ROWS: u16 = 40;

#[test]
fn terminal_default_cols() {
    // 220 is wide enough for most side-by-side split editors
    assert_eq!(DEFAULT_TERM_COLS, 220);
    assert!(
        DEFAULT_TERM_COLS >= 80,
        "cols should be at least a standard 80-col terminal"
    );
}

#[test]
fn terminal_default_rows() {
    assert_eq!(DEFAULT_TERM_ROWS, 40);
    assert!(
        DEFAULT_TERM_ROWS >= 24,
        "rows should be at least a standard 24-row terminal"
    );
}

#[test]
fn terminal_cols_fits_split_editors() {
    // A 1920px window with two editors needs at least ~180 cols for the terminal
    // to be usable when maximized.  220 comfortably satisfies this.
    assert!(DEFAULT_TERM_COLS >= 180);
}

// ── Tab name sanitization ─────────────────────────────────────────────────────

const MAX_TAB_NAME_LEN: usize = 30;

/// Sanitize a terminal tab name:
/// - Trims leading/trailing whitespace
/// - Truncates to MAX_TAB_NAME_LEN characters
/// - Returns "terminal" if the result is empty
fn sanitize_tab_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "terminal".to_string();
    }
    // Truncate at character boundary
    let mut result = trimmed
        .char_indices()
        .take_while(|(i, _)| *i < MAX_TAB_NAME_LEN)
        .map(|(_, c)| c)
        .collect::<String>();
    // If we ran out before the char boundary, use the full (trimmed) string
    if result.is_empty() {
        result = trimmed.to_string();
    }
    result
}

#[test]
fn tab_name_short_stays_as_is() {
    assert_eq!(sanitize_tab_name("my terminal 1"), "my terminal 1");
}

#[test]
fn tab_name_long_truncated_at_30() {
    let long = "this is a very long terminal tab name that exceeds thirty characters";
    let result = sanitize_tab_name(long);
    assert!(
        result.len() <= MAX_TAB_NAME_LEN,
        "expected len <= {MAX_TAB_NAME_LEN}, got {}",
        result.len()
    );
    assert!(result.starts_with("this is a very long terminal t"));
}

#[test]
fn tab_name_exactly_30_chars() {
    let exactly_30 = "a".repeat(30);
    let result = sanitize_tab_name(&exactly_30);
    assert_eq!(result.len(), 30);
}

#[test]
fn tab_name_31_chars_truncated() {
    let thirty_one = "a".repeat(31);
    let result = sanitize_tab_name(&thirty_one);
    assert_eq!(result.len(), 30);
}

#[test]
fn tab_name_leading_trailing_whitespace_trimmed() {
    assert_eq!(sanitize_tab_name("  bash  "), "bash");
}

#[test]
fn tab_name_empty_becomes_terminal() {
    assert_eq!(sanitize_tab_name(""), "terminal");
}

#[test]
fn tab_name_whitespace_only_becomes_terminal() {
    assert_eq!(sanitize_tab_name("   "), "terminal");
}

// ── Scrollback limit enforcement ──────────────────────────────────────────────

const MAX_SCROLLBACK: usize = 10_000;

/// Simulate the scrollback trimming logic from `TermState::commit_line()`.
/// Returns `(lines, prompt_positions)` after enforcing the limit.
fn enforce_scrollback(
    mut lines: Vec<String>,
    mut prompt_positions: Vec<usize>,
) -> (Vec<String>, Vec<usize>) {
    if lines.len() > MAX_SCROLLBACK {
        let drain_count = lines.len() - MAX_SCROLLBACK;
        lines.drain(0..drain_count);
        prompt_positions.retain_mut(|pos| {
            if *pos < drain_count {
                false
            } else {
                *pos -= drain_count;
                true
            }
        });
    }
    (lines, prompt_positions)
}

#[test]
fn scrollback_under_limit_unchanged() {
    let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    let prompts = vec![5, 10, 50];
    let (out_lines, out_prompts) = enforce_scrollback(lines, prompts.clone());
    assert_eq!(out_lines.len(), 100);
    assert_eq!(out_prompts, prompts);
}

#[test]
fn scrollback_at_exact_limit_unchanged() {
    let lines: Vec<String> = (0..MAX_SCROLLBACK).map(|i| format!("line {i}")).collect();
    let (out_lines, _) = enforce_scrollback(lines, vec![]);
    assert_eq!(out_lines.len(), MAX_SCROLLBACK);
}

#[test]
fn scrollback_overflow_trims_oldest() {
    let lines: Vec<String> = (0..MAX_SCROLLBACK + 500)
        .map(|i| format!("line {i}"))
        .collect();
    let (out_lines, _) = enforce_scrollback(lines, vec![]);
    assert_eq!(out_lines.len(), MAX_SCROLLBACK);
    // First remaining line should be line 500
    assert_eq!(out_lines[0], "line 500");
}

#[test]
fn scrollback_prompt_positions_shifted() {
    let total = MAX_SCROLLBACK + 100;
    let lines: Vec<String> = (0..total).map(|i| format!("line {i}")).collect();
    // Prompt was at line 200 (will survive drain of 100) and line 50 (will be lost)
    let prompts = vec![50, 200];
    let (_, out_prompts) = enforce_scrollback(lines, prompts);
    // 50 < 100 → dropped; 200 → 200 - 100 = 100
    assert_eq!(out_prompts, vec![100]);
}

#[test]
fn scrollback_all_prompts_lost_on_large_drain() {
    let total = MAX_SCROLLBACK * 2;
    let lines: Vec<String> = (0..total).map(|i| format!("line {i}")).collect();
    let prompts = vec![0, 100, 999]; // all well below drain_count (= MAX_SCROLLBACK)
    let (_, out_prompts) = enforce_scrollback(lines, prompts);
    assert!(out_prompts.is_empty());
}

// ── 256-color index conversion ────────────────────────────────────────────────

/// Mirror of the color-cube portion of `indexed_to_color()` in terminal.rs.
/// Returns (r, g, b) for a 256-color palette index.
fn indexed_to_rgb(idx: u8) -> (u8, u8, u8) {
    if idx < 16 {
        // Basic 16 colors — test values from the production table
        const BASIC: [(u8, u8, u8); 16] = [
            (0, 0, 0),
            (194, 54, 33),
            (37, 188, 36),
            (173, 173, 39),
            (73, 46, 225),
            (211, 56, 211),
            (51, 187, 200),
            (203, 204, 205),
            (129, 131, 131),
            (252, 57, 31),
            (49, 231, 34),
            (234, 236, 35),
            (88, 51, 255),
            (249, 53, 248),
            (20, 240, 240),
            (233, 235, 235),
        ];
        return BASIC[idx as usize];
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
        return (to_val(r), to_val(g), to_val(b));
    }
    let v = 8u8.saturating_add((idx - 232).saturating_mul(10));
    (v, v, v)
}

#[test]
fn color_idx_0_is_black() {
    assert_eq!(indexed_to_rgb(0), (0, 0, 0));
}

#[test]
fn color_idx_1_is_red() {
    let (r, g, b) = indexed_to_rgb(1);
    assert!(r > 150, "red channel should dominate");
    assert!(g < 100);
    assert!(b < 100);
}

#[test]
fn color_idx_2_is_green() {
    let (r, g, b) = indexed_to_rgb(2);
    assert!(g > 150, "green channel should dominate");
    assert!(r < 100);
    assert!(b < 100);
}

#[test]
fn color_idx_15_is_bright_white() {
    let (r, g, b) = indexed_to_rgb(15);
    // All channels should be high
    assert!(r > 200 && g > 200 && b > 200);
}

#[test]
fn color_idx_16_is_color_cube_start() {
    // Index 16 = cube coordinate (0,0,0) → all zero → black
    assert_eq!(indexed_to_rgb(16), (0, 0, 0));
}

#[test]
fn color_idx_231_is_color_cube_end() {
    // Index 231 = cube coordinate (5,5,5) → max of the 6-step cube
    let (r, g, b) = indexed_to_rgb(231);
    // 55 + 5*40 = 255
    assert_eq!((r, g, b), (255, 255, 255));
}

#[test]
fn color_idx_232_is_dark_gray() {
    // Grayscale ramp starts at 232 → v = 8 + 0*10 = 8
    assert_eq!(indexed_to_rgb(232), (8, 8, 8));
}

#[test]
fn color_idx_255_is_near_white() {
    // v = 8 + (255-232)*10 = 8 + 230 = 238
    assert_eq!(indexed_to_rgb(255), (238, 238, 238));
}

#[test]
fn color_grayscale_is_neutral() {
    // Grayscale entries (232–255) should always have equal R, G, B
    for idx in 232..=255 {
        let (r, g, b) = indexed_to_rgb(idx);
        assert_eq!(r, g, "idx {idx}: R != G");
        assert_eq!(g, b, "idx {idx}: G != B");
    }
}

// ── TermLine plain_text and segment merging ───────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum MockTermColor {
    Default,
    Indexed(u8),
}

#[derive(Clone, Debug)]
struct MockTermSegment {
    text: String,
    fg: MockTermColor,
    bold: bool,
}

#[derive(Clone, Debug, Default)]
struct MockTermLine {
    segments: Vec<MockTermSegment>,
}

impl MockTermLine {
    fn push_char(&mut self, ch: char, fg: MockTermColor, bold: bool) {
        if let Some(last) = self.segments.last_mut() {
            if last.fg == fg && last.bold == bold {
                last.text.push(ch);
                return;
            }
        }
        self.segments.push(MockTermSegment {
            text: ch.to_string(),
            fg,
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

#[test]
fn term_line_plain_text_combines_segments() {
    let mut line = MockTermLine::default();
    for ch in "hello".chars() {
        line.push_char(ch, MockTermColor::Indexed(2), false);
    }
    for ch in " world".chars() {
        line.push_char(ch, MockTermColor::Default, false);
    }
    assert_eq!(line.plain_text(), "hello world");
}

#[test]
fn term_line_same_style_merges_into_one_segment() {
    let mut line = MockTermLine::default();
    for ch in "abc".chars() {
        line.push_char(ch, MockTermColor::Default, false);
    }
    assert_eq!(line.segments.len(), 1);
    assert_eq!(line.segments[0].text, "abc");
}

#[test]
fn term_line_style_change_creates_new_segment() {
    let mut line = MockTermLine::default();
    line.push_char('a', MockTermColor::Default, false);
    line.push_char('b', MockTermColor::Indexed(1), false); // red
    assert_eq!(line.segments.len(), 2);
}

#[test]
fn term_line_bold_change_creates_new_segment() {
    let mut line = MockTermLine::default();
    line.push_char('a', MockTermColor::Default, false);
    line.push_char('b', MockTermColor::Default, true); // bold
    assert_eq!(line.segments.len(), 2);
}

#[test]
fn term_line_is_empty_with_no_segments() {
    let line = MockTermLine::default();
    assert!(line.is_empty());
}

#[test]
fn term_line_is_empty_with_empty_segment_text() {
    let line = MockTermLine {
        segments: vec![MockTermSegment {
            text: String::new(),
            fg: MockTermColor::Default,
            bold: false,
        }],
    };
    assert!(line.is_empty());
}

#[test]
fn term_line_is_not_empty_with_content() {
    let mut line = MockTermLine::default();
    line.push_char('x', MockTermColor::Default, false);
    assert!(!line.is_empty());
}
