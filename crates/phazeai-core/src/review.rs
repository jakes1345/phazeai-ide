//! Reviewing a proposed file change one hunk at a time.
//!
//! A change from `before` to `after` splits into hunks: runs of lines that
//! differ, with unchanged lines between them. The reviewer keeps or undoes
//! each one, and [`apply`] builds the file from the kept hunks and the
//! original lines of the rest, so a reviewer can take the fix and leave the
//! renamed variable, as with Cursor's per-hunk keep and undo.

use std::ops::Range;

use similar::{DiffOp, TextDiff};

/// One run of changed lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The lines it replaces in `before` (0-based, end exclusive; empty for a
    /// pure insertion).
    pub before: Range<usize>,
    /// The lines it writes in `after` (empty for a pure deletion).
    pub after: Range<usize>,
}

impl Hunk {
    /// The lines of `after` it writes, as 1-based numbers for people: the
    /// first and last, or the line it follows for a deletion.
    pub fn after_lines(&self) -> (usize, usize) {
        if self.after.is_empty() {
            (self.after.start, self.after.start)
        } else {
            (self.after.start + 1, self.after.end)
        }
    }
}

/// One row of a change, for showing it: an unchanged line, or a hunk's
/// removed and added lines (each line keeps its newline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Same(String),
    Hunk {
        index: usize,
        removed: Vec<String>,
        added: Vec<String>,
    },
}

fn diff<'a>(before: &'a str, after: &'a str) -> TextDiff<'a, 'a, 'a, str> {
    TextDiff::from_lines(before, after)
}

/// The hunks of the change from `before` to `after`, in order. Adjacent
/// deletions and insertions (a replaced line) are one hunk.
pub fn hunks(before: &str, after: &str) -> Vec<Hunk> {
    let diff = diff(before, after);
    let mut out: Vec<Hunk> = Vec::new();
    let mut open = false;
    for op in diff.ops() {
        if let DiffOp::Equal { .. } = op {
            open = false;
            continue;
        }
        let (b, a) = (op.old_range(), op.new_range());
        match out.last_mut() {
            Some(h) if open => {
                h.before.end = b.end;
                h.after.end = a.end;
            }
            _ => out.push(Hunk {
                before: b,
                after: a,
            }),
        }
        open = true;
    }
    out
}

/// The change as rows to show: unchanged lines and hunks, in file order.
pub fn rows(before: &str, after: &str) -> Vec<Row> {
    let diff = diff(before, after);
    let old = diff.old_slices();
    let new = diff.new_slices();
    let mut rows = Vec::new();
    let mut at_before = 0;
    for (index, h) in hunks(before, after).into_iter().enumerate() {
        rows.extend(
            old[at_before..h.before.start]
                .iter()
                .map(|l| Row::Same(l.to_string())),
        );
        rows.push(Row::Hunk {
            index,
            removed: old[h.before.clone()]
                .iter()
                .map(|l| l.to_string())
                .collect(),
            added: new[h.after.clone()].iter().map(|l| l.to_string()).collect(),
        });
        at_before = h.before.end;
    }
    rows.extend(old[at_before..].iter().map(|l| Row::Same(l.to_string())));
    rows
}

/// The file with the hunks `keep` marks (by index, as from [`hunks`]) taken
/// from `after` and every other line as in `before`. All kept gives `after`;
/// none gives `before`. A hunk `keep` has no entry for is undone.
pub fn apply(before: &str, after: &str, keep: &[bool]) -> String {
    let diff = diff(before, after);
    let old = diff.old_slices();
    let new = diff.new_slices();
    let mut out = String::with_capacity(after.len().max(before.len()));
    let mut at_before = 0;
    for (index, h) in hunks(before, after).into_iter().enumerate() {
        out.extend(old[at_before..h.before.start].iter().copied());
        if keep.get(index).copied().unwrap_or(false) {
            out.extend(new[h.after.clone()].iter().copied());
        } else {
            out.extend(old[h.before.clone()].iter().copied());
        }
        at_before = h.before.end;
    }
    out.extend(old[at_before..].iter().copied());
    out
}

