use crate::constants::endpoints;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Identifies a specific LLM provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Claude,
    OpenAI,
    Ollama,
    Groq,
    Together,
    OpenRouter,
    LmStudio,
    Gemini,
    Mistral,
    DeepSeek,
    Xai,
    Cohere,
    Perplexity,
    /// Azure OpenAI — user must configure base_url per deployment
    Azure,
    Cerebras,
    Fireworks,
    SambaNova,
    /// GitHub Models — free inference via GITHUB_TOKEN
    GithubModels,
    Custom(String),
}

impl ProviderId {
    pub fn name(&self) -> &str {
        match self {
            Self::Claude => "Claude (Anthropic)",
            Self::OpenAI => "OpenAI",
            Self::Ollama => "Ollama (Local)",
            Self::Groq => "Groq",
            Self::Together => "Together.ai",
            Self::OpenRouter => "OpenRouter",
            Self::LmStudio => "LM Studio (Local)",
            Self::Gemini => "Google Gemini",
            Self::Mistral => "Mistral AI",
            Self::DeepSeek => "DeepSeek",
            Self::Xai => "xAI (Grok)",
            Self::Cohere => "Cohere",
            Self::Perplexity => "Perplexity",
            Self::Azure => "Azure OpenAI",
            Self::Cerebras => "Cerebras",
            Self::Fireworks => "Fireworks AI",
            Self::SambaNova => "SambaNova",
            Self::GithubModels => "GitHub Models",
            Self::Custom(name) => name,
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Self::Ollama | Self::LmStudio)
    }

    pub fn needs_api_key(&self) -> bool {
        !self.is_local()
    }

    pub fn default_base_url(&self) -> &str {
        match self {
            Self::Claude => endpoints::CLAUDE_BASE_URL,
            Self::OpenAI => endpoints::OPENAI_BASE_URL,
            Self::Ollama => endpoints::OLLAMA_BASE_URL,
            Self::Groq => endpoints::GROQ_BASE_URL,
            Self::Together => endpoints::TOGETHER_BASE_URL,
            Self::OpenRouter => endpoints::OPENROUTER_BASE_URL,
            Self::LmStudio => endpoints::LMSTUDIO_BASE_URL,
            Self::Gemini => endpoints::GEMINI_BASE_URL,
            Self::Mistral => endpoints::MISTRAL_BASE_URL,
            Self::DeepSeek => endpoints::DEEPSEEK_BASE_URL,
            Self::Xai => endpoints::XAI_BASE_URL,
            Self::Cohere => endpoints::COHERE_BASE_URL,
            Self::Perplexity => endpoints::PERPLEXITY_BASE_URL,
            Self::Azure => endpoints::AZURE_BASE_URL,
            Self::Cerebras => endpoints::CEREBRAS_BASE_URL,
            Self::Fireworks => endpoints::FIREWORKS_BASE_URL,
            Self::SambaNova => endpoints::SAMBANOVA_BASE_URL,
            Self::GithubModels => endpoints::GITHUB_MODELS_BASE_URL,
            Self::Custom(_) => "",
        }
    }

    pub fn default_api_key_env(&self) -> &str {
        match self {
            Self::Claude => "ANTHROPIC_API_KEY",
            Self::OpenAI => "OPENAI_API_KEY",
            Self::Ollama => "",
            Self::Groq => "GROQ_API_KEY",
            Self::Together => "TOGETHER_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::LmStudio => "",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Mistral => "MISTRAL_API_KEY",
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::Xai => "XAI_API_KEY",
            Self::Cohere => "COHERE_API_KEY",
            Self::Perplexity => "PERPLEXITY_API_KEY",
            Self::Azure => "AZURE_OPENAI_API_KEY",
            Self::Cerebras => "CEREBRAS_API_KEY",
            Self::Fireworks => "FIREWORKS_API_KEY",
            Self::SambaNova => "SAMBANOVA_API_KEY",
            Self::GithubModels => "GITHUB_TOKEN",
            Self::Custom(_) => "",
        }
    }

