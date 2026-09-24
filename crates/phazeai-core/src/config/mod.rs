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
    pub word_wrap: bool,
    pub relative_line_numbers: bool,
    pub inlay_hints: bool,
    pub code_lens: bool,
    pub organize_imports_on_save: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            theme: defaults::THEME.to_string(),
            font_size: defaults::FONT_SIZE,
            tab_size: defaults::TAB_SIZE,
            show_line_numbers: true,
            auto_save: true,
            word_wrap: false,
            relative_line_numbers: false,
            inlay_hints: true,
            code_lens: true,
            organize_imports_on_save: false,
        }
    }
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
            api_key_env: "".to_string(),
            base_url: None,
            max_tokens: defaults::MAX_TOKENS,
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

/// Set when the settings file exists but could not be read or parsed, so the
/// UI can tell the user instead of silently running on defaults.
static LOAD_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

impl Settings {
    pub fn config_path() -> PathBuf {
        let config_root = dirs::config_dir().or_else(|| {
            std::env::var_os("HOME").map(|home| {
                let mut p = PathBuf::from(home);
                p.push(".config");
                p
            })
        });
        config_root
            .unwrap_or_else(|| PathBuf::from("."))
            .join(paths::CONFIG_DIR)
            .join(paths::CONFIG_FILE)
    }

    /// Load settings, falling back to defaults if the file is missing or
    /// broken. A broken file is left untouched on disk (see [`Self::save`])
    /// and the problem is reported through [`Self::load_error`].
    pub fn load() -> Self {
        let config_path = Self::config_path();
        let result = match std::fs::read_to_string(&config_path) {
            Ok(content) => toml::from_str::<Self>(&content)
                .map_err(|e| format!("{} is not valid: {e}", config_path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("could not read {}: {e}", config_path.display())),
        };
        let mut slot = LOAD_ERROR.lock().unwrap_or_else(|p| p.into_inner());
        match result {
            Ok(settings) => {
                *slot = None;
                settings
            }
            Err(msg) => {
                if slot.as_deref() != Some(msg.as_str()) {
                    tracing::error!("{msg}; using default settings");
                }
                *slot = Some(msg);
                Self::default()
            }
        }
    }

    /// The error from the most recent [`Self::load`], if the settings file
    /// exists but could not be used.
    pub fn load_error() -> Option<String> {
        LOAD_ERROR.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Save settings atomically. If the file currently on disk can't be
    /// parsed, it is first copied to `settings.toml.corrupt-<timestamp>` so a
    /// typo in a hand-edited config never silently destroys the user's
    /// providers and routes.
    pub fn save(&self) -> Result<(), crate::error::PhazeError> {
        let config_path = Self::config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Ok(existing) = std::fs::read_to_string(&config_path) {
            if toml::from_str::<Self>(&existing).is_err() {
                let backup = config_path.with_extension(format!(
                    "toml.corrupt-{}",
                    chrono::Local::now().format("%Y%m%d-%H%M%S")
                ));
                std::fs::copy(&config_path, &backup)?;
                tracing::warn!(
                    backup = %backup.display(),
                    "backed up unparseable settings file before overwriting"
                );
            }
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| crate::error::PhazeError::Config(e.to_string()))?;
        let tmp = config_path.with_extension("toml.tmp");
        std::fs::write(&tmp, content)?;
        std::fs::rename(&tmp, &config_path)?;
        *LOAD_ERROR.lock().unwrap_or_else(|p| p.into_inner()) = None;
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
                "claude" | "anthropic" => ProviderId::Claude,
                "openai" => ProviderId::OpenAI,
                "ollama" => ProviderId::Ollama,
                "groq" => ProviderId::Groq,
                "together" => ProviderId::Together,
                "openrouter" => ProviderId::OpenRouter,
                "lmstudio" | "lm_studio" => ProviderId::LmStudio,
                "gemini" => ProviderId::Gemini,
                "mistral" => ProviderId::Mistral,
                "deepseek" => ProviderId::DeepSeek,
                "xai" | "grok" => ProviderId::Xai,
                "cohere" => ProviderId::Cohere,
                "perplexity" => ProviderId::Perplexity,
                "azure" | "azure_openai" => ProviderId::Azure,
                "cerebras" => ProviderId::Cerebras,
                "fireworks" => ProviderId::Fireworks,
                "sambanova" => ProviderId::SambaNova,
                "github" | "github_models" | "githubmodels" => ProviderId::GithubModels,
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

    /// Build a client for one kind of task: the `[model_routes]` entry for
    /// `task` if one is configured and buildable, otherwise the active
    /// provider/model. Used by the multi-agent pipeline to give each role
    /// its own model.
    pub fn build_llm_client_for(
        &self,
        task: TaskType,
    ) -> Result<Box<dyn crate::llm::LlmClient>, crate::error::PhazeError> {
        let registry = self.build_provider_registry();
        if let Some(route) = self.model_routes.get(&task) {
            let id = ModelRouter::parse_provider_id(&route.provider);
            if let Some(config) = registry.get_config(&id) {
                match registry.build_client_for(config, &route.model) {
                    Ok(client) => return Ok(client),
                    Err(e) => tracing::warn!(
                        "model route for {task:?} unusable ({e}); using the active model"
                    ),
                }
            }
        }
        registry.build_active_client()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_settings_file_keeps_user_values() {
        // An older/hand-written config missing whole sections must still parse
        // instead of being treated as corrupt and replaced with defaults.
        let s: Settings = toml::from_str(
            "[llm]\nprovider = \"claude\"\nmodel = \"my-model\"\n\n[editor]\nfont_size = 18.0\n",
        )
        .unwrap();
        assert_eq!(s.llm.provider, LlmProvider::Claude);
        assert_eq!(s.llm.model, "my-model");
        assert_eq!(s.llm.max_tokens, defaults::MAX_TOKENS);
        assert_eq!(s.editor.font_size, 18.0);
        assert!(s.sidecar.enabled);
    }

    #[test]
    fn readme_model_routes_example_parses() {
        let s: Settings = toml::from_str(
            r#"
[model_routes.reasoning]
provider = "claude"
model = "claude-opus-4-7"

[model_routes.code_generation]
provider = "ollama"
model = "qwen2.5-coder:14b"

[model_routes.code_review]
provider = "openai"
model = "gpt-4.1"
"#,
        )
        .unwrap();
        assert_eq!(s.model_routes.len(), 3);
        assert_eq!(
            s.model_routes[&TaskType::CodeGeneration].model,
            "qwen2.5-coder:14b"
        );
        // A local route builds without any API key.
        assert!(s.build_llm_client_for(TaskType::CodeGeneration).is_ok());
    }
}
