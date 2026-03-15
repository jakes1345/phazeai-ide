use std::collections::HashMap;

use super::vscode_assets::TokenColorRule;

/// An RGBA color as (red, green, blue, alpha) bytes, each in 0–255.
///
/// This is the core-layer color type. The UI layer converts these to
/// `floem::peniko::Color` via `Color::from_rgba8(r, g, b, a)`.
pub type RgbaColor = (u8, u8, u8, u8);

/// Maps VSCode `colors` object keys to PhazePalette-compatible RGBA colors.
///
/// Returns a map of our palette field names to RGBA colors.
/// Unmapped fields should fall back to the active built-in theme.
pub fn convert_workbench_colors(colors: &HashMap<String, String>) -> HashMap<String, RgbaColor> {
    let mut result = HashMap::new();

    // VSCode workbench key → our PhazePalette field name
    let mappings: &[(&str, &str)] = &[
        ("editor.background", "bg_base"),
        ("editor.foreground", "text_primary"),
        ("sideBar.background", "bg_panel"),
        ("activityBar.background", "bg_deep"),
        ("editorGroupHeader.tabsBackground", "bg_surface"),
        ("tab.activeBackground", "bg_elevated"),
        ("statusBar.background", "bg_deep"),
        ("editor.selectionBackground", "selection"),
        ("editor.lineHighlightBackground", "bg_elevated"),
        ("editorLineNumber.foreground", "text_muted"),
        ("editorCursor.foreground", "accent"),
        ("focusBorder", "glass_border"),
        ("badge.background", "accent"),
        ("errorForeground", "error"),
        ("editorWarning.foreground", "warning"),
    ];

    for (vscode_key, palette_key) in mappings {
        if let Some(hex) = colors.get(*vscode_key) {
            if let Some(color) = parse_hex_color(hex) {
                // Only insert first match — don't overwrite an already-mapped field.
                result.entry(palette_key.to_string()).or_insert(color);
            }
        }
    }

    result
}

/// Maps VSCode tokenColors rules to our syntax color palette fields.
///
/// Returns a map of `syn_*` palette field names to RGBA colors.
/// First TextMate scope match wins; subsequent matches for the same field
/// are ignored so that more-specific rules in the theme take priority.
pub fn extract_syntax_colors(token_colors: &[TokenColorRule]) -> HashMap<String, RgbaColor> {
    let mut result = HashMap::new();

    // TextMate scope prefixes → our syn_* palette field name.
    // Ordering matters: more specific scopes should appear before broader ones
    // when they map to the same field.
    let scope_mappings: &[(&[&str], &str)] = &[
        (
            &["keyword", "keyword.control", "storage.type", "storage.modifier"],
            "syn_keyword",
        ),
        (&["string", "string.quoted", "string.template"], "syn_string"),
        (&["comment", "comment.line", "comment.block"], "syn_comment"),
        (
            &[
                "entity.name.function",
                "support.function",
                "meta.function-call",
            ],
            "syn_function",
        ),
        (
            &["constant.numeric", "constant.language", "constant.character"],
            "syn_number",
        ),
        (
            &[
                "entity.name.type",
                "support.type",
                "support.class",
                "entity.name.class",
            ],
            "syn_type",
        ),
        (
            &["keyword.operator", "punctuation.separator", "punctuation.accessor"],
            "syn_operator",
        ),
        (
            &[
                "entity.name.tag",
                "meta.preprocessor",
                "support.other.macro",
                "meta.attribute",
            ],
            "syn_macro",
        ),
        (
            &[
                "variable",
                "variable.other",
                "variable.parameter",
            ],
            "syn_variable",
        ),
        (
            &["string.regexp", "constant.regexp"],
            "syn_string",
        ),
    ];

    for rule in token_colors {
        // Only process rules that have a foreground color
        let fg = match rule.settings.foreground.as_deref() {
            Some(s) => s,
            None => continue,
        };
        let color = match parse_hex_color(fg) {
            Some(c) => c,
            None => continue,
        };

        // Apply to every scope this rule declares
        if let Some(ref scope_sel) = rule.scope {
            for scope_str in scope_sel.scopes() {
                let trimmed = scope_str.trim();
                for (patterns, field) in scope_mappings {
                    if patterns.iter().any(|p| trimmed.starts_with(p)) {
                        // First match wins
                        result.entry(field.to_string()).or_insert(color);
                    }
                }
            }
        }
    }

    result
}

/// Parse a hex color string (#RGB, #RGBA, #RRGGBB, #RRGGBBAA) into an RGBA tuple.
///
/// Returns `None` if the string is not a recognised hex color format.
pub fn parse_hex_color(hex: &str) -> Option<RgbaColor> {
    let trimmed = hex.trim();
    if !trimmed.starts_with('#') {
        return None;
    }
    let hex = &trimmed[1..];
    match hex.len() {
        3 => {
            // #RGB → expand each nibble to a byte (multiply by 17 = 0x11)
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            Some((r, g, b, 255))
        }
        4 => {
            // #RGBA
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            let a = u8::from_str_radix(&hex[3..4], 16).ok()? * 17;
            Some((r, g, b, a))
        }
        6 => {
            // #RRGGBB
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b, 255))
        }
        8 => {
            // #RRGGBBAA
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
            Some((r, g, b, a))
        }
        _ => None,
    }
}
