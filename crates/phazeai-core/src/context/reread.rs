//! A file read again after it changed: only what changed is sent.
//!
//! An agent reads a file, edits it, then often reads it again to check its
//! work. The second read sends the whole file a second time, and both copies
//! then stay in every later request. When the first read is still shown word
//! for word, the lines that differ are enough: the model has the rest above.
//! Claude Code tells its model about a changed file the same way, with the
//! changed lines rather than the file.
//!
//! [`change_note`] compares the two `read_file` results as the model saw
//! them. Each line of a result starts with its line number (`12\tcode`), so
//! the numbers are set aside and the text alone is compared: a line added
//! near the top would otherwise make every later line look changed. The
//! numbers come back on the added lines, so the model knows where they now
//! sit.

use similar::{DiffTag, TextDiff};
use std::fmt::Write as _;

/// The note is sent only when it is at most this share of the full result;
/// a file that changed throughout is sent whole.
const MAX_SHARE: f64 = 0.5;

/// What the model is told instead of `new`, a `read_file` result, when the
/// same read returned `old` in call `earlier_id` and that result is still in
/// the conversation word for word: the lines that differ, as a diff against
/// it. `None` when nothing differs or when the note would not be much
/// shorter than `new` itself.
pub fn change_note(earlier_id: &str, old: &str, new: &str) -> Option<String> {
    let old_lines: Vec<Line> = old.lines().map(Line::parse).collect();
    let new_lines: Vec<Line> = new.lines().map(Line::parse).collect();
    let old_text: Vec<&str> = old_lines.iter().map(|l| l.text).collect();
    let new_text: Vec<&str> = new_lines.iter().map(|l| l.text).collect();
    let diff = TextDiff::configure().diff_slices(&old_text, &new_text);

    let mut body = String::new();
    for group in diff.grouped_ops(0) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_range = first.old_range().start..last.old_range().end;
        let new_range = first.new_range().start..last.new_range().end;
        let _ = writeln!(
            body,
            "@@ -{} +{} @@",
            span(&old_lines, &old_range),
            span(&new_lines, &new_range)
        );
        for op in &group {
            let (tag, old_r, new_r) = op.as_tag_tuple();
            if tag == DiffTag::Equal {
                continue;
            }
            for line in &old_lines[old_r] {
                let _ = writeln!(body, "-{}", line.text);
            }
            for line in &new_lines[new_r] {
                match line.number {
                    Some(n) => {
                        let _ = writeln!(body, "+{n}\t{}", line.text);
                    }
                    None => {
                        let _ = writeln!(body, "+{}", line.text);
                    }
                }
            }
        }
    }
    if body.is_empty() {
        return None;
    }
    let note = format!(
        "Changed since your earlier read {earlier_id}, which is still above; every other \
         line is as shown there. Removed lines start with \"-\", added lines with \"+\" and \
         their line number now.\n{}",
        body.trim_end()
    );
    (note.len() as f64 <= new.len() as f64 * MAX_SHARE).then_some(note)
}

/// One line of a `read_file` result: its line number in the file, when it
/// has one (the closing "[lines ...]" note has none), and its text.
struct Line<'a> {
    number: Option<u64>,
    text: &'a str,
}

impl<'a> Line<'a> {
    fn parse(line: &'a str) -> Self {
        if let Some((number, text)) = line.split_once('\t') {
            if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
                if let Ok(n) = number.parse() {
                    return Line {
                        number: Some(n),
                        text,
                    };
                }
            }
        }
        Line {
            number: None,
            text: line,
        }
    }
}

