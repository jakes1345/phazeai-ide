//! UTF-8–safe string truncation helpers.
//!
//! Slicing a `&str` at an arbitrary byte index panics when it lands inside a
//! multi-byte character (any non-ASCII source file, emoji in a diff, etc.).

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
}