    pub fn all_builtin() -> Vec<ProviderId> {
        vec![
            Self::Claude,
            Self::OpenAI,
            Self::Gemini,
            Self::Mistral,
            Self::DeepSeek,
            Self::Xai,
            Self::Groq,
            Self::Cerebras,
            Self::Together,
            Self::Fireworks,
            Self::SambaNova,
            Self::OpenRouter,
            Self::Perplexity,
            Self::Cohere,
            Self::Azure,
            Self::GithubModels,
            Self::Ollama,
            Self::LmStudio,
        ]
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Capabilities of a provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderCapabilities {
    pub supports_tools: bool,
    pub supports_streaming: bool,
    pub supports_vision: bool,
    pub supports_system_prompt: bool,
    pub max_context_window: usize,
}

/// Info about a specific model available from a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_window: usize,
    pub supports_tools: bool,
    /// Cost per million input tokens (USD), 0.0 for free/local
    pub input_cost_per_m: f64,
    /// Cost per million output tokens (USD), 0.0 for free/local
    pub output_cost_per_m: f64,
}

/// Configuration for a single provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: ProviderId,
    pub enabled: bool,
    pub api_key_env: String,
    pub base_url: String,
    pub default_model: String,
}

/// Keyring service name used for storing provider API keys.
pub const KEYRING_SERVICE: &str = "phazeai";

/// Read an API key from the OS keyring (macOS Keychain / Linux Secret Service /
/// Windows Credential Manager). Returns `None` if the entry is absent or the
/// keyring backend is unavailable (e.g., headless CI).
pub fn keyring_get(entry_name: &str) -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, entry_name).ok()?;
    entry.get_password().ok()
}

/// Store an API key in the OS keyring. Returns the backend error as a string
/// so UI layers can surface it to the user.
pub fn keyring_set(entry_name: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, entry_name).map_err(|e| e.to_string())?;
    entry.set_password(value).map_err(|e| e.to_string())
}

/// Delete an API key from the OS keyring. A missing entry is treated as
/// success (idempotent clear).
pub fn keyring_delete(entry_name: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, entry_name).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Key-source indicator shown in the settings UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeySource {
    None,
    Keyring,
    Env,
}

impl ProviderConfig {
    /// Resolve the API key, preferring the OS keyring and falling back to the
    /// configured environment variable.
    pub fn api_key(&self) -> Option<String> {
        if self.api_key_env.is_empty() {
            return None;
        }
        if let Some(k) = keyring_get(&self.api_key_env) {
            if !k.is_empty() {
                return Some(k);
            }
        }
        std::env::var(&self.api_key_env).ok()
    }

    /// Report where the key came from (for the settings UI status line).
    pub fn api_key_source(&self) -> ApiKeySource {
        if self.api_key_env.is_empty() {
            return ApiKeySource::None;
        }
        if keyring_get(&self.api_key_env)
            .filter(|s| !s.is_empty())
            .is_some()
        {
            return ApiKeySource::Keyring;
        }
        if std::env::var(&self.api_key_env).is_ok() {
            return ApiKeySource::Env;
        }
        ApiKeySource::None
    }

    pub fn is_available(&self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.id.needs_api_key() {
            self.api_key().is_some()
        } else {
            true
        }
    }
}

/// Manages all configured providers and provides model listing.
pub struct ProviderRegistry {
    providers: HashMap<ProviderId, ProviderConfig>,
    active_provider: ProviderId,
    active_model: String,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    pub fn new() -> Self {
        let mut providers = HashMap::new();

        // Register all built-in providers with defaults
        for id in ProviderId::all_builtin() {
            let config = ProviderConfig {
                api_key_env: id.default_api_key_env().to_string(),
                base_url: id.default_base_url().to_string(),
                default_model: default_model_for(&id).to_string(),
                enabled: true,
                id: id.clone(),
            };
            providers.insert(id, config);
        }

        Self {
            providers,
            active_provider: ProviderId::Claude,
            active_model: crate::constants::models::DEFAULT_CLAUDE_MODEL.to_string(),
        }
    }

