use serde::{Deserialize, Serialize};

/// Stored credentials (persisted to ~/.config/phazeai/cloud.toml).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CloudCredentials {
    pub email: Option<String>,
    /// API token from https://app.phazeai.com/settings/tokens
    pub api_token: Option<String>,
}

impl CloudCredentials {
    pub fn load() -> Self {
        let path = credentials_path();
        if let Ok(content) = std::fs::read_to_string(&path) {
            toml::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = credentials_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.api_token.as_ref().map(|t| !t.is_empty()).unwrap_or(false)
    }
}

fn credentials_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("phazeai")
        .join("cloud.toml")
}

/// Live session with the PhazeAI Cloud backend.
#[derive(Debug, Clone)]
pub struct CloudSession {
    pub email: String,
    pub token: String,
    pub tier: crate::subscription::Tier,
    /// Remaining AI credits for this billing period (tokens).
    pub credits_remaining: u64,
}
