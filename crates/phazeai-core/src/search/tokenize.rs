//! Identifier-aware tokenizer shared by indexing and querying.
//!
//! Text is cut into runs of alphanumeric characters and `_`. Each run is
//! emitted lowercased as a whole, and so are its pieces: `snake_case` parts,
//! `camelCase` / `PascalCase` words, acronyms (`parseJSONConfig` -> `parse`,
//! `json`, `config`) and letter/digit boundaries (`utf8` -> `utf`, `8`). So a
//! query for `json config` matches the identifier `parseJsonConfig`, and a
//! query for the identifier itself matches only code that spells it that way
//! more strongly than prose does.
//!
//! Dropped: single characters, tokens over [`MAX_TOKEN_CHARS`] (the whole of a
//! long identifier is not kept, its pieces are: `shouldReturnEmptyList...` is
//! found by `empty list`), runs over [`MAX_RUN_CHARS`] (junk as a whole: base64,
//! minified code), long hex and number runs, and a short list of English stop
//! words.

use std::ops::Range;

/// Longest token, in characters, that is kept.
pub(crate) const MAX_TOKEN_CHARS: usize = 64;
/// Longest run of word characters that is looked into. Real identifiers, even
/// descriptive test names, stay far below it; a longer run is data. The cap
/// also bounds the work (splitting is quadratic in the number of pieces).
pub(crate) const MAX_RUN_CHARS: usize = 256;
/// Most distinct terms one query contributes (the snippet picker keeps a
/// `u64` bit mask per line, one bit per term).
pub(crate) const MAX_QUERY_TERMS: usize = 64;

/// Sorted, for `binary_search`.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "do", "does", "for", "how", "if", "in",
    "is", "it", "of", "on", "or", "that", "the", "this", "to", "was", "we", "were", "what", "when",
    "where", "which", "who", "why", "will", "with", "you", "your",
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Lower,
    Upper,
    Digit,
    /// Letters without case (CJK, ...).
    Other,
}

fn class(c: char) -> Class {
    if c.is_numeric() {
        Class::Digit
    } else if c.is_uppercase() {
        Class::Upper
    } else if c.is_lowercase() {
        Class::Lower
    } else {
        Class::Other
    }
}

/// Split `seg` (no `_`) at camelCase, acronym, letter/digit and script
/// boundaries; `out` receives byte ranges.
fn split_words(seg: &str, out: &mut Vec<(usize, usize)>) {
    out.clear();
    let mut start = 0;
    let mut prev: Option<Class> = None;
    let mut chars = seg.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let cls = class(c);
        if let Some(p) = prev {
            let boundary = match (p, cls) {
                (Class::Lower, Class::Upper) => true,
                // JSONConfig: the last capital of an acronym starts the next word.
                (Class::Upper, Class::Upper) => {
                    chars.peek().is_some_and(|&(_, n)| class(n) == Class::Lower)
                }
                (Class::Digit, Class::Digit) => false,
                (Class::Digit, _) | (_, Class::Digit) => true,
                (Class::Other, Class::Other) => false,
                (Class::Other, _) | (_, Class::Other) => true,
                _ => false,
            };
            if boundary {
                out.push((start, i));
                start = i;
            }
        }
        prev = Some(cls);
    }
    out.push((start, seg.len()));
}

