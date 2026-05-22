//! Tree-sitter based syntax highlighting.
//!
//! First slice of the syntect → tree-sitter migration. Exposes a single
//! `highlight()` function that turns a source string into a flat list of
//! class-tagged byte ranges. The editor will consume these spans and map
//! `class` indices into theme colors (next step — not wired here).
//!
//! Languages: Rust and Python. Extending: add a `*_config()` constructor,
//! a `SyntaxLang` variant, and a `from_extension()` arm.

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Canonical highlight class names. Their position is the index returned
/// inside `HighlightSpan::class`. The editor's theme module maps these names
/// to palette colors.
pub const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "function",
    "function.builtin",
    "function.macro",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// A contiguous run of source bytes carrying a single highlight class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    /// Index into `HIGHLIGHT_NAMES`; `None` for unstyled source.
    pub class: Option<usize>,
}

/// Supported source languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxLang {
    Rust,
    Python,
}

impl SyntaxLang {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "py" | "pyi" => Some(Self::Python),
            _ => None,
        }
    }
}

fn rust_config() -> Option<HighlightConfiguration> {
    let mut config = HighlightConfiguration::new(
        tree_sitter_rust::LANGUAGE.into(),
        "rust",
        tree_sitter_rust::HIGHLIGHTS_QUERY,
        tree_sitter_rust::INJECTIONS_QUERY,
        "",
    )
    .ok()?;
    config.configure(HIGHLIGHT_NAMES);
    Some(config)
}

fn python_config() -> Option<HighlightConfiguration> {
    let mut config = HighlightConfiguration::new(
        tree_sitter_python::LANGUAGE.into(),
        "python",
        tree_sitter_python::HIGHLIGHTS_QUERY,
        "",
        "",
    )
    .ok()?;
    config.configure(HIGHLIGHT_NAMES);
    Some(config)
}

/// Highlight a source string into class-tagged spans. Returns an empty Vec
/// on any tree-sitter error so callers can safely fall back to plain text.
pub fn highlight(source: &str, lang: SyntaxLang) -> Vec<HighlightSpan> {
    let Some(config) = (match lang {
        SyntaxLang::Rust => rust_config(),
        SyntaxLang::Python => python_config(),
    }) else {
        return Vec::new();
    };

    let mut highlighter = Highlighter::new();
    let bytes = source.as_bytes();
    let events = match highlighter.highlight(&config, bytes, None, |_| None) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for event in events {
        match event {
            Ok(HighlightEvent::HighlightStart(h)) => stack.push(h.0),
            Ok(HighlightEvent::HighlightEnd) => {
                stack.pop();
            }
            Ok(HighlightEvent::Source { start, end }) => {
                spans.push(HighlightSpan {
                    start,
                    end,
                    class: stack.last().copied(),
                });
            }
            Err(_) => return Vec::new(),
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_from_extension_maps_known_languages() {
        assert_eq!(SyntaxLang::from_extension("rs"), Some(SyntaxLang::Rust));
        assert_eq!(SyntaxLang::from_extension("RS"), Some(SyntaxLang::Rust));
        assert_eq!(SyntaxLang::from_extension("py"), Some(SyntaxLang::Python));
        assert_eq!(SyntaxLang::from_extension("pyi"), Some(SyntaxLang::Python));
        assert_eq!(SyntaxLang::from_extension("txt"), None);
    }

    #[test]
    fn rust_source_yields_keyword_span() {
        let spans = highlight("fn main() {}", SyntaxLang::Rust);
        assert!(!spans.is_empty(), "expected spans for valid rust source");
        let kw_idx = HIGHLIGHT_NAMES
            .iter()
            .position(|n| *n == "keyword")
            .unwrap();
        assert!(
            spans.iter().any(|s| s.class == Some(kw_idx)),
            "expected at least one `keyword` span, got: {spans:?}"
        );
    }

    #[test]
    fn python_source_yields_some_classified_spans() {
        let spans = highlight("def foo():\n    return 1\n", SyntaxLang::Python);
        assert!(!spans.is_empty(), "expected spans for valid python source");
        assert!(
            spans.iter().any(|s| s.class.is_some()),
            "expected at least one classified span"
        );
    }
}
