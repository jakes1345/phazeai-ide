use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub llm: LlmSettings,
    pub editor: EditorSettings,
    pub sidecar: SidecarSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSettings {
    pub provider: LlmProvider,
    pub model: String,
    pub api_key_env: String,
    pub base_url: Option<String>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Claude,
    OpenAI,
    Ollama,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    pub theme: String,
    pub font_size: f32,
    pub tab_size: u32,
    pub show_line_numbers: bool,
    pub auto_save: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarSettings {
    pub enabled: bool,
    pub python_path: String,
    pub auto_start: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            llm: LlmSettings {
                provider: LlmProvider::Claude,
                model: "claude-sonnet-4-5-20250929".to_string(),
                api_key_env: "ANTHROPIC_API_KEY".to_string(),
                base_url: None,
                max_tokens: 8192,
            },
            editor: EditorSettings {
                theme: "Dark".to_string(),
                font_size: 14.0,
                tab_size: 4,
                show_line_numbers: true,
                auto_save: true,
            },
            sidecar: SidecarSettings {
                enabled: true,
                python_path: "python3".to_string(),
                auto_start: true,
            },
        }
    }
}

impl Settings {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("phazeai")
            .join("config.toml")
    }

    pub fn load() -> Self {
        let config_path = Self::config_path();
        if config_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(config) = toml::from_str(&content) {
                    return config;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), crate::error::PhazeError> {
        let config_path = Self::config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| crate::error::PhazeError::Config(e.to_string()))?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    /// Get the API key from the environment variable specified in settings.
    pub fn api_key(&self) -> Option<String> {
        std::env::var(&self.llm.api_key_env).ok()
    }

    /// Build an LLM client from the current settings.
    pub fn build_llm_client(&self) -> Result<Box<dyn crate::llm::LlmClient>, crate::error::PhazeError> {
        match self.llm.provider {
            LlmProvider::Claude => {
                let api_key = self.api_key().ok_or_else(|| {
                    crate::error::PhazeError::Config(format!(
                        "API key not found in env var '{}'",
                        self.llm.api_key_env
                    ))
                })?;
                let mut client = crate::llm::ClaudeClient::new(api_key)
                    .with_model(&self.llm.model)
                    .with_max_tokens(self.llm.max_tokens);
                if let Some(ref url) = self.llm.base_url {
                    client = client.with_base_url(url);
                }
                Ok(Box::new(client))
            }
            LlmProvider::OpenAI => {
                let api_key = self.api_key().ok_or_else(|| {
                    crate::error::PhazeError::Config(format!(
                        "API key not found in env var '{}'",
                        self.llm.api_key_env
                    ))
                })?;
                let mut client =
                    crate::llm::OpenAIClient::new(api_key).with_model(&self.llm.model);
                if let Some(ref url) = self.llm.base_url {
                    client = client.with_base_url(url);
                }
                Ok(Box::new(client))
            }
            LlmProvider::Ollama => {
                let mut client = crate::llm::OllamaClient::new(&self.llm.model);
                if let Some(ref url) = self.llm.base_url {
                    client = client.with_base_url(url);
                }
                Ok(Box::new(client))
            }
        }
    }
}
