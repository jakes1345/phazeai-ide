//! User themes loaded from `~/.config/phazeai/themes/*.toml`.
//!
//! A theme file starts from a built-in theme and overrides any palette colour:
//!
//! ```toml
//! name = "Ember"
//! base = "Tokyo Night"
//!
//! [colors]
//! accent = "#ff7a18"
//! bg_base = "#14110f"
//! selection = "#ff7a1840"   # #rrggbbaa for transparency
//! ```
//!
//! Every key under `[colors]` is a field of [`PhazePalette`]; unknown keys and
//! malformed colours are reported instead of being silently ignored. The
//! Settings panel can export the current theme as a complete file to start from.

use std::path::{Path, PathBuf};

use floem::peniko::Color;
use serde::Deserialize;

use crate::theme::{PhazePalette, PhazeTheme, ThemeVariant};

/// Lists every palette colour once so that reading, writing and validating
/// theme files can never drift from the struct definition.
macro_rules! palette_colors {
    ($($field:ident),* $(,)?) => {
        impl PhazePalette {
            /// Every palette colour with its theme-file key, in declaration order.
            pub fn colors(&self) -> Vec<(&'static str, Color)> {
                vec![$((stringify!($field), self.$field)),*]
            }

            /// Set the colour named `key`. Returns false for an unknown key.
            pub fn set_color(&mut self, key: &str, color: Color) -> bool {
                match key {
                    $(stringify!($field) => self.$field = color,)*
                    _ => return false,
                }
                true
            }
        }
    };
}

palette_colors!(
    bg_deep,
    bg_base,
    bg_surface,
    bg_panel,
    bg_elevated,
    text_primary,
    text_secondary,
    text_muted,
    text_disabled,
    accent,
    accent_hover,
    accent_dim,
    success,
    warning,
    error,
    info,
    git_added,
    git_modified,
    git_deleted,
    git_untracked,
    border,
    border_focus,
    selection,
    syn_keyword,
    syn_string,
    syn_comment,
    syn_function,
    syn_number,
    syn_type,
    syn_operator,
    syn_macro,
    diag_error,
    diag_warning,
    diag_info,
    diag_hint,
    diff_added_fg,
    diff_added_bg,
    diff_removed_fg,
    diff_removed_bg,
    diff_header_fg,
    diff_header_bg,
    bracket_1,
    bracket_2,
    bracket_3,
    bracket_4,
    find_match_bg,
    find_match_border,
    indent_guide,
    inlay_hint,
    matching_bracket_bg,
    cursor_line_bg,
    minimap_bar,
    overlay_bg,
    overlay_bg_light,
    button_primary_bg,
    button_primary_fg,
    button_hover_bg,
    glass_bg,
    glass_border,
    glow,
);