/// Long hex runs (hashes, ids) and long numbers carry no meaning to search for.
fn is_junk(tok: &str) -> bool {
    let b = tok.as_bytes();
    if b.len() >= 5 && b.iter().all(u8::is_ascii_digit) {
        return true;
    }
    if b.len() >= 8 && b.iter().all(u8::is_ascii_hexdigit) {
        return true;
    }
    tok.strip_prefix("0x")
        .is_some_and(|rest| rest.len() >= 6 && rest.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn accept(tok: &str) -> bool {
    let n = tok.chars().count();
    (2..=MAX_TOKEN_CHARS).contains(&n) && STOPWORDS.binary_search(&tok).is_err() && !is_junk(tok)
}

/// Byte ranges of the runs of word characters in `text`.
fn word_runs(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut chars = text.char_indices();
    let mut start = None;
    std::iter::from_fn(move || {
        for (i, c) in chars.by_ref() {
            if is_word_char(c) {
                start.get_or_insert(i);
            } else if let Some(s) = start.take() {
                return Some((s, i));
            }
        }
        start.take().map(|s| (s, text.len()))
    })
}

/// Reusable tokenizer state (scratch buffers); create one per file or query.
pub(crate) struct Tokenizer {
    /// Candidate tokens of the identifier being processed; `cands[..used]` are live.
    cands: Vec<String>,
    used: usize,
    parts: Vec<(usize, usize)>,
}

impl Tokenizer {
    pub(crate) fn new() -> Self {
        Self {
            cands: Vec::new(),
            used: 0,
            parts: Vec::new(),
        }
    }

    /// Call `emit` for every token of `text`, in order of appearance. An
    /// identifier yields its whole lowercased form first, then its pieces.
    pub(crate) fn run<F: FnMut(&str)>(&mut self, text: &str, mut emit: F) {
        for (a, b) in word_runs(text) {
            self.word(&text[a..b], &mut emit);
        }
    }

    /// The byte range of the first run of `text` that yields a token `wanted`
    /// accepts: where a query term matches, for cutting an excerpt.
    pub(crate) fn first_match(
        &mut self,
        text: &str,
        wanted: impl Fn(&str) -> bool,
    ) -> Option<Range<usize>> {
        for (a, b) in word_runs(text) {
            let mut found = false;
            self.word(&text[a..b], &mut |t: &str| found |= wanted(t));
            if found {
                return Some(a..b);
            }
        }
        None
    }

    fn word<F: FnMut(&str)>(&mut self, word: &str, emit: &mut F) {
        // Cheap reject before counting characters: no kept run has more bytes.
        if word.len() > MAX_RUN_CHARS * 4 || word.chars().count() > MAX_RUN_CHARS {
            return;
        }
        // Most tokens are plain lowercase words: no allocation, no splitting.
        if word.bytes().all(|b| b.is_ascii_lowercase()) {
            if accept(word) {
                emit(word);
            }
            return;
        }
        self.used = 0;
        self.push_lower(word);
        if is_junk(&self.cands[0]) {
            return;
        }
        for segment in word.split('_').filter(|s| !s.is_empty()) {
            self.push_lower(segment);
            split_words(segment, &mut self.parts);
            if self.parts.len() > 1 {
                for i in 0..self.parts.len() {
                    let (a, b) = self.parts[i];
                    self.push_lower(&segment[a..b]);
                }
            }
        }
        for tok in &self.cands[..self.used] {
            if accept(tok) {
                emit(tok);
            }
        }
    }

    /// Lowercase `s` into the next candidate slot unless an equal candidate exists.
    fn push_lower(&mut self, s: &str) {
        if self.used == self.cands.len() {
            self.cands.push(String::new());
        }
        let slot = &mut self.cands[self.used];
        slot.clear();
        for c in s.chars() {
            slot.extend(c.to_lowercase());
        }
        let (live, rest) = self.cands.split_at(self.used);
        if !live.contains(&rest[0]) {
            self.used += 1;
        }
    }
}

/// The distinct terms of a query in order of first appearance (at most
/// [`MAX_QUERY_TERMS`]). Empty when the query has nothing searchable.
pub(crate) fn query_terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    Tokenizer::new().run(query, |t| {
        if out.len() < MAX_QUERY_TERMS && !out.iter().any(|x| x == t) {
            out.push(t.to_owned());
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        Tokenizer::new().run(text, |t| out.push(t.to_owned()));
        out
    }

    fn has(text: &str, expected: &[&str]) {
        let got = toks(text);
        for e in expected {
            assert!(got.iter().any(|g| g == e), "{text:?}: {e:?} not in {got:?}");
        }
    }

    #[test]
    fn stopwords_are_sorted() {
        assert!(STOPWORDS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn plain_words_are_lowercased_runs() {
        assert_eq!(
            toks("Hello, World! foo-bar"),
            ["hello", "world", "foo", "bar"]
        );
    }

    #[test]
    fn snake_case_yields_whole_and_parts() {
        assert_eq!(
            toks("parse_json_config"),
            ["parse_json_config", "parse", "json", "config"]
        );
        assert_eq!(toks("__init__"), ["__init__", "init"]);
        assert_eq!(
            toks("MAX_RETRY_COUNT"),
            ["max_retry_count", "max", "retry", "count"]
        );
    }

    #[test]
    fn camel_and_pascal_case_split_into_words() {
        assert_eq!(
            toks("parseJsonConfig"),
            ["parsejsonconfig", "parse", "json", "config"]
        );
        assert_eq!(
            toks("JsonConfigParser"),
            ["jsonconfigparser", "json", "config", "parser"]
        );
    }

    #[test]
    fn acronyms_stay_together_and_end_before_the_next_word() {
        assert_eq!(
            toks("parseJSONConfig"),
            ["parsejsonconfig", "parse", "json", "config"]
        );
        assert_eq!(toks("HTTPServer"), ["httpserver", "http", "server"]);
        assert_eq!(toks("IOError"), ["ioerror", "io", "error"]);
        assert_eq!(toks("getURL"), ["geturl", "get", "url"]);
    }

    #[test]
    fn digits_split_off_and_one_char_pieces_are_dropped() {
        assert_eq!(toks("utf8"), ["utf8", "utf"]);
        assert_eq!(toks("sha256"), ["sha256", "sha", "256"]);
        assert_eq!(toks("x86_64"), ["x86_64", "x86", "86", "64"]);
        assert_eq!(toks("h264Decoder"), ["h264decoder", "264", "decoder"]);
    }

    #[test]
    fn mixed_snake_and_camel() {
        assert_eq!(
            toks("fooBar_baz"),
            ["foobar_baz", "foobar", "foo", "bar", "baz"]
        );
    }

    #[test]
    fn repeated_pieces_are_emitted_once_per_identifier() {
        assert_eq!(toks("item_item"), ["item_item", "item"]);
        assert_eq!(toks("Config"), ["config"]);
    }

    #[test]
    fn unicode_words_are_kept_and_lowercased() {
        assert_eq!(toks("Größe"), ["größe"]);
        assert_eq!(toks("ÜberCool"), ["übercool", "über", "cool"]);
        assert_eq!(toks("日本語 text"), ["日本語", "text"]);
        has("naïve_café", &["naïve", "café"]);
    }

    #[test]
    fn single_chars_and_stop_words_are_dropped() {
        assert_eq!(toks("a b c the of x"), Vec::<String>::new());
        // Pieces that are stop words go too, the whole identifier stays.
        assert_eq!(toks("is_empty"), ["is_empty", "empty"]);
        // Two-character tokens are kept.
        assert_eq!(toks("io fs ok"), ["io", "fs", "ok"]);
    }

    #[test]
    fn long_junk_is_dropped() {
        // One word that is too long to keep has nothing to split into.
        let long = "a".repeat(MAX_TOKEN_CHARS + 1);
        assert!(toks(&long).is_empty());
        // A run too long to be an identifier is data: neither it nor its
        // camelCase pieces are emitted.
        let blob = "AbCd".repeat(MAX_RUN_CHARS / 2);
        assert!(blob.len() > MAX_RUN_CHARS);
        assert!(toks(&blob).is_empty());
        let max = "z".repeat(MAX_TOKEN_CHARS);
        assert_eq!(toks(&max), std::slice::from_ref(&max));
        assert!(toks("deadbeefcafe0123").is_empty());
        assert!(toks("0xdeadbeef").is_empty());
        assert!(toks("123456789").is_empty());
        assert!(toks("550e8400").is_empty());
    }

    #[test]
    fn long_identifiers_are_not_kept_whole_but_their_words_are() {
        // Descriptive names (test functions, constants) run past 64 characters.
        let camel = "shouldReturnEmptyListWhenUserHasNoPermissionsAndAccountIsDisabledForTenant";
        assert!(camel.chars().count() > MAX_TOKEN_CHARS);
        assert_eq!(
            toks(camel),
            [
                "should",
                "return",
                "empty",
                "list",
                "user",
                "has",
                "no",
                "permissions",
                "account",
                "disabled",
                "tenant"
            ]
        );
        let snake = "a_partial_build_before_the_first_search_does_not_stop_the_search_from_indexing_the_rest";
        assert!(snake.chars().count() > MAX_TOKEN_CHARS);
        assert_eq!(
            toks(snake),
            [
                "partial", "build", "before", "first", "search", "not", "stop", "from", "indexing",
                "rest"
            ]
        );
        // The longest run that is still looked into.
        let edge = format!("{}_end", "word_".repeat((MAX_RUN_CHARS - 4) / 5));
        assert!(edge.chars().count() <= MAX_RUN_CHARS);
        assert_eq!(toks(&edge), ["word", "end"]);
        let beyond = format!("{edge}_end");
        assert!(beyond.chars().count() > MAX_RUN_CHARS);
        assert!(toks(&beyond).is_empty());
        // A query spelling the identifier finds the same words.
        for t in query_terms(snake) {
            assert!(toks(snake).contains(&t), "{t}");
        }
        for t in query_terms("partial build indexing") {
            assert!(toks(snake).contains(&t), "{t}");
        }
    }

    #[test]
    fn first_match_names_the_run_a_wanted_token_came_from() {
        let mut tk = Tokenizer::new();
        let text = "let x = 1; parseJsonConfig(path) // see config";
        let at = tk.first_match(text, |t| t == "json").unwrap();
        assert_eq!(&text[at], "parseJsonConfig");
        let at = tk.first_match(text, |t| t == "config").unwrap();
        assert_eq!(&text[at], "parseJsonConfig", "the earliest run wins");
        // A run that ends the text counts too.
        let at = tk.first_match("tail config", |t| t == "config").unwrap();
        assert_eq!(&"tail config"[at], "config");
        assert_eq!(tk.first_match(text, |t| t == "absent"), None);
        assert_eq!(tk.first_match("", |_| true), None);
        assert_eq!(tk.first_match("日本語 x", |t| t == "日本語"), Some(0..9));
    }

    #[test]
    fn short_numbers_and_hex_like_words_survive() {
        assert_eq!(toks("2024"), ["2024"]);
        assert_eq!(toks("port 8080"), ["port", "8080"]);
        assert_eq!(toks("0xff"), ["0xff", "xff"]);
        assert_eq!(toks("facade"), ["facade"]);
    }

    #[test]
    fn query_terms_dedupe_in_order_and_cap() {
        assert_eq!(
            query_terms("Parse JSON parse json config"),
            ["parse", "json", "config"]
        );
        assert_eq!(
            query_terms("parseJsonConfig"),
            ["parsejsonconfig", "parse", "json", "config"]
        );
        assert!(query_terms("   ").is_empty());
        assert!(query_terms("the of a").is_empty());
        let many: String = (0..200).map(|i| format!("word{i}x ")).collect();
        assert_eq!(query_terms(&many).len(), MAX_QUERY_TERMS);
    }

    #[test]
    fn json_config_query_terms_match_a_camel_case_identifier() {
        let doc = toks("fn parseJsonConfig(path: &Path)");
        for t in query_terms("json config") {
            assert!(doc.contains(&t), "{t} missing from {doc:?}");
        }
    }
}
