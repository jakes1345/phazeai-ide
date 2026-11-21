//! UTF-8–safe string truncation helpers.
//!
//! Slicing a `&str` at an arbitrary byte index panics when it lands inside a
//! multi-byte character (any non-ASCII source file, emoji in a diff, etc.).

use std::borrow::Cow;

/// `s` itself if it is at most `max` bytes long; otherwise its start and its
/// end, at most `max` bytes of them together, around a line saying how many
/// bytes were left out. For command output: what was asked for comes first,
/// errors and totals usually come last. Both cuts fall on char boundaries.
pub fn cut_middle(s: &str, max: usize) -> Cow<'_, str> {
    if s.len() <= max {
        return Cow::Borrowed(s);
    }
    let head = truncate_bytes(s, max / 2);
    let rest = &s[head.len()..];
    let tail = tail_bytes(rest, max - head.len());
    let omitted = rest.len() - tail.len();
    Cow::Owned(format!("{head}\n[... {omitted} bytes omitted ...]\n{tail}"))
}

/// The longest prefix of `s` that is at most `max` bytes and ends on a char boundary.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The longest suffix of `s` that is at most `max` bytes and starts on a char boundary.
pub fn tail_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// A run of this many identical lines or more is shown as its first line and
/// a count.
const REPEAT_RUN: usize = 3;

/// Command output as a reader of the finished terminal would see it, without
/// what only a live terminal needs:
/// - escape sequences (colours, cursor moves, window titles, hyperlinks) are
///   removed, and so are other control characters but tab and newline;
/// - a line redrawn with carriage returns (progress bars, spinners, `\r\n`
///   line ends) keeps only what was drawn last;
/// - a backspace erases the character before it;
/// - a run of [`REPEAT_RUN`] or more identical lines becomes the line once
///   and a note with the count.
///
/// Text with none of these comes back borrowed and unchanged.
pub fn clean_terminal_output(s: &str) -> Cow<'_, str> {
    let noisy = s
        .bytes()
        .any(|b| b == 0x1b || b == b'\r' || b == 0x08 || (b < 0x20 && b != b'\n' && b != b'\t'));
    let lines: Vec<Cow<'_, str>> = if noisy {
        strip_escapes(s)
            .split('\n')
            .map(|line| Cow::Owned(redraw(line)))
            .collect()
    } else {
        s.split('\n').map(Cow::Borrowed).collect()
    };
    let repeats = lines
        .windows(REPEAT_RUN)
        .any(|w| w.iter().all(|l| *l == w[0]));
    if !noisy && !repeats {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < lines.len() {
        let run = lines[i..].iter().take_while(|l| **l == lines[i]).count();
        // A trailing newline splits off one empty last piece; keep it as is.
        let last = i + run == lines.len();
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&lines[i]);
        if run >= REPEAT_RUN && !(last && lines[i].is_empty()) {
            let more = run - 1;
            out.push_str(&format!(
                "\n[previous line repeated {more} more time{}]",
                if more == 1 { "" } else { "s" }
            ));
            i += run;
        } else {
            i += 1;
        }
    }
    Cow::Owned(out)
}

/// `s` without escape sequences: CSI (`ESC [` … a final byte), OSC (`ESC ]`
/// … BEL or `ESC \\`), DCS, SOS, PM and APC strings (… `ESC \\`), and the
/// two-byte escapes; and without control characters other than tab,
/// newline, carriage return and backspace, which [`redraw`] handles.
fn strip_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    // Parameters and intermediates, then one final byte.
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    // A string, ended by BEL or ST (`ESC \\`).
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // Character set selection takes one more byte.
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | '%') => {
                    chars.next();
                }
                _ => {}
            },
            // The 8-bit CSI.
            '\u{9b}' => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            '\n' | '\t' | '\r' | '\u{8}' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// One line as the terminal leaves it: text after the last carriage return