/// `#rgb`, `#rrggbb` or `#rrggbbaa` (the `#` is optional).
pub fn parse_hex_color(s: &str) -> Option<Color> {
    let hex = s.trim().trim_start_matches('#');
    if !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    match hex.len() {
        3 => {
            let mut c = [0u8; 3];
            for (i, ch) in hex.chars().enumerate() {
                let v = ch.to_digit(16)? as u8;
                c[i] = v * 17;
            }
            Some(Color::from_rgb8(c[0], c[1], c[2]))
        }
        6 => Some(Color::from_rgb8(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color::from_rgba8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

/// `#rrggbb`, or `#rrggbbaa` when the colour is not fully opaque.
pub fn color_to_hex(color: Color) -> String {
    let c = color.to_rgba8();
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    colors: std::collections::BTreeMap<String, String>,
}

/// Parse one theme file's contents.
pub fn parse_theme(text: &str) -> Result<PhazeTheme, String> {
    let file: ThemeFile = toml::from_str(text).map_err(|e| e.message().to_string())?;
    let name = file.name.trim().to_string();
    if name.is_empty() {
        return Err("`name` must not be empty".to_string());
    }
    if ThemeVariant::all()
        .iter()
        .any(|v| v.name().eq_ignore_ascii_case(&name))
    {
        return Err(format!("`{name}` is the name of a built-in theme"));
    }
    let variant = match file.base.as_deref() {
        None => ThemeVariant::Dark,
        Some(base) => {
            // `from_name` falls back to Dark for names it doesn't know.
            let v = ThemeVariant::from_name(base);
            let is_dark = base.trim().eq_ignore_ascii_case("dark");
            if v == ThemeVariant::Dark && !is_dark {
                return Err(format!("unknown base theme `{base}`"));
            }
            v
        }
    };
    let mut theme = PhazeTheme::from_variant(variant);
    for (key, value) in &file.colors {
        let color = parse_hex_color(value)
            .ok_or_else(|| format!("colors.{key}: `{value}` is not a #rrggbb colour"))?;
        if !theme.palette.set_color(key, color) {
            return Err(format!("colors.{key}: no such palette colour"));
        }
    }
    theme.custom_name = Some(name);
    Ok(theme)
}

/// A complete theme file for `theme`, saved under `name`.
pub fn theme_to_toml(theme: &PhazeTheme, name: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("name = {}\n", toml_string(name)));
    out.push_str(&format!("base = {}\n\n", toml_string(theme.variant.name())));
    out.push_str("# Delete any line to inherit it from the base theme.\n[colors]\n");
    for (key, color) in theme.palette.colors() {
        out.push_str(&format!("{key} = \"{}\"\n", color_to_hex(color)));
    }
    out
}

fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// `~/.config/phazeai/themes`.
pub fn themes_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("phazeai")
        .join("themes")
}

/// Themes that loaded, and `(file, problem)` for those that didn't.
#[derive(Default)]
pub struct LoadedThemes {
    pub themes: Vec<PhazeTheme>,
    pub errors: Vec<(PathBuf, String)>,
}

/// Every `*.toml` theme in `dir` (the extension in any case), sorted by name.
/// Later duplicates of a name are reported as errors.
pub fn load_themes_from(dir: &Path) -> LoadedThemes {
    let mut loaded = LoadedThemes::default();
    let Ok(read) = std::fs::read_dir(dir) else {
        return loaded;
    };
    let mut paths: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
        })
        .collect();
    paths.sort();
    for path in paths {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| parse_theme(&text));
        match parsed {
            Ok(theme) => {
                let name = theme.display_name().to_string();
                if loaded
                    .themes
                    .iter()
                    .any(|t| t.display_name().eq_ignore_ascii_case(&name))
                {
                    loaded
                        .errors
                        .push((path, format!("another theme is already named `{name}`")));
                } else {
                    loaded.themes.push(theme);
                }
            }
            Err(e) => loaded.errors.push((path, e)),
        }
    }
    loaded
        .themes
        .sort_by_key(|t| t.display_name().to_ascii_lowercase());
    loaded
}

/// User themes from [`themes_dir`].
pub fn load_custom_themes() -> LoadedThemes {
    let loaded = load_themes_from(&themes_dir());
    for (path, err) in &loaded.errors {
        tracing::warn!(target: "phazeai_ui", "theme {}: {err}", path.display());
    }
    loaded
}

/// The user theme named `name` (case-insensitive), if there is one.
pub fn find_custom_theme(name: &str) -> Option<PhazeTheme> {
    let name = name.trim();
    if ThemeVariant::all()
        .iter()
        .any(|v| v.name().eq_ignore_ascii_case(name))
    {
        return None;
    }
    load_themes_from(&themes_dir())
        .themes
        .into_iter()
        .find(|t| t.display_name().eq_ignore_ascii_case(name))
}

/// Write `theme` to the themes folder as a new file named after `name`,
/// never overwriting an existing file. Returns the path written.
pub fn export_theme(theme: &PhazeTheme, name: &str) -> std::io::Result<PathBuf> {
    let dir = themes_dir();
    std::fs::create_dir_all(&dir)?;
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let stem = if stem.is_empty() {
        "theme".to_string()
    } else {
        stem
    };
    let path = std::iter::once(dir.join(format!("{stem}.toml")))
        .chain((2..).map(|n| dir.join(format!("{stem}-{n}.toml"))))
        .find(|p| !p.exists())
        .expect("an unused file name exists");
    std::fs::write(&path, theme_to_toml(theme, name))?;
    Ok(path)
}

