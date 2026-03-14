use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::vscode_assets::*;

/// Represents an installed VSCode extension with parsed assets.
#[derive(Debug, Clone)]
pub struct InstalledExtension {
    pub manifest: VsixManifest,
    /// Root directory where the extension files are stored.
    pub root_dir: PathBuf,
    /// Parsed language configurations, keyed by language ID.
    pub language_configs: HashMap<String, LanguageConfiguration>,
    /// Parsed theme files, keyed by theme label.
    pub themes: HashMap<String, VscodeThemeFile>,
    /// Parsed snippets, keyed by language ID.
    pub snippets: HashMap<String, Vec<SnippetEntry>>,
}

/// The extensions install directory: ~/.phazeai/extensions/
fn extensions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".phazeai")
        .join("extensions")
}

/// Install a .vsix file by extracting it and parsing its manifest.
pub fn install_vsix(vsix_path: &Path) -> Result<InstalledExtension, String> {
    let file = std::fs::File::open(vsix_path)
        .map_err(|e| format!("Failed to open VSIX file: {}", e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Failed to read VSIX ZIP: {}", e))?;

    // Read package.json from the archive
    let manifest = read_manifest_from_zip(&mut archive)?;

    // Determine install directory
    let ext_id = format!(
        "{}.{}-{}",
        manifest.publisher.as_deref().unwrap_or("unknown"),
        manifest.name,
        manifest.version
    );
    let install_dir = extensions_dir().join(&ext_id);
    let _ = std::fs::create_dir_all(&install_dir);

    // Extract all files from the "extension/" prefix in the VSIX
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read ZIP entry: {}", e))?;
        let name = entry.name().to_string();

        // VSIX files have entries under "extension/" prefix
        let rel_path = if let Some(stripped) = name.strip_prefix("extension/") {
            stripped.to_string()
        } else {
            name.clone()
        };

        if rel_path.is_empty() || name.ends_with('/') {
            // Directory entry
            let dir_path = install_dir.join(&rel_path);
            let _ = std::fs::create_dir_all(dir_path);
            continue;
        }

        let out_path = install_dir.join(&rel_path);
        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("Failed to read entry '{}': {}", name, e))?;
        std::fs::write(&out_path, &buf)
            .map_err(|e| format!("Failed to write '{}': {}", out_path.display(), e))?;
    }

    // Now load all assets from the installed directory
    load_extension(&install_dir)
}

/// Load an already-extracted extension from a directory containing package.json.
pub fn load_extension(dir: &Path) -> Result<InstalledExtension, String> {
    let manifest_path = dir.join("package.json");
    let manifest_str = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read package.json: {}", e))?;
    let manifest: VsixManifest = serde_json::from_str(&manifest_str)
        .map_err(|e| format!("Invalid package.json: {}", e))?;

    let contributes = manifest.contributes.as_ref();

    // Load language configurations
    let mut language_configs = HashMap::new();
    if let Some(c) = contributes {
        for lang in &c.languages {
            if let Some(config_path) = &lang.configuration_path {
                let full_path = dir.join(config_path);
                if full_path.exists() {
                    match load_language_config(&full_path) {
                        Ok(config) => {
                            language_configs.insert(lang.id.clone(), config);
                        }
                        Err(e) => tracing::warn!(
                            "Failed to load language config for {}: {}",
                            lang.id,
                            e
                        ),
                    }
                }
            }
        }
    }

    // Load themes
    let mut themes = HashMap::new();
    if let Some(c) = contributes {
        for theme_contrib in &c.themes {
            let full_path = dir.join(&theme_contrib.path);
            if full_path.exists() {
                match load_theme_file(&full_path) {
                    Ok(theme) => {
                        themes.insert(theme_contrib.label.clone(), theme);
                    }
                    Err(e) => tracing::warn!(
                        "Failed to load theme '{}': {}",
                        theme_contrib.label,
                        e
                    ),
                }
            }
        }
    }

    // Load snippets
    let mut snippets: HashMap<String, Vec<SnippetEntry>> = HashMap::new();
    if let Some(c) = contributes {
        for snippet_contrib in &c.snippets {
            let full_path = dir.join(&snippet_contrib.path);
            if full_path.exists() {
                match load_snippet_file(&full_path) {
                    Ok(entries) => {
                        snippets
                            .entry(snippet_contrib.language.clone())
                            .or_default()
                            .extend(entries);
                    }
                    Err(e) => tracing::warn!(
                        "Failed to load snippets for {}: {}",
                        snippet_contrib.language,
                        e
                    ),
                }
            }
        }
    }

    Ok(InstalledExtension {
        manifest,
        root_dir: dir.to_path_buf(),
        language_configs,
        themes,
        snippets,
    })
}