    pub fn from_configs(configs: Vec<ProviderConfig>) -> Self {
        let mut registry = Self::new();
        for config in configs {
            registry.providers.insert(config.id.clone(), config);
        }
        registry
    }

    pub fn active_provider(&self) -> &ProviderId {
        &self.active_provider
    }

    pub fn active_model(&self) -> &str {
        &self.active_model
    }

    pub fn set_active(&mut self, provider: ProviderId, model: String) {
        self.active_provider = provider;
        self.active_model = model;
    }

    pub fn set_provider(&mut self, provider: ProviderId) {
        if let Some(config) = self.providers.get(&provider) {
            self.active_model = config.default_model.clone();
        }
        self.active_provider = provider;
    }

    pub fn set_model(&mut self, model: String) {
        self.active_model = model;
    }

    pub fn get_config(&self, id: &ProviderId) -> Option<&ProviderConfig> {
        self.providers.get(id)
    }

    pub fn active_config(&self) -> Option<&ProviderConfig> {
        self.providers.get(&self.active_provider)
    }

    pub fn available_providers(&self) -> Vec<&ProviderConfig> {
        self.providers
            .values()
            .filter(|c| c.is_available())
            .collect()
    }

    pub fn all_providers(&self) -> Vec<&ProviderConfig> {
        self.providers.values().collect()
    }

    pub fn add_custom_provider(&mut self, name: String, config: ProviderConfig) {
        self.providers.insert(ProviderId::Custom(name), config);
    }

    /// Build an LLM client for the currently active provider/model.
    pub fn build_active_client(
        &self,
    ) -> Result<Box<dyn super::LlmClient>, crate::error::PhazeError> {
        let config = self.active_config().ok_or_else(|| {
            crate::error::PhazeError::Config(format!(
                "Provider {:?} not configured",
                self.active_provider
            ))
        })?;

        self.build_client_for(config, &self.active_model)
    }

    /// Build an LLM client for a specific provider and model.
    pub fn build_client_for(
        &self,
        config: &ProviderConfig,
        model: &str,
    ) -> Result<Box<dyn super::LlmClient>, crate::error::PhazeError> {
        match config.id {
            ProviderId::Claude => {
                let api_key = config.api_key().ok_or_else(|| {
                    crate::error::PhazeError::Config(format!(
                        "Set {} environment variable for Claude",
                        config.api_key_env
                    ))
                })?;
                let client = super::ClaudeClient::new(api_key)
                    .with_model(model)
                    .with_base_url(&config.base_url)
                    .with_max_tokens(8192);
                Ok(Box::new(client))
            }
            ProviderId::Ollama => {
                let client = super::OllamaClient::new(model).with_base_url(&config.base_url);
                Ok(Box::new(client))
            }
            // All other providers use OpenAI-compatible API
            _ => {
                let api_key = if config.id.needs_api_key() {
                    config.api_key().ok_or_else(|| {
                        crate::error::PhazeError::Config(format!(
                            "Set {} environment variable for {}",
                            config.api_key_env,
                            config.id.name()
                        ))
                    })?
                } else {
                    String::new()
                };
                let client = super::OpenAIClient::new(api_key)
                    .with_model(model)
                    .with_base_url(&config.base_url);
                Ok(Box::new(client))
            }
        }
    }

