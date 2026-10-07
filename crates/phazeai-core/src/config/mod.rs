use crate::constants::{defaults, paths};
use crate::llm::model_router::{ModelRoute, ModelRouter, TaskType};
use crate::llm::provider::{ProviderConfig, ProviderId, ProviderRegistry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub llm: LlmSettings,
    pub editor: EditorSettings,
    pub sidecar: SidecarSettings,
    #[serde(default)]
    pub providers: Vec<ProviderEntry>,
    #[serde(default)]
    pub model_routes: HashMap<TaskType, ModelRoute>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmSettings {
    pub provider: LlmProvider,
    pub model: String,
    pub api_key_env: String,
    pub base_url: Option<String>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Claude,
    OpenAI,
    Ollama,
    Groq,
    Together,
    OpenRouter,
    LmStudio,
    Gemini,
}

impl LlmProvider {
    pub fn to_provider_id(&self) -> ProviderId {
        match self {
            LlmProvider::Claude => ProviderId::Claude,
            LlmProvider::OpenAI => ProviderId::OpenAI,
            LlmProvider::Ollama => ProviderId::Ollama,
            LlmProvider::Groq => ProviderId::Groq,
            LlmProvider::Together => ProviderId::Together,
            LlmProvider::OpenRouter => ProviderId::OpenRouter,
            LlmProvider::LmStudio => ProviderId::LmStudio,
            LlmProvider::Gemini => ProviderId::Gemini,
        }
    }
}

/// A configured provider entry in settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub name: String,
    pub enabled: bool,
    pub api_key_env: String,
    pub base_url: String,
    pub default_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    pub theme: String,
    pub font_size: f32,
    pub tab_size: u32,
    pub show_line_numbers: bool,
    pub auto_save: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SidecarSettings {
    pub enabled: bool,
    pub python_path: String,
    pub auto_start: bool,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            provider: LlmProvider::Ollama,
            model: defaults::DEFAULT_MODEL.to_string(),
            api_key_env: String::new(),
            base_url: None,
            max_tokens: defaults::MAX_TOKENS,
        }
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            theme: defaults::THEME.to_string(),
            font_size: defaults::FONT_SIZE,
            tab_size: defaults::TAB_SIZE,
            show_line_numbers: true,
            auto_save: true,
        }
    }
}

impl Default for SidecarSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            python_path: defaults::PYTHON_PATH.to_string(),
            auto_start: true,
        }
    }
}

impl Settings {
    /// Directory holding `config.toml`, `session.toml` and other PhazeAI state.
    /// Single source of truth for every crate (was previously re-derived by hand).
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(paths::CONFIG_DIR)
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join(paths::CONFIG_FILE)
    }

    /// Update only the editor section on disk, preserving `[llm]`, `[sidecar]`,
    /// providers and routes. Safe to call from UI code on every preference change.
    pub fn save_editor(
        theme: &str,
        font_size: f32,
        tab_size: u32,
    ) -> Result<(), crate::error::PhazeError> {
        let mut s = Self::load();
        s.editor.theme = theme.to_string();
        s.editor.font_size = font_size;
        s.editor.tab_size = tab_size;
        s.save()
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

    /// Build a ProviderRegistry from settings.
    pub fn build_provider_registry(&self) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();

        // Apply any custom provider configs from settings
        for entry in &self.providers {
            let id = match entry.name.to_lowercase().as_str() {
                "claude" => ProviderId::Claude,
                "openai" => ProviderId::OpenAI,
                "ollama" => ProviderId::Ollama,
                "groq" => ProviderId::Groq,
                "together" => ProviderId::Together,
                "openrouter" => ProviderId::OpenRouter,
                "lmstudio" | "lm_studio" => ProviderId::LmStudio,
                "gemini" => ProviderId::Gemini,
                other => ProviderId::Custom(other.to_string()),
            };
            let config = ProviderConfig {
                id: id.clone(),
                enabled: entry.enabled,
                api_key_env: entry.api_key_env.clone(),
                base_url: entry.base_url.clone(),
                default_model: entry.default_model.clone(),
            };
            registry.add_custom_provider(entry.name.clone(), config);
        }

        // Set active provider from legacy settings
        let provider_id = self.llm.provider.to_provider_id();
        registry.set_active(provider_id, self.llm.model.clone());

        registry
    }

    /// Build an LLM client from the current settings.
    pub fn build_llm_client(
        &self,
    ) -> Result<Box<dyn crate::llm::LlmClient>, crate::error::PhazeError> {
        let registry = self.build_provider_registry();
        let default_client = registry.build_active_client()?;

        if self.model_routes.is_empty() {
            Ok(default_client)
        } else {
            let router = ModelRouter::new(&self.model_routes, &registry, default_client);
            Ok(Box::new(router))
        }
    }
}