/// Read package.json from inside a VSIX ZIP archive.
fn read_manifest_from_zip(
    archive: &mut zip::ZipArchive<std::fs::File>,
) -> Result<VsixManifest, String> {
    // Try both "extension/package.json" and "package.json"
    let possible_paths = ["extension/package.json", "package.json"];

    for path in &possible_paths {
        if let Ok(mut entry) = archive.by_name(path) {
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|e| format!("Failed to read {}: {}", path, e))?;
            return serde_json::from_str(&buf)
                .map_err(|e| format!("Invalid package.json: {}", e));
        }
    }

    Err("No package.json found in VSIX archive".to_string())
}

/// Parse a VSCode theme JSON file (supports JSONC with comments).
pub fn load_theme_file(path: &Path) -> Result<VscodeThemeFile, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read theme file: {}", e))?;

    // VSCode theme files often have comments (JSONC). Strip them.
    let cleaned = strip_json_comments(&content);

    serde_json::from_str(&cleaned).map_err(|e| format!("Invalid theme JSON: {}", e))
}

/// Parse a language-configuration.json file (supports JSONC with comments).
pub fn load_language_config(path: &Path) -> Result<LanguageConfiguration, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read language config: {}", e))?;
    let cleaned = strip_json_comments(&content);
    serde_json::from_str(&cleaned).map_err(|e| format!("Invalid language configuration: {}", e))
}

/// Parse a VSCode snippets JSON file (supports JSONC with comments).
pub fn load_snippet_file(path: &Path) -> Result<Vec<SnippetEntry>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read snippet file: {}", e))?;
    let cleaned = strip_json_comments(&content);
    let map: HashMap<String, SnippetEntry> = serde_json::from_str(&cleaned)
        .map_err(|e| format!("Invalid snippet JSON: {}", e))?;
    Ok(map.into_values().collect())
}

/// Scan ~/.phazeai/extensions/ for installed extensions and load each one.
pub fn scan_installed_extensions() -> Vec<InstalledExtension> {
    let dir = extensions_dir();
    if !dir.exists() {
        return Vec::new();
    }

    let mut extensions = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("package.json").exists() {
                match load_extension(&path) {
                    Ok(ext) => extensions.push(ext),
                    Err(e) => tracing::warn!(
                        "Failed to load extension from {}: {}",
                        path.display(),
                        e
                    ),
                }
            }
        }
    }
    extensions
}

/// Strip single-line (`//`) and multi-line (`/* */`) comments from JSONC.
///
/// VSCode JSON files commonly use JSONC format with comments. This parser
/// is state-machine based and correctly handles comments inside strings,
/// escaped characters, and nested structures.
pub fn strip_json_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape_next = false;

    while let Some(c) = chars.next() {
        if escape_next {
            output.push(c);
            escape_next = false;
            continue;
        }

        if in_string {
            output.push(c);
            match c {
                '\\' => escape_next = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        // Not in a string
        match c {
            '"' => {
                in_string = true;
                output.push(c);
            }
            '/' => match chars.peek() {
                Some('/') => {
                    // Single-line comment — consume to end of line (keep the newline)
                    chars.next(); // consume second '/'
                    for nc in chars.by_ref() {
                        if nc == '\n' {
                            output.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    // Multi-line comment — consume until */
                    chars.next(); // consume '*'
                    let mut prev = ' ';
                    for nc in chars.by_ref() {
                        if prev == '*' && nc == '/' {
                            break;
                        }
                        // Preserve newlines so line numbers stay correct
                        if nc == '\n' {
                            output.push('\n');
                        }
                        prev = nc;
                    }
                }
                _ => {
                    output.push(c);
                }
            },
            _ => {
                output.push(c);
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_line_comment() {
        let input = r#"{"a": 1 // comment
, "b": 2}"#;
        let out = strip_json_comments(input);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn strip_block_comment() {
        let input = r#"{"a": /* block */ 42}"#;
        let out = strip_json_comments(input);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], 42);
    }

    #[test]
    fn comment_in_string_preserved() {
        let input = r#"{"url": "https://example.com/path"}"#;
        let out = strip_json_comments(input);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "https://example.com/path");
    }

    #[test]
    fn escaped_quote_in_string() {
        let input = r#"{"s": "he said \"hello\" // not a comment"}"#;
        let out = strip_json_comments(input);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["s"], "he said \"hello\" // not a comment");
    }

    #[test]
    fn parse_hex_color_formats() {
        use super::super::theme_convert::parse_hex_color;
        assert_eq!(parse_hex_color("#fff"), Some((255, 255, 255, 255)));
        assert_eq!(parse_hex_color("#ffffff"), Some((255, 255, 255, 255)));
        assert_eq!(parse_hex_color("#ff000080"), Some((255, 0, 0, 128)));
        assert_eq!(parse_hex_color("#ffff"), Some((255, 255, 255, 255)));
        assert_eq!(parse_hex_color("bad"), None);
    }
}