    /// Get known models for a provider (static list for cloud, dynamic for local).
    pub fn known_models(provider: &ProviderId) -> Vec<ModelInfo> {
        match provider {
            ProviderId::Claude => vec![
                ModelInfo {
                    id: "claude-opus-4-6".into(),
                    name: "Claude Opus 4.6".into(),
                    context_window: 200_000,
                    supports_tools: true,
                    input_cost_per_m: 15.0,
                    output_cost_per_m: 75.0,
                },
                ModelInfo {
                    id: "claude-sonnet-4-5-20250929".into(),
                    name: "Claude Sonnet 4.5".into(),
                    context_window: 200_000,
                    supports_tools: true,
                    input_cost_per_m: 3.0,
                    output_cost_per_m: 15.0,
                },
                ModelInfo {
                    id: "claude-haiku-4-5-20251001".into(),
                    name: "Claude Haiku 4.5".into(),
                    context_window: 200_000,
                    supports_tools: true,
                    input_cost_per_m: 0.80,
                    output_cost_per_m: 4.0,
                },
            ],
            ProviderId::OpenAI => vec![
                ModelInfo {
                    id: "gpt-4o".into(),
                    name: "GPT-4o".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 2.50,
                    output_cost_per_m: 10.0,
                },
                ModelInfo {
                    id: "gpt-4o-mini".into(),
                    name: "GPT-4o Mini".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 0.15,
                    output_cost_per_m: 0.60,
                },
                ModelInfo {
                    id: "o1".into(),
                    name: "o1".into(),
                    context_window: 200_000,
                    supports_tools: true,
                    input_cost_per_m: 15.0,
                    output_cost_per_m: 60.0,
                },
            ],
            ProviderId::Groq => vec![
                ModelInfo {
                    id: "llama-3.3-70b-versatile".into(),
                    name: "Llama 3.3 70B".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 0.59,
                    output_cost_per_m: 0.79,
                },
                ModelInfo {
                    id: "mixtral-8x7b-32768".into(),
                    name: "Mixtral 8x7B".into(),
                    context_window: 32_768,
                    supports_tools: true,
                    input_cost_per_m: 0.24,
                    output_cost_per_m: 0.24,
                },
                ModelInfo {
                    id: "deepseek-r1-distill-llama-70b".into(),
                    name: "DeepSeek R1 70B".into(),
                    context_window: 128_000,
                    supports_tools: false,
                    input_cost_per_m: 0.75,
                    output_cost_per_m: 0.99,
                },
            ],
            ProviderId::Together => vec![
                ModelInfo {
                    id: "meta-llama/Llama-3.3-70B-Instruct-Turbo".into(),
                    name: "Llama 3.3 70B Turbo".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 0.88,
                    output_cost_per_m: 0.88,
                },
                ModelInfo {
                    id: "deepseek-ai/DeepSeek-R1".into(),
                    name: "DeepSeek R1".into(),
                    context_window: 128_000,
                    supports_tools: false,
                    input_cost_per_m: 3.0,
                    output_cost_per_m: 7.0,
                },
                ModelInfo {
                    id: "Qwen/Qwen2.5-Coder-32B-Instruct".into(),
                    name: "Qwen 2.5 Coder 32B".into(),
                    context_window: 32_768,
                    supports_tools: true,
                    input_cost_per_m: 0.80,
                    output_cost_per_m: 0.80,
                },
            ],
            ProviderId::OpenRouter => vec![
                ModelInfo {
                    id: "anthropic/claude-sonnet-4-5-20250929".into(),
                    name: "Claude Sonnet 4.5 (via OpenRouter)".into(),
                    context_window: 200_000,
                    supports_tools: true,
                    input_cost_per_m: 3.0,
                    output_cost_per_m: 15.0,
                },
                ModelInfo {
                    id: "google/gemini-2.0-flash-001".into(),
                    name: "Gemini 2.0 Flash".into(),
                    context_window: 1_000_000,
                    supports_tools: true,
                    input_cost_per_m: 0.10,
                    output_cost_per_m: 0.40,
                },
                ModelInfo {
                    id: "deepseek/deepseek-chat".into(),
                    name: "DeepSeek V3".into(),
                    context_window: 64_000,
                    supports_tools: true,
                    input_cost_per_m: 0.14,
                    output_cost_per_m: 0.28,
                },
            ],
            ProviderId::Gemini => vec![
                ModelInfo {
                    id: "gemini-2.5-pro".into(),
                    name: "Gemini 2.5 Pro (thinking)".into(),
                    context_window: 1_000_000,
                    supports_tools: true,
                    input_cost_per_m: 1.25,
                    output_cost_per_m: 10.0,
                },
                ModelInfo {
                    id: "gemini-2.5-flash".into(),
                    name: "Gemini 2.5 Flash".into(),
                    context_window: 1_000_000,
                    supports_tools: true,
                    input_cost_per_m: 0.075,
                    output_cost_per_m: 0.30,
                },
                ModelInfo {
                    id: "gemini-2.0-flash".into(),
                    name: "Gemini 2.0 Flash".into(),
                    context_window: 1_000_000,
                    supports_tools: true,
                    input_cost_per_m: 0.10,
                    output_cost_per_m: 0.40,
                },
                ModelInfo {
                    id: "gemini-2.0-flash-lite".into(),
                    name: "Gemini 2.0 Flash Lite".into(),
                    context_window: 1_000_000,
                    supports_tools: true,
                    input_cost_per_m: 0.075,
                    output_cost_per_m: 0.30,
                },
                ModelInfo {
                    id: "gemini-1.5-pro".into(),
                    name: "Gemini 1.5 Pro".into(),
                    context_window: 2_000_000,
                    supports_tools: true,
                    input_cost_per_m: 1.25,
                    output_cost_per_m: 5.0,
                },
                ModelInfo {
                    id: "gemini-1.5-flash".into(),
                    name: "Gemini 1.5 Flash".into(),
                    context_window: 1_000_000,
                    supports_tools: true,
                    input_cost_per_m: 0.075,
                    output_cost_per_m: 0.30,
                },
            ],
            ProviderId::Mistral => vec![
                ModelInfo {
                    id: "mistral-large-latest".into(),
                    name: "Mistral Large".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 2.0,
                    output_cost_per_m: 6.0,
                },
                ModelInfo {
                    id: "mistral-small-latest".into(),
                    name: "Mistral Small".into(),
                    context_window: 32_000,
                    supports_tools: true,
                    input_cost_per_m: 0.10,
                    output_cost_per_m: 0.30,
                },
                ModelInfo {
                    id: "codestral-latest".into(),
                    name: "Codestral".into(),
                    context_window: 256_000,
                    supports_tools: true,
                    input_cost_per_m: 0.30,
                    output_cost_per_m: 0.90,
                },
                ModelInfo {
                    id: "mistral-nemo".into(),
                    name: "Mistral Nemo 12B".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 0.10,
                    output_cost_per_m: 0.10,
                },
            ],
            ProviderId::DeepSeek => vec![
                ModelInfo {
                    id: "deepseek-chat".into(),
                    name: "DeepSeek V3".into(),
                    context_window: 64_000,
                    supports_tools: true,
                    input_cost_per_m: 0.27,
                    output_cost_per_m: 1.10,
                },
                ModelInfo {
                    id: "deepseek-reasoner".into(),
                    name: "DeepSeek R1 (reasoning)".into(),
                    context_window: 64_000,
                    supports_tools: false,
                    input_cost_per_m: 0.55,
                    output_cost_per_m: 2.19,
                },
            ],
            ProviderId::Xai => vec![
                ModelInfo {
                    id: "grok-3".into(),
                    name: "Grok 3".into(),
                    context_window: 131_072,
                    supports_tools: true,
                    input_cost_per_m: 3.0,
                    output_cost_per_m: 15.0,
                },
                ModelInfo {
                    id: "grok-3-mini".into(),
                    name: "Grok 3 Mini (thinking)".into(),
                    context_window: 131_072,
                    supports_tools: true,
                    input_cost_per_m: 0.30,
                    output_cost_per_m: 0.50,
                },
                ModelInfo {
                    id: "grok-2-1212".into(),
                    name: "Grok 2".into(),
                    context_window: 131_072,
                    supports_tools: true,
                    input_cost_per_m: 2.0,
                    output_cost_per_m: 10.0,
                },
            ],
            ProviderId::Cohere => vec![
                ModelInfo {
                    id: "command-r-plus-08-2024".into(),
                    name: "Command R+".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 2.50,
                    output_cost_per_m: 10.0,
                },
                ModelInfo {
                    id: "command-r-08-2024".into(),
                    name: "Command R".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 0.15,
                    output_cost_per_m: 0.60,
                },
                ModelInfo {
                    id: "command-a-03-2025".into(),
                    name: "Command A".into(),
                    context_window: 256_000,
                    supports_tools: true,
                    input_cost_per_m: 2.50,
                    output_cost_per_m: 10.0,
                },
            ],
            ProviderId::Perplexity => vec![
                ModelInfo {
                    id: "sonar-pro".into(),
                    name: "Sonar Pro (web search)".into(),
                    context_window: 200_000,
                    supports_tools: false,
                    input_cost_per_m: 3.0,
                    output_cost_per_m: 15.0,
                },
                ModelInfo {
                    id: "sonar".into(),
                    name: "Sonar (web search)".into(),
                    context_window: 200_000,
                    supports_tools: false,
                    input_cost_per_m: 1.0,
                    output_cost_per_m: 1.0,
                },
                ModelInfo {
                    id: "sonar-reasoning-pro".into(),
                    name: "Sonar Reasoning Pro".into(),
                    context_window: 200_000,
                    supports_tools: false,
                    input_cost_per_m: 2.0,
                    output_cost_per_m: 8.0,
                },
            ],
            ProviderId::Azure => vec![
                ModelInfo {
                    id: "gpt-4o".into(),
                    name: "GPT-4o (set base_url to your deployment)".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 2.50,
                    output_cost_per_m: 10.0,
                },
                ModelInfo {
                    id: "gpt-4o-mini".into(),
                    name: "GPT-4o Mini".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 0.15,
                    output_cost_per_m: 0.60,
                },
            ],
            ProviderId::Cerebras => vec![
                ModelInfo {
                    id: "llama3.1-70b".into(),
                    name: "Llama 3.1 70B (ultra-fast)".into(),
                    context_window: 8_192,
                    supports_tools: true,
                    input_cost_per_m: 0.59,
                    output_cost_per_m: 0.59,
                },
                ModelInfo {
                    id: "llama3.1-8b".into(),
                    name: "Llama 3.1 8B (ultra-fast)".into(),
                    context_window: 8_192,
                    supports_tools: true,
                    input_cost_per_m: 0.10,
                    output_cost_per_m: 0.10,
                },
                ModelInfo {
                    id: "llama-4-scout-17b-16e-instruct".into(),
                    name: "Llama 4 Scout 17B".into(),
                    context_window: 131_072,
                    supports_tools: true,
                    input_cost_per_m: 0.27,
                    output_cost_per_m: 0.85,
                },
            ],
            ProviderId::Fireworks => vec![
                ModelInfo {
                    id: "accounts/fireworks/models/llama-v3p1-70b-instruct".into(),
                    name: "Llama 3.1 70B".into(),
                    context_window: 131_072,
                    supports_tools: true,
                    input_cost_per_m: 0.90,
                    output_cost_per_m: 0.90,
                },
                ModelInfo {
                    id: "accounts/fireworks/models/deepseek-v3".into(),
                    name: "DeepSeek V3".into(),
                    context_window: 64_000,
                    supports_tools: true,
                    input_cost_per_m: 0.90,
                    output_cost_per_m: 0.90,
                },
                ModelInfo {
                    id: "accounts/fireworks/models/qwen2p5-coder-32b-instruct".into(),
                    name: "Qwen 2.5 Coder 32B".into(),
                    context_window: 32_768,
                    supports_tools: true,
                    input_cost_per_m: 0.90,
                    output_cost_per_m: 0.90,
                },
            ],
            ProviderId::SambaNova => vec![
                ModelInfo {
                    id: "Meta-Llama-3.1-405B-Instruct".into(),
                    name: "Llama 3.1 405B".into(),
                    context_window: 16_384,
                    supports_tools: true,
                    input_cost_per_m: 5.0,
                    output_cost_per_m: 10.0,
                },
                ModelInfo {
                    id: "Meta-Llama-3.3-70B-Instruct".into(),
                    name: "Llama 3.3 70B".into(),
                    context_window: 131_072,
                    supports_tools: true,
                    input_cost_per_m: 0.60,
                    output_cost_per_m: 1.20,
                },
                ModelInfo {
                    id: "DeepSeek-R1".into(),
                    name: "DeepSeek R1 (reasoning)".into(),
                    context_window: 32_768,
                    supports_tools: false,
                    input_cost_per_m: 5.0,
                    output_cost_per_m: 10.0,
                },
            ],
            ProviderId::GithubModels => vec![
                ModelInfo {
                    id: "gpt-4o".into(),
                    name: "GPT-4o".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 0.0,
                    output_cost_per_m: 0.0,
                },
                ModelInfo {
                    id: "Meta-Llama-3.1-405B-Instruct".into(),
                    name: "Llama 3.1 405B".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 0.0,
                    output_cost_per_m: 0.0,
                },
                ModelInfo {
                    id: "mistral-large".into(),
                    name: "Mistral Large".into(),
                    context_window: 128_000,
                    supports_tools: true,
                    input_cost_per_m: 0.0,
                    output_cost_per_m: 0.0,
                },
                ModelInfo {
                    id: "Phi-4".into(),
                    name: "Phi-4".into(),
                    context_window: 16_384,
                    supports_tools: true,
                    input_cost_per_m: 0.0,
                    output_cost_per_m: 0.0,
                },
            ],
            ProviderId::LmStudio | ProviderId::Ollama => {
                // Dynamic - must query the server
                vec![]
            }
            ProviderId::Custom(_) => vec![],
        }
    }
}

