use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Parsed VSCode extension package.json
#[derive(Debug, Clone, Deserialize)]
pub struct VsixManifest {
    pub name: String,
    pub publisher: Option<String>,
    pub version: String,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub categories: Option<Vec<String>>,
    pub contributes: Option<VsixContributes>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VsixContributes {
    #[serde(default)]
    pub languages: Vec<LanguageContribution>,
    #[serde(default)]
    pub grammars: Vec<GrammarContribution>,
    #[serde(default)]
    pub themes: Vec<ThemeContribution>,
    #[serde(default)]
    pub snippets: Vec<SnippetContribution>,
    #[serde(default, rename = "iconThemes")]
    pub icon_themes: Vec<IconThemeContribution>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LanguageContribution {
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(rename = "configuration")]
    pub configuration_path: Option<String>,
    #[serde(rename = "firstLine")]
    pub first_line: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GrammarContribution {
    pub language: Option<String>,
    #[serde(rename = "scopeName")]
    pub scope_name: String,
    pub path: String,
    #[serde(default, rename = "embeddedLanguages")]
    pub embedded_languages: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThemeContribution {
    pub label: String,
    #[serde(rename = "uiTheme")]
    pub ui_theme: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SnippetContribution {
    pub language: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IconThemeContribution {
    pub id: String,
    pub label: String,
    pub path: String,
}

// ── VSCode Theme File ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct VscodeThemeFile {
    #[serde(rename = "type")]
    pub theme_type: Option<String>,
    #[serde(default)]
    pub colors: HashMap<String, String>,
    #[serde(default, rename = "tokenColors")]
    pub token_colors: Vec<TokenColorRule>,
    #[serde(default, rename = "semanticHighlighting")]
    pub semantic_highlighting: Option<bool>,
    #[serde(default, rename = "semanticTokenColors")]
    pub semantic_token_colors: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenColorRule {
    pub name: Option<String>,
    pub scope: Option<ScopeSelector>,
    pub settings: TokenColorSettings,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ScopeSelector {
    Single(String),
    Multiple(Vec<String>),
}

impl ScopeSelector {
    pub fn scopes(&self) -> Vec<&str> {
        match self {
            ScopeSelector::Single(s) => s.split(',').map(|s| s.trim()).collect(),
            ScopeSelector::Multiple(v) => v.iter().map(|s| s.as_str()).collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenColorSettings {
    pub foreground: Option<String>,
    pub background: Option<String>,
    #[serde(rename = "fontStyle")]
    pub font_style: Option<String>,
}

// ── Language Configuration ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LanguageConfiguration {
    pub comments: Option<CommentConfig>,
    #[serde(default)]
    pub brackets: Vec<(String, String)>,
    #[serde(default, rename = "autoClosingPairs")]
    pub auto_closing_pairs: Vec<AutoClosePair>,
    #[serde(default, rename = "surroundingPairs")]
    pub surrounding_pairs: Vec<SurroundingPair>,
    pub folding: Option<FoldingConfig>,
    #[serde(rename = "wordPattern")]
    pub word_pattern: Option<String>,
    #[serde(rename = "indentationRules")]
    pub indentation_rules: Option<IndentationRules>,
    #[serde(default, rename = "colorizedBracketPairs")]
    pub colorized_bracket_pairs: Vec<(String, String)>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommentConfig {
    #[serde(rename = "lineComment")]
    pub line_comment: Option<String>,
    #[serde(rename = "blockComment")]
    pub block_comment: Option<(String, String)>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AutoClosePair {
    Simple((String, String)),
    Detailed {
        open: String,
        close: String,
        #[serde(default, rename = "notIn")]
        not_in: Vec<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SurroundingPair {
    Array((String, String)),
    Object { open: String, close: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct FoldingConfig {
    pub markers: Option<FoldingMarkers>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FoldingMarkers {
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndentationRules {
    #[serde(rename = "increaseIndentPattern")]
    pub increase_indent_pattern: Option<String>,
    #[serde(rename = "decreaseIndentPattern")]
    pub decrease_indent_pattern: Option<String>,
    #[serde(rename = "indentNextLinePattern")]
    pub indent_next_line_pattern: Option<String>,
    #[serde(rename = "unIndentedLinePattern")]
    pub un_indented_line_pattern: Option<String>,
}

// ── Snippet File ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SnippetEntry {
    pub prefix: SnippetPrefix,
    pub body: SnippetBody,
    pub description: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SnippetPrefix {
    Single(String),
    Multiple(Vec<String>),
}

impl SnippetPrefix {
    pub fn triggers(&self) -> Vec<&str> {
        match self {
            SnippetPrefix::Single(s) => vec![s.as_str()],
            SnippetPrefix::Multiple(v) => v.iter().map(|s| s.as_str()).collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SnippetBody {
    Single(String),
    Lines(Vec<String>),
}

impl SnippetBody {
    pub fn text(&self) -> String {
        match self {
            SnippetBody::Single(s) => s.clone(),
            SnippetBody::Lines(lines) => lines.join("\n"),
        }
    }
}
