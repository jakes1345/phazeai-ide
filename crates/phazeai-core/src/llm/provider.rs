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
            Self::Custom(_) => "",
        }
    }

    pub fn all_builtin() -> Vec<ProviderId> {
        vec![
            Self::Claude,
            Self::OpenAI,
            Self::Ollama,
            Self::Groq,
            Self::Together,
            Self::OpenRouter,
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

impl ProviderConfig {
    pub fn api_key(&self) -> Option<String> {
        if self.api_key_env.is_empty() {
            return None;
        }
        std::env::var(&self.api_key_env).ok()
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
            active_model: "claude-sonnet-4-5-20250929".to_string(),
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
            ProviderId::LmStudio | ProviderId::Ollama => {
                // Dynamic - must query the server
                vec![]
            }
            ProviderId::Custom(_) => vec![],
        }
    }
}

fn default_model_for(id: &ProviderId) -> &str {
    match id {
        ProviderId::Claude => "claude-sonnet-4-5-20250929",
        ProviderId::OpenAI => "gpt-4o",
        ProviderId::Ollama => "phaze-beast",
        ProviderId::Groq => "llama-3.3-70b-versatile",
        ProviderId::Together => "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        ProviderId::OpenRouter => "anthropic/claude-sonnet-4-5-20250929",
        ProviderId::LmStudio => "default",
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