fn default_model_for(id: &ProviderId) -> &str {
    use crate::constants::models;
    match id {
        ProviderId::Claude => models::DEFAULT_CLAUDE_MODEL,
        ProviderId::OpenAI => models::DEFAULT_OPENAI_MODEL,
        ProviderId::Ollama => models::BASE_PLANNER,
        ProviderId::Groq => models::DEFAULT_GROQ_MODEL,
        ProviderId::Together => models::DEFAULT_TOGETHER_MODEL,
        ProviderId::OpenRouter => models::DEFAULT_OPENROUTER_MODEL,
        ProviderId::LmStudio => models::DEFAULT_LMSTUDIO_MODEL,
        ProviderId::Gemini => models::DEFAULT_GEMINI_MODEL,
        ProviderId::Mistral => models::DEFAULT_MISTRAL_MODEL,
        ProviderId::DeepSeek => models::DEFAULT_DEEPSEEK_MODEL,
        ProviderId::Xai => models::DEFAULT_XAI_MODEL,
        ProviderId::Cohere => models::DEFAULT_COHERE_MODEL,
        ProviderId::Perplexity => models::DEFAULT_PERPLEXITY_MODEL,
        ProviderId::Azure => models::DEFAULT_AZURE_MODEL,
        ProviderId::Cerebras => models::DEFAULT_CEREBRAS_MODEL,
        ProviderId::Fireworks => models::DEFAULT_FIREWORKS_MODEL,
        ProviderId::SambaNova => models::DEFAULT_SAMBANOVA_MODEL,
        ProviderId::GithubModels => models::DEFAULT_GITHUB_MODELS_MODEL,
        ProviderId::Custom(_) => "default",
    }
}

/// Token usage tracking for cost estimation.
#[derive(Debug, Clone, Default)]
pub struct UsageTracker {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub request_count: u64,
}

impl UsageTracker {
    pub fn track(&mut self, input: u32, output: u32) {
        self.total_input_tokens += input as u64;
        self.total_output_tokens += output as u64;
        self.request_count += 1;
    }

    pub fn estimated_cost(&self, model: &ModelInfo) -> f64 {
        let input_cost = (self.total_input_tokens as f64 / 1_000_000.0) * model.input_cost_per_m;
        let output_cost = (self.total_output_tokens as f64 / 1_000_000.0) * model.output_cost_per_m;
        input_cost + output_cost
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
