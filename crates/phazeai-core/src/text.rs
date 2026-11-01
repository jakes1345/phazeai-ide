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
}
