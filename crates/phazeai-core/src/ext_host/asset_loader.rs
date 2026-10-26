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
    install_vsix_into(vsix_path, &extensions_dir())
}

/// [`install_vsix`] into an explicit extensions folder.
///
/// Entry names come from the archive and are untrusted: every one must stay
/// inside the extension's own folder, or the whole install is refused.
fn install_vsix_into(
    vsix_path: &Path,
    extensions_root: &Path,
) -> Result<InstalledExtension, String> {
    let file =
        std::fs::File::open(vsix_path).map_err(|e| format!("Failed to open VSIX file: {}", e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Failed to read VSIX ZIP: {}", e))?;

    // Refuse the whole archive before writing anything if any entry would
    // land outside the extension folder.
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read ZIP entry: {}", e))?;
        if entry.enclosed_name().is_none() {
            return Err(format!(
                "Refusing VSIX: entry '{}' points outside the extension folder",
                String::from_utf8_lossy(entry.name_raw())
            ));
        }
    }

    // Read package.json from the archive
    let manifest = read_manifest_from_zip(&mut archive)?;

    // Determine install directory
    let ext_id = format!(
        "{}.{}-{}",
        manifest.publisher.as_deref().unwrap_or("unknown"),
        manifest.name,
        manifest.version
    );
    let install_dir = extensions_root.join(&ext_id);
    let _ = std::fs::create_dir_all(&install_dir);

    // Extract all files from the "extension/" prefix in the VSIX
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read ZIP entry: {}", e))?;
        // `enclosed_name` rejects absolute paths and `..` components.
        let Some(enclosed) = entry.enclosed_name() else {
            return Err(format!(
                "Refusing VSIX: entry '{}' points outside the extension folder",
                String::from_utf8_lossy(entry.name_raw())
            ));
        };
        let name = enclosed.to_string_lossy().into_owned();

        // VSIX files have entries under the "extension/" prefix.
        let rel_path = enclosed
            .strip_prefix("extension")
            .unwrap_or(&enclosed)
            .to_path_buf();

        if rel_path.as_os_str().is_empty() || entry.is_dir() {
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
    let manifest: VsixManifest =
        serde_json::from_str(&manifest_str).map_err(|e| format!("Invalid package.json: {}", e))?;

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
                        Err(e) => {
                            tracing::warn!("Failed to load language config for {}: {}", lang.id, e)
                        }
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
                    Err(e) => {
                        tracing::warn!("Failed to load theme '{}': {}", theme_contrib.label, e)
                    }
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
            return serde_json::from_str(&buf).map_err(|e| format!("Invalid package.json: {}", e));
        }
    }

    Err("No package.json found in VSIX archive".to_string())
}

/// Parse a VSCode theme JSON file (supports JSONC with comments).
pub fn load_theme_file(path: &Path) -> Result<VscodeThemeFile, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read theme file: {}", e))?;

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
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read snippet file: {}", e))?;
    let cleaned = strip_json_comments(&content);
    let map: HashMap<String, SnippetEntry> =
        serde_json::from_str(&cleaned).map_err(|e| format!("Invalid snippet JSON: {}", e))?;
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
                    Err(e) => {
                        tracing::warn!("Failed to load extension from {}: {}", path.display(), e)
                    }
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
    use std::io::Write;

    /// Write a .vsix (a zip) with the given `(entry name, contents)` pairs.
    fn make_vsix(path: &Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    const MANIFEST: &str = r#"{"name":"demo","version":"1.0.0","publisher":"acme"}"#;

    #[test]
    fn a_normal_vsix_installs_under_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let vsix = dir.path().join("demo.vsix");
        make_vsix(
            &vsix,
            &[
                ("extension/package.json", MANIFEST),
                ("extension/themes/dark.json", "{}"),
            ],
        );
        let root = dir.path().join("extensions");
        let installed = install_vsix_into(&vsix, &root).unwrap();
        assert_eq!(installed.root_dir, root.join("acme.demo-1.0.0"));
        assert!(root.join("acme.demo-1.0.0/package.json").is_file());
        assert!(root.join("acme.demo-1.0.0/themes/dark.json").is_file());
    }

    /// The zip writer refuses `..` and absolute names, so build the hostile
    /// archive the way an attacker would: write a same-length placeholder name,
    /// then patch the name bytes (local header and central directory) in the file.
    fn make_hostile_vsix(path: &Path, placeholder: &str, evil: &str) {
        assert_eq!(placeholder.len(), evil.len());
        make_vsix(
            path,
            &[("extension/package.json", MANIFEST), (placeholder, "pwned")],
        );
        let bytes = std::fs::read(path).unwrap();
        let (p, e) = (placeholder.as_bytes(), evil.as_bytes());
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        let mut patched = 0;
        while i < bytes.len() {
            if bytes[i..].starts_with(p) {
                out.extend_from_slice(e);
                i += p.len();
                patched += 1;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        assert_eq!(
            patched, 2,
            "name appears in the local header and the directory"
        );
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn a_vsix_entry_that_escapes_the_folder_is_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        for (placeholder, evil) in [
            ("extension/XX/XX/escape.txt", "extension/../../escape.txt"),
            ("XX/escape.txt", "../escape.txt"),
            (
                "extension/XX/XX/XX/XX/escape.txt",
                "extension/../../../../escape.txt",
            ),
        ] {
            let vsix = dir.path().join("evil.vsix");
            make_hostile_vsix(&vsix, placeholder, evil);
            let root = dir.path().join("extensions");
            let err = install_vsix_into(&vsix, &root).unwrap_err();
            assert!(
                err.contains("outside the extension folder"),
                "{evil}: {err}"
            );
            assert!(!dir.path().join("escape.txt").exists(), "{evil}");
            assert!(!root.exists(), "{evil}: nothing may be created");
        }
    }

    #[test]
    fn an_absolute_vsix_entry_is_confined_to_the_extension_folder() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let abs_name = format!(
            "/{}/escape.txt",
            outside.strip_prefix("/").unwrap().display()
        );
        // Same-length placeholder: swap the leading '/' for a letter.
        let placeholder = format!("X{}", &abs_name[1..]);
        let vsix = dir.path().join("abs.vsix");
        make_hostile_vsix(&vsix, &placeholder, &abs_name);
        let root = dir.path().join("extensions");
        install_vsix_into(&vsix, &root).unwrap();
        // Written under the extension's folder, never at the absolute path.
        assert!(!outside.join("escape.txt").exists());
        let rerooted = root
            .join("acme.demo-1.0.0")
            .join(outside.strip_prefix("/").unwrap())
            .join("escape.txt");
        assert!(rerooted.is_file(), "{}", rerooted.display());
    }

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
