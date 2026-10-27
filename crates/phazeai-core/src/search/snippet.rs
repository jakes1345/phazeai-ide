//! Choosing the lines of a chunk worth showing.

use super::tokenize::Tokenizer;
use std::collections::HashMap;

/// Most lines in a snippet.
pub(crate) const MAX_LINES: usize = 10;
/// Most characters in a snippet.
pub(crate) const MAX_CHARS: usize = 600;

pub(crate) struct Snippet {
    /// 1-based first and last line of `text`.
    pub start: usize,
    pub end: usize,
    pub text: String,
}

fn blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn width(line: &str) -> usize {
    line.trim_end().chars().count()
}

/// The densest window of at most [`MAX_LINES`] lines of a chunk.
///
/// `lines` are the chunk's lines, `first_line` the 1-based number of the first
/// one, `terms` maps each query term to its index (below 64). The window with
/// the most distinct query terms wins, then the most term occurrences, then
/// the earliest. Blank edge lines are trimmed, and while the text is over
/// [`MAX_CHARS`] the edge line with fewer hits is dropped, so the result is
/// always exactly the lines `start..=end`.
pub(crate) fn build(lines: &[&str], first_line: usize, terms: &HashMap<&str, usize>) -> Snippet {
    let mut tk = Tokenizer::new();
    let info: Vec<(u64, u32)> = lines
        .iter()
        .map(|line| {
            let (mut mask, mut count) = (0u64, 0u32);
            tk.run(line, |t| {
                if let Some(&i) = terms.get(t) {
                    mask |= 1u64 << i;
                    count += 1;
                }
            });
            (mask, count)
        })
        .collect();

    let w = lines.len().min(MAX_LINES);
    let mut best_key: Option<(u32, u32)> = None;
    let mut a = 0;
    for s in 0..=lines.len() - w {
        let win = &info[s..s + w];
        let key = (
            win.iter().fold(0u64, |m, x| m | x.0).count_ones(),
            win.iter().map(|x| x.1).sum::<u32>(),
        );
        if best_key.is_none_or(|b| key > b) {
            best_key = Some(key);
            a = s;
        }
    }
    let mut b = a + w;

    let trim = |a: &mut usize, b: &mut usize| {
        while *a + 1 < *b && blank(lines[*a]) {
            *a += 1;
        }
        while *b > *a + 1 && blank(lines[*b - 1]) {
            *b -= 1;
        }
    };
    let chars = |a: usize, b: usize| {
        lines[a..b].iter().map(|l| width(l)).sum::<usize>() + (b - a).saturating_sub(1)
    };
    trim(&mut a, &mut b);
    while b - a > 1 && chars(a, b) > MAX_CHARS {
        // Drop the edge line with fewer hits; on a tie, the last one.
        if info[a].1 < info[b - 1].1 {
            a += 1;
        } else {
            b -= 1;
        }
        trim(&mut a, &mut b);
    }

    let mut text = lines[a..b]
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    if chars(a, b) > MAX_CHARS {
        // A single enormous line.
        text = text.chars().take(MAX_CHARS - 1).collect::<String>() + "\u{2026}";
    }
    Snippet {
        start: first_line + a,
        end: first_line + b - 1,
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(list: &[&'static str]) -> HashMap<&'static str, usize> {
        list.iter().enumerate().map(|(i, t)| (*t, i)).collect()
    }

    fn snip(lines: &[&str], first: usize, q: &[&'static str]) -> Snippet {
        build(lines, first, &terms(q))
    }

    #[test]
    fn short_chunks_are_returned_whole_with_exact_line_numbers() {
        let lines = ["fn a() {", "    parse();", "}"];
        let s = snip(&lines, 10, &["parse"]);
        assert_eq!((s.start, s.end), (10, 12));
        assert_eq!(s.text, "fn a() {\n    parse();\n}");
    }

    #[test]
    fn long_chunks_are_cut_to_the_densest_window() {
        let mut lines: Vec<String> = (0..40).map(|i| format!("filler {i}")).collect();
        lines[25] = "json parse here".into();
        lines[27] = "config too".into();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = snip(&refs, 100, &["json", "config"]);
        let n = s.text.lines().count();
        assert!(n <= MAX_LINES);
        assert_eq!(s.end - s.start + 1, n);
        let shown: Vec<&str> = s.text.lines().collect();
        assert!(shown.contains(&"json parse here") && shown.contains(&"config too"));
        // Line 25 of the chunk is file line 125.
        assert!(s.start <= 125 && s.end >= 127);
        assert_eq!(refs[s.start - 100], shown[0]);
    }

    #[test]
    fn more_distinct_terms_beat_more_occurrences() {
        let mut lines: Vec<String> = (0..60).map(|i| format!("x{i}")).collect();
        for l in &mut lines[2..8] {
            *l = "alpha alpha alpha".into();
        }
        lines[45] = "alpha beta".into();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = snip(&refs, 1, &["alpha", "beta"]);
        assert!(s.text.contains("alpha beta"), "{}", s.text);
    }

    #[test]
    fn ties_go_to_the_earliest_window() {
        let lines: Vec<String> = (0..30).map(|i| format!("alpha {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = snip(&refs, 1, &["alpha"]);
        assert_eq!((s.start, s.end), (1, 10));
    }

    #[test]
    fn no_hits_shows_the_start_of_the_chunk() {
        let lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = snip(&refs, 5, &["absent"]);
        assert_eq!(s.start, 5);
        assert_eq!(s.end, 14);
    }

    #[test]
    fn blank_edges_are_trimmed_from_the_reported_range() {
        let mut lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        // The earliest window holding the hit (lines 3..=12) starts on blank lines.
        lines[3] = "".into();
        lines[4] = "   ".into();
        lines[12] = "needle".into();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = snip(&refs, 1, &["needle"]);
        assert_eq!((s.start, s.end), (6, 13));
        assert_eq!(s.text.lines().next(), Some("line 5"));
        assert_eq!(s.text.lines().last(), Some("needle"));
        assert_eq!(s.text.lines().count(), s.end - s.start + 1);
    }

    #[test]
    fn over_long_windows_shrink_toward_the_hits() {
        let filler = "f".repeat(120);
        let mut lines: Vec<String> = vec![filler; 10];
        lines[6] = format!("needle {}", "n".repeat(100));
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let s = snip(&refs, 1, &["needle"]);
        assert!(
            s.text.chars().count() <= MAX_CHARS,
            "{}",
            s.text.chars().count()
        );
        assert!(s.text.contains("needle"));
        assert_eq!(s.text.lines().count(), s.end - s.start + 1);
    }

    #[test]
    fn a_single_huge_line_is_truncated() {
        let line = format!("needle {}", "z".repeat(5000));
        let s = snip(&[line.as_str()], 7, &["needle"]);
        assert_eq!((s.start, s.end), (7, 7));
        assert_eq!(s.text.chars().count(), MAX_CHARS);
        assert!(s.text.ends_with('\u{2026}'));
    }

    #[test]
    fn trailing_whitespace_and_carriage_returns_are_removed() {
        let s = snip(&["alpha  \r", "beta\t"], 1, &["alpha"]);
        assert_eq!(s.text, "alpha\nbeta");
    }
}
