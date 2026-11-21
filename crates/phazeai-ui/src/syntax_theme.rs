//! Syntax colours from the active theme.
//!
//! The editor highlights with tree-sitter for the languages it has grammars
//! for and syntect for the rest. Both used to paint base16-ocean colours
//! whatever the theme. [`SyntaxColors`] takes the theme's `syn_*` palette
//! colours, and both engines read from it: tree-sitter classes through
//! [`SyntaxColors::for_class`], syntect scopes through [`syntect_theme`].

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

use floem::peniko::Color;
use syntect::highlighting::{
    Color as SynColor, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};

use crate::theme::PhazePalette;

/// The colours code is drawn in, taken from a theme's palette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyntaxColors {
    pub text: Color,
    pub keyword: Color,
    pub string: Color,
    pub comment: Color,
    pub function: Color,
    pub number: Color,
    pub ty: Color,
    pub operator: Color,
    pub macro_: Color,
}

impl SyntaxColors {
    pub fn from_palette(p: &PhazePalette) -> Self {
        Self {
            text: p.text_primary,
            keyword: p.syn_keyword,
            string: p.syn_string,
            comment: p.syn_comment,
            function: p.syn_function,
            number: p.syn_number,
            ty: p.syn_type,
            operator: p.syn_operator,
            macro_: p.syn_macro,
        }
    }

    /// The colour of a tree-sitter highlight class (`syntax::HIGHLIGHT_NAMES`).
    pub fn for_class(&self, name: &str) -> Color {
        match name {
            "keyword" => self.keyword,
            "function.macro" => self.macro_,
            "function" | "function.builtin" => self.function,
            "string" | "string.special" => self.string,
            "comment" => self.comment,
            "type" | "type.builtin" | "constructor" => self.ty,
            "number" | "constant" | "constant.builtin" => self.number,
            "operator" | "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => {
                self.operator
            }
            "attribute" | "label" => self.macro_,
            "tag" => self.keyword,
            // Fields, parameters and plain names stay the text colour: a
            // calm page where structure, not every identifier, has colour.
            _ => self.text,
        }
    }

    fn key(&self) -> [u32; 9] {
        let k = |c: Color| u32::from_be_bytes(c.to_rgba8().to_u8_array());
        [
            k(self.text),
            k(self.keyword),
            k(self.string),
            k(self.comment),
            k(self.function),
            k(self.number),
            k(self.ty),
            k(self.operator),
            k(self.macro_),
        ]
    }
}

fn syn(c: Color) -> SynColor {
    let c = c.to_rgba8();
    SynColor {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

/// A syntect theme in these colours: TextMate scopes mapped onto the same
/// roles as the tree-sitter classes. Built once per set of colours and kept
/// for the life of the program (a syntect highlighter borrows its theme, and
/// there are only as many as there are themes in use).
pub fn syntect_theme(colors: &SyntaxColors) -> &'static Theme {
    static CACHE: Mutex<Option<HashMap<[u32; 9], &'static Theme>>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = cache.get_or_insert_with(HashMap::new);
    let key = colors.key();
    if let Some(theme) = map.get(&key) {
        return theme;
    }
    let theme: &'static Theme = Box::leak(Box::new(build(colors)));
    map.insert(key, theme);
    theme
}

fn build(c: &SyntaxColors) -> Theme {
    let item = |scopes: &str, color: Color, style: Option<FontStyle>| ThemeItem {
        scope: ScopeSelectors::from_str(scopes).expect("valid scope selectors"),
        style: StyleModifier {
            foreground: Some(syn(color)),
            background: None,
            font_style: style,
        },
    };
    Theme {
        name: Some("PhazeAI".into()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(syn(c.text)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            item(
                "comment, punctuation.definition.comment",
                c.comment,
                Some(FontStyle::ITALIC),
            ),
            item(
                "string, constant.character, string punctuation.definition",
                c.string,
                None,
            ),
            item(
                "constant.numeric, constant.language, constant.other",
                c.number,
                None,
            ),
            item(
                "keyword, storage, storage.type, storage.modifier",
                c.keyword,
                None,
            ),
            item("keyword.operator, punctuation", c.operator, None),
            item(
                "entity.name.function, support.function, meta.function-call variable.function",
                c.function,
                None,
            ),
            item(
                "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, \
                 entity.name.trait, support.type, support.class, entity.other.inherited-class",
                c.ty,
                None,
            ),
            item(
                "entity.name.macro, support.macro, meta.macro, meta.attribute, meta.annotation",
                c.macro_,
                None,
            ),
            item("entity.name.tag", c.keyword, None),
            item("entity.other.attribute-name", c.function, None),
            item(
                "markup.heading, entity.name.section",
                c.function,
                Some(FontStyle::BOLD),
            ),
            item("markup.bold", c.text, Some(FontStyle::BOLD)),
            item("markup.italic", c.text, Some(FontStyle::ITALIC)),
            item("markup.inline.raw, markup.raw", c.string, None),
            item("markup.underline.link", c.function, None),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{PhazeTheme, ThemeVariant};
    use syntect::easy::HighlightLines;
    use syntect::parsing::SyntaxSet;

    fn colors() -> SyntaxColors {
        SyntaxColors::from_palette(&PhazeTheme::from_variant(ThemeVariant::Phaze).palette)
    }

    #[test]
    fn tree_sitter_classes_use_the_palette() {
        let c = colors();
        assert_eq!(c.for_class("keyword"), c.keyword);
        assert_eq!(c.for_class("function.macro"), c.macro_);
        assert_eq!(c.for_class("string"), c.string);
        assert_eq!(c.for_class("type.builtin"), c.ty);
        assert_eq!(c.for_class("property"), c.text);
        assert_eq!(c.for_class("unknown.class"), c.text);
    }

    /// Languages without a tree-sitter grammar here (Python is one) are
    /// highlighted by syntect, through [`syntect_theme`].
    #[test]
    fn syntect_highlights_in_the_palette_colours() {
        let c = colors();
        let ps = SyntaxSet::load_defaults_newlines();
        let syntax = ps.find_syntax_by_extension("py").unwrap();
        let mut h = HighlightLines::new(syntax, syntect_theme(&c));
        let mut pieces = Vec::new();
        for line in ["class Shop:\n", "    def total(self): return 1  # sum\n"] {
            for (style, text) in h.highlight_line(line, &ps).unwrap() {
                pieces.push((style.foreground, text.trim().to_string()));
            }
        }
        let color_of = |text: &str| {
            pieces
                .iter()
                .find(|(_, t)| t == text)
                .unwrap_or_else(|| panic!("no piece {text:?} in {pieces:?}"))
                .0
        };
        assert_eq!(color_of("class"), syn(c.keyword));
        assert_eq!(color_of("Shop"), syn(c.ty));
        assert_eq!(color_of("def"), syn(c.keyword));
        assert_eq!(color_of("total"), syn(c.function));
        assert_eq!(color_of("1"), syn(c.number));
        assert_eq!(color_of("sum"), syn(c.comment));
    }

    #[test]
    fn one_theme_per_set_of_colours() {
        let c = colors();
        assert!(std::ptr::eq(syntect_theme(&c), syntect_theme(&c)));
        let other =
            SyntaxColors::from_palette(&PhazeTheme::from_variant(ThemeVariant::Dracula).palette);
        assert!(!std::ptr::eq(syntect_theme(&c), syntect_theme(&other)));
    }
}