/// A name for an exported copy of `theme` that no other theme uses.
pub fn export_name_for(theme: &PhazeTheme) -> String {
    let taken = load_custom_themes();
    let base = format!("My {}", theme.display_name().trim_start_matches("My "));
    std::iter::once(base.clone())
        .chain((2..).map(|n| format!("{base} {n}")))
        .find(|n| {
            !taken
                .themes
                .iter()
                .any(|t| t.display_name().eq_ignore_ascii_case(n))
        })
        .expect("an unused name exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors_parse_and_round_trip() {
        assert_eq!(
            parse_hex_color("#fff"),
            Some(Color::from_rgb8(255, 255, 255))
        );
        assert_eq!(
            parse_hex_color("102030"),
            Some(Color::from_rgb8(16, 32, 48))
        );
        assert_eq!(
            parse_hex_color("#10203080"),
            Some(Color::from_rgba8(16, 32, 48, 128))
        );
        assert_eq!(parse_hex_color("#12"), None);
        assert_eq!(parse_hex_color("#gggggg"), None);
        assert_eq!(color_to_hex(Color::from_rgb8(16, 32, 48)), "#102030");
        assert_eq!(
            color_to_hex(Color::from_rgba8(16, 32, 48, 128)),
            "#10203080"
        );
    }

    #[test]
    fn palette_keys_cover_every_colour() {
        let mut p = PhazePalette::dark();
        let keys: Vec<&str> = p.colors().iter().map(|(k, _)| *k).collect();
        assert_eq!(keys.len(), 60);
        for key in keys {
            assert!(p.set_color(key, Color::from_rgb8(1, 2, 3)), "{key}");
        }
        assert!(p
            .colors()
            .iter()
            .all(|(_, c)| *c == Color::from_rgb8(1, 2, 3)));
        assert!(!p.set_color("not_a_colour", Color::BLACK));
    }

    #[test]
    fn theme_file_overrides_base() {
        let t = parse_theme(
            "name = \"Ember\"\nbase = \"Tokyo Night\"\n[colors]\naccent = \"#ff7a18\"\n",
        )
        .unwrap();
        assert_eq!(t.display_name(), "Ember");
        assert_eq!(t.variant, ThemeVariant::TokyoNight);
        assert_eq!(t.palette.accent, Color::from_rgb8(0xff, 0x7a, 0x18));
        let base = PhazeTheme::from_variant(ThemeVariant::TokyoNight);
        assert_eq!(t.palette.bg_base, base.palette.bg_base);
    }

    #[test]
    fn theme_file_errors_are_specific() {
        let bad_key = parse_theme("name = \"X\"\n[colors]\naccnet = \"#fff\"\n").unwrap_err();
        assert!(bad_key.contains("accnet"), "{bad_key}");
        let bad_color = parse_theme("name = \"X\"\n[colors]\naccent = \"red\"\n").unwrap_err();
        assert!(bad_color.contains("accent"), "{bad_color}");
        let bad_base = parse_theme("name = \"X\"\nbase = \"Nope\"\n").unwrap_err();
        assert!(bad_base.contains("Nope"), "{bad_base}");
        assert!(parse_theme("name = \"Dracula\"\n").is_err());
        assert!(parse_theme("name = \"X\"\ncolours = 1\n").is_err());
    }

    #[test]
    fn exported_theme_parses_back_identically() {
        let original = PhazeTheme::from_variant(ThemeVariant::Synthwave84);
        let text = theme_to_toml(&original, "My \"Synth\"");
        let back = parse_theme(&text).unwrap();
        assert_eq!(back.display_name(), "My \"Synth\"");
        assert_eq!(back.variant, ThemeVariant::Synthwave84);
        // Files store 8-bit channels, so compare at that precision.
        let rgba8 = |t: &PhazeTheme| -> Vec<_> {
            t.palette
                .colors()
                .into_iter()
                .map(|(k, c)| (k, c.to_rgba8()))
                .collect()
        };
        assert_eq!(rgba8(&back), rgba8(&original));
    }

    #[test]
    fn folder_loading_reports_bad_files_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.toml"), "name = \"Zed\"\n").unwrap();
        std::fs::write(dir.path().join("b.toml"), "name = \"zed\"\n").unwrap();
        std::fs::write(dir.path().join("c.toml"), "name = \"Alpha\"\n").unwrap();
        std::fs::write(dir.path().join("d.toml"), "oops").unwrap();
        std::fs::write(dir.path().join("e.txt"), "name = \"Ignored\"\n").unwrap();
        // Files saved by tools that upper-case extensions still count.
        std::fs::write(dir.path().join("upper.TOML"), "name = \"Upper\"\n").unwrap();
        let loaded = load_themes_from(dir.path());
        let names: Vec<&str> = loaded.themes.iter().map(|t| t.display_name()).collect();
        assert_eq!(names, vec!["Alpha", "Upper", "Zed"]);
        assert_eq!(loaded.errors.len(), 2);
    }
}