/// `start,count` of `range` in the file's own line numbers, as a unified
/// diff writes it. An empty range is placed after the line before it.
fn span(lines: &[Line], range: &std::ops::Range<usize>) -> String {
    let count = range.end - range.start;
    let number_at = |i: usize| lines.get(i).and_then(|l| l.number);
    let start = if count == 0 {
        // Unified diffs give the line *before* an insertion or deletion.
        match range.start.checked_sub(1) {
            Some(before) => number_at(before).unwrap_or(range.start as u64),
            None => number_at(0).map_or(0, |n| n.saturating_sub(1)),
        }
    } else {
        number_at(range.start).unwrap_or(range.start as u64 + 1)
    };
    format!("{start},{count}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file as `read_file` shows it: numbered lines from `first`.
    fn shown(lines: &[&str], first: u64) -> String {
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{}\t{l}", first + i as u64))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn file(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("    let value_{i} = compute_something({i});"))
            .collect()
    }

    #[test]
    fn a_line_added_near_the_top_sends_only_that_line() {
        let before = file(60);
        let mut after = before.clone();
        after.insert(2, "    // added".to_string());
        let old = shown(&before.iter().map(String::as_str).collect::<Vec<_>>(), 1);
        let new = shown(&after.iter().map(String::as_str).collect::<Vec<_>>(), 1);
        let note = change_note("call_7", &old, &new).expect("a short note");
        assert!(note.starts_with("Changed since your earlier read call_7"));
        // Inserted after line 2, as line 3; nothing else listed although
        // every later line number moved down by one.
        assert!(note.contains("@@ -2,0 +3,1 @@\n+3\t    // added"), "{note}");
        assert_eq!(note.matches("\n+").count(), 1, "{note}");
        assert_eq!(note.matches("\n-").count(), 0, "{note}");
        assert!(note.len() * 4 < new.len());
    }

    #[test]
    fn a_changed_line_shows_its_old_text_and_its_new_one() {
        let before = file(60);
        let mut after = before.clone();
        after[39] = "    let value_39 = fixed(39);".to_string();
        let old = shown(&before.iter().map(String::as_str).collect::<Vec<_>>(), 1);
        let new = shown(&after.iter().map(String::as_str).collect::<Vec<_>>(), 1);
        let note = change_note("c1", &old, &new).unwrap();
        assert!(
            note.ends_with(
                "@@ -40,1 +40,1 @@\n-    let value_39 = compute_something(39);\n\
                 +40\t    let value_39 = fixed(39);"
            ),
            "{note}"
        );
    }

    #[test]
    fn removed_lines_and_separate_changes_get_one_hunk_each() {
        let before = file(80);
        let mut after = before.clone();
        after.drain(10..13); // lines 11-13 removed
        after[60] = "    changed();".to_string(); // was line 64
        let old = shown(&before.iter().map(String::as_str).collect::<Vec<_>>(), 1);
        let new = shown(&after.iter().map(String::as_str).collect::<Vec<_>>(), 1);
        let note = change_note("c1", &old, &new).unwrap();
        assert!(
            note.contains("@@ -11,3 +10,0 @@\n-    let value_10"),
            "{note}"
        );
        assert!(note.contains("@@ -64,1 +61,1 @@"), "{note}");
        assert_eq!(note.matches("@@ -").count(), 2);
    }

    #[test]
    fn a_window_read_keeps_the_files_own_line_numbers() {
        let before = file(30);
        let mut after = before.clone();
        after[5] = "    middle();".to_string();
        let mut old = shown(&before.iter().map(String::as_str).collect::<Vec<_>>(), 101);
        let mut new = shown(&after.iter().map(String::as_str).collect::<Vec<_>>(), 101);
        old.push_str("\n[lines 101-130 of 400; read on with offset 131]");
        new.push_str("\n[lines 101-130 of 400; read on with offset 131]");
        let note = change_note("c1", &old, &new).unwrap();
        assert!(note.contains("@@ -106,1 +106,1 @@"), "{note}");
        assert!(note.contains("+106\t    middle();"), "{note}");
        assert!(!note.contains("read on"), "{note}");
    }

    #[test]
    fn a_new_closing_note_is_sent_like_any_line() {
        let lines = file(30);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let old = shown(&refs, 1);
        let new = format!("{old}\n[lines 1-30 of 31; read on with offset 31]");
        let note = change_note("c1", &old, &new).unwrap();
        assert!(note.ends_with("+[lines 1-30 of 31; read on with offset 31]"));
    }

    #[test]
    fn nothing_changed_or_too_much_changed_sends_the_file() {
        let before = file(20);
        let refs: Vec<&str> = before.iter().map(String::as_str).collect();
        let old = shown(&refs, 1);
        assert_eq!(change_note("c1", &old, &old), None);

        let after: Vec<String> = (0..20).map(|i| format!("fn other_{i}() {{}}")).collect();
        let refs: Vec<&str> = after.iter().map(String::as_str).collect();
        assert_eq!(change_note("c1", &old, &shown(&refs, 1)), None);
    }

    #[test]
    fn an_insertion_at_the_very_top_is_placed_before_line_one() {
        let before = file(40);
        let mut after = before.clone();
        after.insert(0, "// header".to_string());
        let old = shown(&before.iter().map(String::as_str).collect::<Vec<_>>(), 1);
        let new = shown(&after.iter().map(String::as_str).collect::<Vec<_>>(), 1);
        let note = change_note("c1", &old, &new).unwrap();
        assert!(note.contains("@@ -0,0 +1,1 @@\n+1\t// header"), "{note}");
    }

    #[test]
    fn text_that_only_looks_numbered_is_compared_whole() {
        assert_eq!(Line::parse("12\tx").number, Some(12));
        assert_eq!(Line::parse("\tx").number, None);
        assert_eq!(Line::parse("1a\tx").text, "1a\tx");
        assert_eq!(Line::parse("(empty file)").number, None);
    }
}
