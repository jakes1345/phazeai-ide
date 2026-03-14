use std::collections::HashMap;
use std::path::PathBuf;

use super::asset_loader::InstalledExtension;
use super::vscode_assets::{LanguageConfiguration, SnippetEntry, VscodeThemeFile};

/// Runtime registry of all loaded extension assets.
///
/// Aggregates language configurations, themes, snippets, and grammar paths
/// from every registered `InstalledExtension` and provides fast lookup APIs.
#[derive(Debug, Default)]
pub struct ExtensionRegistry {
    /// All installed extensions in registration order.
    extensions: Vec<InstalledExtension>,
    /// File extension (without leading `.`, lowercased) → language ID mapping,
    /// aggregated from all registered extensions.
    ext_to_language: HashMap<String, String>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an installed extension to the registry and rebuild mappings.
    pub fn register(&mut self, ext: InstalledExtension) {
        // Build extension-to-language mappings from this extension's contributions.
        if let Some(ref contributes) = ext.manifest.contributes {
            for lang in &contributes.languages {
                for file_ext in &lang.extensions {
                    let normalized = file_ext.trim_start_matches('.').to_lowercase();
                    self.ext_to_language.insert(normalized, lang.id.clone());
                }
            }
        }
        self.extensions.push(ext);
    }

    /// Return a slice of all registered extensions.
    pub fn extensions(&self) -> &[InstalledExtension] {
        &self.extensions
    }

    /// Look up the language ID for a file extension (with or without leading `.`).
    ///
    /// Returns `None` if no registered extension contributes this file type.
    pub fn language_for_extension(&self, file_ext: &str) -> Option<&str> {
        let normalized = file_ext.trim_start_matches('.').to_lowercase();
        self.ext_to_language.get(&normalized).map(|s| s.as_str())
    }

    /// Get the language configuration for a language ID, searching all extensions.
    ///
    /// Returns the first match found. Extensions are searched in registration order.
    pub fn language_config(&self, language_id: &str) -> Option<&LanguageConfiguration> {
        for ext in &self.extensions {
            if let Some(config) = ext.language_configs.get(language_id) {
                return Some(config);
            }
        }
        None
    }

    /// Return all available themes across all registered extensions.
    ///
    /// Each entry is `(label, theme_file)`. Labels are not guaranteed to be unique
    /// if multiple extensions contribute a theme with the same label.
    pub fn available_themes(&self) -> Vec<(&str, &VscodeThemeFile)> {
        let mut themes = Vec::new();
        for ext in &self.extensions {
            for (label, theme) in &ext.themes {
                themes.push((label.as_str(), theme));
            }
        }
        themes
    }

    /// Find a specific theme by label, searching all extensions.
    ///
    /// Returns the first match in registration order.
    pub fn theme_by_label(&self, label: &str) -> Option<&VscodeThemeFile> {
        for ext in &self.extensions {
            if let Some(theme) = ext.themes.get(label) {
                return Some(theme);
            }
        }
        None
    }

    /// Get all snippet entries for a language ID, from all extensions.
    pub fn snippets_for_language(&self, language_id: &str) -> Vec<&SnippetEntry> {
        let mut result = Vec::new();
        for ext in &self.extensions {
            if let Some(entries) = ext.snippets.get(language_id) {
                result.extend(entries.iter());
            }
        }
        result
    }

    /// Collect all grammar file paths from all extensions.
    ///
    /// Each entry is `(scope_name, absolute_path)`. Only paths that exist on
    /// disk are included. Suitable for passing to a syntect `SyntaxSet` loader.
    pub fn grammar_paths(&self) -> Vec<(String, PathBuf)> {
        let mut paths = Vec::new();
        for ext in &self.extensions {
            if let Some(ref contributes) = ext.manifest.contributes {
                for grammar in &contributes.grammars {
                    let full_path = ext.root_dir.join(&grammar.path);
                    if full_path.exists() {
                        paths.push((grammar.scope_name.clone(), full_path));
                    }
                }
            }
        }
        paths
    }

    /// Return a human-readable summary line for each registered extension.
    pub fn summary(&self) -> Vec<String> {
        self.extensions
            .iter()
            .map(|ext| {
                let c = ext.manifest.contributes.as_ref();
                let themes = c.map(|c| c.themes.len()).unwrap_or(0);
                let grammars = c.map(|c| c.grammars.len()).unwrap_or(0);
                let snippets = c.map(|c| c.snippets.len()).unwrap_or(0);
                let langs = c.map(|c| c.languages.len()).unwrap_or(0);
                format!(
                    "{} v{} — {} themes, {} grammars, {} snippets, {} languages",
                    ext.manifest
                        .display_name
                        .as_deref()
                        .unwrap_or(&ext.manifest.name),
                    ext.manifest.version,
                    themes,
                    grammars,
                    snippets,
                    langs,
                )
            })
            .collect()
    }

    /// Scan `~/.phazeai/extensions/` for installed extensions and register them all.
    ///
    /// Extensions that fail to load are skipped with a warning.
    pub fn load_all(&mut self) {
        for ext in super::asset_loader::scan_installed_extensions() {
            self.register(ext);
        }
    }

    /// Uninstall an extension by name: removes from the registry and deletes its directory.
    ///
    /// Rebuilds the `ext_to_language` mapping after removal.
    pub fn uninstall(&mut self, name: &str) -> Result<(), String> {
        let pos = self
            .extensions
            .iter()
            .position(|e| e.manifest.name == name)
            .ok_or_else(|| format!("Extension '{}' not found", name))?;

        let ext = self.extensions.remove(pos);
        std::fs::remove_dir_all(&ext.root_dir)
            .map_err(|e| format!("Failed to remove extension directory: {}", e))?;

        // Rebuild ext_to_language from the remaining extensions
        self.ext_to_language.clear();
        for remaining in &self.extensions {
            if let Some(ref contributes) = remaining.manifest.contributes {
                for lang in &contributes.languages {
                    for file_ext in &lang.extensions {
                        let normalized = file_ext.trim_start_matches('.').to_lowercase();
                        self.ext_to_language
                            .insert(normalized, lang.id.clone());
                    }
                }
            }
        }

        Ok(())
    }
}