/// A short note for the model on a partly kept change, naming what was not
/// written by the lines of its own version, so it doesn't make it again
/// unasked: "The user kept 2 of 3 changes; not written: lines 14-16."
pub fn partial_note(before: &str, after: &str, keep: &[bool]) -> String {
    let all = hunks(before, after);
    let kept = (0..all.len())
        .filter(|&i| keep.get(i).copied().unwrap_or(false))
        .count();
    let undone: Vec<String> = all
        .iter()
        .enumerate()
        .filter(|(i, _)| !keep.get(*i).copied().unwrap_or(false))
        .map(|(_, h)| match h.after_lines() {
            (a, _) if h.after.is_empty() => format!("the deletion after line {a}"),
            (a, b) if a == b => format!("line {a}"),
            (a, b) => format!("lines {a}-{b}"),
        })
        .collect();
    format!(
        "The user reviewed this change and kept {kept} of {} parts. Not written \
         (by line in your version): {}. The file holds the kept parts only.",
        all.len(),
        undone.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str =
        "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n\nfn helper() {}\n";
    const AFTER: &str =
        "fn main() {\n    let x = 2;\n    println!(\"{x}\");\n}\n\nfn helper() {\n    todo!()\n}\n";

    #[test]
    fn a_change_splits_into_its_separate_runs_of_lines() {
        let h = hunks(BEFORE, AFTER);
        assert_eq!(h.len(), 2, "{h:?}");
        assert_eq!(h[0].before, 1..2);
        assert_eq!(h[0].after, 1..2);
        assert_eq!(h[1].before, 5..6);
        assert_eq!(h[1].after, 5..8);
        assert_eq!(h[1].after_lines(), (6, 8));
    }

    #[test]
    fn keeping_all_gives_the_change_and_none_gives_the_original() {
        assert_eq!(apply(BEFORE, AFTER, &[true, true]), AFTER);
        assert_eq!(apply(BEFORE, AFTER, &[false, false]), BEFORE);
        assert_eq!(apply(BEFORE, AFTER, &[]), BEFORE);
    }

    #[test]
    fn keeping_one_hunk_writes_only_that_one() {
        let first_only = apply(BEFORE, AFTER, &[true, false]);
        assert_eq!(
            first_only,
            "fn main() {\n    let x = 2;\n    println!(\"{x}\");\n}\n\nfn helper() {}\n"
        );
        let second_only = apply(BEFORE, AFTER, &[false, true]);
        assert!(second_only.contains("let x = 1;") && second_only.contains("todo!()"));
    }

    #[test]
    fn insertions_deletions_and_new_files_round_trip() {
        let cases = [
            ("", "a\nb\n"),
            ("a\nb\n", ""),
            ("a\nb\nc\n", "a\nc\n"),
            ("a\nc\n", "a\nb\nc\n"),
            ("a\nb", "a\nB"),
            ("x\n", "x\n"),
        ];
        for (before, after) in cases {
            let n = hunks(before, after).len();
            assert_eq!(
                apply(before, after, &vec![true; n]),
                after,
                "{before:?} → {after:?}"
            );
            assert_eq!(
                apply(before, after, &vec![false; n]),
                before,
                "{before:?} → {after:?}"
            );
        }
        assert!(hunks("x\n", "x\n").is_empty());
    }

    #[test]
    fn rows_show_every_line_once_in_order() {
        let rows = rows(BEFORE, AFTER);
        let hunks: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Hunk {
                    index,
                    removed,
                    added,
                } => Some((*index, removed.len(), added.len())),
                Row::Same(_) => None,
            })
            .collect();
        assert_eq!(hunks, [(0, 1, 1), (1, 1, 3)]);
        let same = rows.iter().filter(|r| matches!(r, Row::Same(_))).count();
        assert_eq!(same, 4);
    }

    #[test]
    fn the_note_names_what_was_not_written() {
        let note = partial_note(BEFORE, AFTER, &[true, false]);
        assert!(note.contains("kept 1 of 2"), "{note}");
        assert!(note.contains("lines 6-8"), "{note}");
        assert!(!note.contains("line 2"), "{note}");
    }
}