/// that drew something (a bare `\r` before the line end draws nothing), with
/// backspaces applied.
fn redraw(line: &str) -> String {
    let drawn = line
        .split('\r')
        .rev()
        .find(|part| !part.is_empty())
        .unwrap_or("");
    let mut out = String::with_capacity(drawn.len());
    for c in drawn.chars() {
        if c == '\u{8}' {
            out.pop();
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_cut_exactly() {
        assert_eq!(truncate_bytes("hello world", 5), "hello");
        assert_eq!(tail_bytes("hello world", 5), "world");
    }

    #[test]
    fn short_strings_are_untouched() {
        assert_eq!(truncate_bytes("hi", 10), "hi");
        assert_eq!(tail_bytes("hi", 10), "hi");
    }

    #[test]
    fn never_splits_a_multibyte_char() {
        let s = "aé😀b"; // 1 + 2 + 4 + 1 bytes
        for max in 0..=s.len() {
            let head = truncate_bytes(s, max);
            let tail = tail_bytes(s, max);
            assert!(head.len() <= max && tail.len() <= max);
            assert!(s.starts_with(head) && s.ends_with(tail));
        }
        assert_eq!(truncate_bytes(s, 2), "a"); // 2 would split 'é'
        assert_eq!(tail_bytes(s, 2), "b"); // would start inside '😀'
    }

    /// `cut` split back into (head, bytes omitted, tail).
    fn parts(cut: &str) -> (&str, usize, &str) {
        let (head, rest) = cut.split_once("\n[... ").expect("no marker");
        let (omitted, tail) = rest
            .split_once(" bytes omitted ...]\n")
            .expect("no marker end");
        (head, omitted.parse().unwrap(), tail)
    }

    #[test]
    fn cut_middle_leaves_short_strings_alone() {
        assert!(matches!(cut_middle("hello", 5), Cow::Borrowed("hello")));
        assert!(matches!(cut_middle("", 0), Cow::Borrowed("")));
    }

    #[test]
    fn cut_middle_keeps_the_start_and_the_end_and_counts_what_it_left_out() {
        assert_eq!(
            cut_middle("0123456789", 4),
            "01\n[... 6 bytes omitted ...]\n89"
        );
        assert_eq!(
            cut_middle("0123456789", 0),
            "\n[... 10 bytes omitted ...]\n"
        );
    }

    #[test]
    fn cut_middle_never_splits_a_character() {
        // Every char width, at every cut position.
        let s = "a✓é😀b✓✓é😀😀é✓z".repeat(3);
        for max in 0..s.len() {
            let cut = cut_middle(&s, max);
            let (head, omitted, tail) = parts(&cut);
            assert!(s.starts_with(head) && s.ends_with(tail), "max {max}");
            assert!(head.len() + tail.len() <= max, "max {max}");
            assert_eq!(head.len() + omitted + tail.len(), s.len(), "max {max}");
            // At most one character's worth of the budget goes unused.
            assert!(head.len() + tail.len() + 4 > max, "max {max}");
        }
    }

    #[test]
    fn plain_output_is_borrowed_unchanged() {
        let text = "a\n\tb\nc\n\nd\n";
        assert!(matches!(clean_terminal_output(text), Cow::Borrowed(t) if t == text));
        // Two identical lines are not a run.
        let twice = "same\nsame\nother";
        assert!(matches!(clean_terminal_output(twice), Cow::Borrowed(_)));
    }

    #[test]
    fn colours_cursor_moves_titles_and_links_go() {
        let raw = "\u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m foo v0.1.0\n\
                   \u{1b}]0;cargo build\u{7}\u{1b}[2K\u{1b}[1Gdone\n\
                   \u{1b}]8;;https://example.com\u{1b}\\link\u{1b}]8;;\u{1b}\\ text\n\
                   \u{1b}(Bplain\u{1b}=\u{1b}[?25l\u{1b}[38;2;10;20;30mrgb\u{1b}[m\u{7}";
        assert_eq!(
            clean_terminal_output(raw),
            "   Compiling foo v0.1.0\ndone\nlink text\nplainrgb"
        );
    }

    #[test]
    fn a_redrawn_line_keeps_what_was_drawn_last() {
        let progress = "Downloading\r 10%\r 55%\r100%\nok\r\nnext\r\n";
        assert_eq!(clean_terminal_output(progress), "100%\nok\nnext\n");
        assert_eq!(clean_terminal_output("ab\u{8}c"), "ac");
        assert_eq!(clean_terminal_output("x\u{8}\u{8}y"), "y");
        // Non-ASCII text survives the rewrite.
        assert_eq!(clean_terminal_output("\u{1b}[31mé✓\u{1b}[0m"), "é✓");
    }

    #[test]
    fn a_run_of_identical_lines_becomes_one_with_a_count() {
        let text = "start\nwarning: x\nwarning: x\nwarning: x\nwarning: x\nend\n";
        assert_eq!(
            clean_terminal_output(text),
            "start\nwarning: x\n[previous line repeated 3 more times]\nend\n"
        );
        let blanks = "a\n\n\n\n\nb";
        assert_eq!(
            clean_terminal_output(blanks),
            "a\n\n[previous line repeated 3 more times]\nb"
        );
        // Redraws that leave the same line each time count as a run too.
        let spinner = "\u{1b}[2Kworking\r\n\u{1b}[2Kworking\r\n\u{1b}[2Kworking\r\nfinished";
        assert_eq!(
            clean_terminal_output(spinner),
            "working\n[previous line repeated 2 more times]\nfinished"
        );
    }
}
