//! Line-based chunking.
//!
//! A file is cut into chunks of about [`TARGET_LINES`] lines / [`TARGET_BYTES`]
//! bytes. Past the target a chunk ends at the next blank line; at the hard
//! caps it ends at the last blank line seen after half the target, or in the
//! middle of a paragraph when there was none. Chunks never start or end with a
//! blank line, never overlap, and are never empty; blank lines between chunks
//! belong to no chunk.

pub(crate) const TARGET_LINES: usize = 40;
pub(crate) const TARGET_BYTES: usize = 2048;
pub(crate) const HARD_LINES: usize = 80;
pub(crate) const HARD_BYTES: usize = 4096;
/// A blank line this far into a chunk is remembered as a fallback break.
const SOFT_MIN_LINES: usize = TARGET_LINES / 2;
const SOFT_MIN_BYTES: usize = TARGET_BYTES / 2;

/// Lines `start..=end`, 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// Cut `lines` (a file without line terminators) into chunks.
pub(crate) fn split(lines: &[&str]) -> Vec<Span> {
    let n = lines.len();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < n {
        if is_blank(lines[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let mut bytes = 0usize;
        // Last non-blank line of the chunk so far.
        let mut last = i;
        // (blank line, last non-blank line before it) to fall back on at a hard cap.
        let mut soft: Option<(usize, usize)> = None;
        let mut j = i;
        let (end, next) = loop {
            if j >= n {
                break (last, n);
            }
            let len = lines[j].len() + 1;
            if j > start && (j - start >= HARD_LINES || bytes + len > HARD_BYTES) {
                break match soft {
                    Some((blank, before)) => (before, blank + 1),
                    None => (last, j),
                };
            }
            bytes += len;
            if is_blank(lines[j]) {
                let size = j - start + 1;
                if size >= TARGET_LINES || bytes >= TARGET_BYTES {
                    break (last, j + 1);
                }
                if size >= SOFT_MIN_LINES || bytes >= SOFT_MIN_BYTES {
                    soft = Some((j, last));
                }
            } else {
                last = j;
            }
            j += 1;
        };
        spans.push(Span {
            start: start + 1,
            end: end + 1,
        });
        i = next;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    fn run(lines: &[String]) -> Vec<Span> {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        split(&refs)
    }

    fn sp(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    #[test]
    fn empty_and_blank_files_have_no_chunks() {
        assert!(run(&[]).is_empty());
        assert!(run(&["".into(), "   ".into(), "\t".into()]).is_empty());
    }

    #[test]
    fn small_file_is_one_chunk() {
        assert_eq!(run(&numbered(10)), [sp(1, 10)]);
    }

    #[test]
    fn leading_and_trailing_blank_lines_are_not_part_of_a_chunk() {
        let lines: Vec<String> = ["", "", "a", "b", "", ""].map(String::from).to_vec();
        assert_eq!(run(&lines), [sp(3, 4)]);
    }

    #[test]
    fn long_run_without_blank_lines_is_cut_at_the_hard_cap() {
        assert_eq!(run(&numbered(200)), [sp(1, 80), sp(81, 160), sp(161, 200)]);
    }

    #[test]
    fn past_the_target_the_next_blank_line_ends_the_chunk() {
        let mut lines = numbered(50);
        lines.insert(45, String::new()); // blank at line 46
        let spans = run(&lines);
        // 40 lines is the target, the first blank line after it is line 46.
        assert_eq!(spans[0], sp(1, 45));
        assert_eq!(spans[1], sp(47, 51));
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn blank_lines_before_the_target_do_not_split() {
        let mut lines = numbered(30);
        lines.insert(10, String::new());
        lines.insert(20, String::new());
        assert_eq!(run(&lines), [sp(1, 32)]);
    }

    #[test]
    fn hard_cap_prefers_the_last_blank_line_after_half_the_target() {
        // A blank at line 30, then 70 more lines with no blank: the cut falls
        // back to the blank at 30 instead of splitting a paragraph at line 80.
        let mut lines = numbered(100);
        lines.insert(29, String::new());
        let spans = run(&lines);
        assert_eq!(spans[0], sp(1, 29));
        assert_eq!(spans[1].start, 31);
    }

    #[test]
    fn byte_cap_applies_to_long_lines() {
        let long = "x".repeat(1500);
        let lines = vec![long.clone(), long.clone(), long.clone(), long.clone()];
        // 1501 bytes per line: three lines would be 4503 > HARD_BYTES.
        assert_eq!(run(&lines), [sp(1, 2), sp(3, 4)]);
    }

    #[test]
    fn one_enormous_line_is_one_chunk() {
        let lines = vec!["y".repeat(100_000), "after".to_string()];
        assert_eq!(run(&lines), [sp(1, 1), sp(2, 2)]);
    }

    #[test]
    fn every_non_blank_line_is_in_exactly_one_chunk() {
        // Deterministic pseudo-random blank/non-blank layout, long enough to
        // exercise every branch.
        let mut seed = 0x9e37_79b9_u32;
        let lines: Vec<String> = (0..2000)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                match seed >> 28 {
                    0..=2 => String::new(),
                    3 => "z".repeat(((seed >> 8) % 900) as usize + 1),
                    _ => format!("code {i}"),
                }
            })
            .collect();
        let spans = run(&lines);
        let mut covered = vec![0u8; lines.len() + 1];
        let mut prev_end = 0;
        for s in &spans {
            assert!(s.start >= 1 && s.start <= s.end && s.end <= lines.len());
            assert!(s.start > prev_end, "chunks overlap or are out of order");
            prev_end = s.end;
            assert!(!is_blank(&lines[s.start - 1]) && !is_blank(&lines[s.end - 1]));
            assert!(s.end - s.start < HARD_LINES);
            let bytes: usize = lines[s.start - 1..s.end].iter().map(|l| l.len() + 1).sum();
            assert!(bytes <= HARD_BYTES || s.start == s.end, "{bytes} bytes");
            for c in covered.iter_mut().take(s.end + 1).skip(s.start) {
                *c += 1;
            }
        }
        for (i, l) in lines.iter().enumerate() {
            if !is_blank(l) {
                assert_eq!(
                    covered[i + 1],
                    1,
                    "line {} covered {} times",
                    i + 1,
                    covered[i + 1]
                );
            }
        }
    }
}
