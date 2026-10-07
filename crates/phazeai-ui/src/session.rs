//! Session persistence (`~/.config/phazeai/session.toml`).
//!
//! Open tabs, active file, panel layout and vim flag. Serialised with serde/TOML
//! so paths containing quotes, backslashes or unicode round-trip correctly, and
//! written atomically (temp file + rename) so a crash cannot corrupt the file.

use std::path::{Path, PathBuf};

use phazeai_core::Settings;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SessionData {
    /// All open tab paths.
    pub open_tabs: Vec<PathBuf>,
    /// The active (focused) tab path, if any.
    pub open_file: Option<PathBuf>,
    pub left_panel_width: f64,
    pub show_bottom_panel: bool,
    pub vim_mode: bool,
    pub theme: String,
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            open_tabs: Vec::new(),
            open_file: None,
            left_panel_width: 300.0,
            show_bottom_panel: false,
            vim_mode: false,
            theme: "Midnight Blue".to_string(),
        }
    }
}

pub fn session_path() -> PathBuf {
    Settings::config_dir().join("session.toml")
}

/// Write `contents` to `path` atomically.
pub fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

fn load_from(path: &Path) -> SessionData {
    let Ok(text) = std::fs::read_to_string(path) else {
        return SessionData::default();
    };
    let mut data: SessionData = toml::from_str(&text).unwrap_or_default();
    // Drop files that no longer exist so we never restore empty tabs.
    data.open_tabs.retain(|p| p.exists());
    if data.open_file.as_ref().is_some_and(|p| !p.exists()) {
        data.open_file = None;
    }
    data
}

fn save_to(path: &Path, data: &SessionData) {
    match toml::to_string_pretty(data) {
        Ok(text) => {
            if let Err(e) = atomic_write(path, &text) {
                eprintln!("[session] failed to save {}: {e}", path.display());
            }
        }
        Err(e) => eprintln!("[session] failed to serialise session: {e}"),
    }
}

pub fn load() -> SessionData {
    load_from(&session_path())
}

/// Update layout fields and the active file, **keeping the saved tab list**.
pub fn save_layout(
    open_file: Option<&PathBuf>,
    left_panel_width: f64,
    show_bottom_panel: bool,
    vim_mode: bool,
    theme: &str,
) {
    let path = session_path();
    let mut data = load_from(&path);
    data.open_file = open_file.cloned();
    data.left_panel_width = left_panel_width;
    data.show_bottom_panel = show_bottom_panel;
    data.vim_mode = vim_mode;
    data.theme = theme.to_string();
    save_to(&path, &data);
}

/// Save the full set of open tabs plus the active file and layout.
pub fn save_tabs(
    tabs: &[PathBuf],
    active: Option<&PathBuf>,
    left_panel_width: f64,
    show_bottom_panel: bool,
    vim_mode: bool,
    theme: &str,
) {
    let data = SessionData {
        open_tabs: tabs.to_vec(),
        open_file: active.cloned(),
        left_panel_width,
        show_bottom_panel,
        vim_mode,
        theme: theme.to_string(),
    };
    save_to(&session_path(), &data);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_files(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| {
                let p = dir.join(n);
                std::fs::write(&p, "x").unwrap();
                p
            })
            .collect()
    }

    #[test]
    fn round_trips_awkward_paths() {
        let dir = tempfile::tempdir().unwrap();
        let tabs = temp_files(
            dir.path(),
            &["a \"quoted\".rs", "back\\slash.rs", "émoji 😀.rs"],
        );
        let file = dir.path().join("session.toml");
        let data = SessionData {
            open_tabs: tabs.clone(),
            open_file: Some(tabs[0].clone()),
            theme: "My \"Theme\"".into(),
            ..Default::default()
        };
        save_to(&file, &data);
        let loaded = load_from(&file);
        assert_eq!(loaded, data);
    }

    #[test]
    fn missing_files_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let tabs = temp_files(dir.path(), &["keep.rs"]);
        let file = dir.path().join("session.toml");
        let data = SessionData {
            open_tabs: vec![tabs[0].clone(), dir.path().join("gone.rs")],
            open_file: Some(dir.path().join("gone.rs")),
            ..Default::default()
        };
        save_to(&file, &data);
        let loaded = load_from(&file);
        assert_eq!(loaded.open_tabs, vec![tabs[0].clone()]);
        assert_eq!(loaded.open_file, None);
    }

    #[test]
    fn corrupt_file_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("session.toml");
        std::fs::write(&file, "this is = = not toml [[[").unwrap();
        assert_eq!(load_from(&file), SessionData::default());
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("session.toml");
        std::fs::write(&file, "vim_mode = true\n").unwrap();
        let loaded = load_from(&file);
        assert!(loaded.vim_mode);
        assert_eq!(loaded.left_panel_width, 300.0);
    }
}
