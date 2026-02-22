use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persisted IDE layout state, saved between sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeState {
    pub last_workspace: Option<PathBuf>,
    pub explorer_width: f32,
    pub chat_width: f32,
    pub terminal_height: f32,
    pub show_explorer: bool,
    pub show_chat: bool,
    pub show_terminal: bool,
    pub ai_mode: String,
}

impl Default for IdeState {
    fn default() -> Self {
        Self {
            last_workspace: None,
            explorer_width: 220.0,
            chat_width: 320.0,
            terminal_height: 200.0,
            show_explorer: true,
            show_chat: true,
            show_terminal: true,
            ai_mode: "Chat".to_string(),
        }
    }
}

impl IdeState {
    fn state_path() -> PathBuf {
        // Use $HOME/.config/phazeai/ (same as phazeai-core)
        let home = std::env::var("HOME")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let base = home.join(".config").join("phazeai");
        let _ = std::fs::create_dir_all(&base);
        base.join("ide_state.json")
    }

    pub fn load() -> Self {
        let path = Self::state_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::state_path();
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, s);
        }
    }
}
