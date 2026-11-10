use crate::constants::{defaults, paths};
use crate::llm::model_router::{ModelRoute, ModelRouter, TaskType};
use crate::llm::provider::{ProviderConfig, ProviderId, ProviderRegistry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "RawSettings")]
pub struct Settings {
    pub llm: LlmSettings,
    pub editor: EditorSettings,
    /// Built-in code search. Named `[sidecar]` before the Python subprocess
    /// was replaced; old config files keep loading (see [`RawSettings`]).
    pub search: SearchSettings,
    pub providers: Vec<ProviderEntry>,
    pub model_routes: HashMap<TaskType, ModelRoute>,
    pub local_ai: crate::local_ai::LocalAiSettings,
    pub updates: UpdateSettings,
    pub agent: AgentSettings,
    /// `[permissions]`: tool calls that run unasked, always ask, or never
    /// run (see [`crate::permissions`]).
    #[serde(skip_serializing_if = "crate::permissions::PermissionSettings::is_empty")]
    pub permissions: crate::permissions::PermissionSettings,
    /// `[hooks]`: the user's commands run before and after tool calls (see
    /// [`crate::hooks`]).
    #[serde(skip_serializing_if = "crate::hooks::HookSettings::is_empty")]
    pub hooks: crate::hooks::HookSettings,
}

/// `config.toml` as it is read. Not a field alias: serde refuses a file that
/// holds both an alias and the name it stands for (duplicate field), and a
/// user who adds `[search]` to a config that still has `[sidecar]` would lose
/// every setting to the defaults on the next save. Both tables are accepted,
/// and `[search]` wins.
#[derive(Deserialize)]
struct RawSettings {
    llm: LlmSettings,
    editor: EditorSettings,
    #[serde(default)]
    search: Option<SearchSettings>,
    /// The table `[search]` replaced.
    #[serde(default)]
    sidecar: Option<SearchSettings>,
    #[serde(default)]
    providers: Vec<ProviderEntry>,
    #[serde(default)]
    model_routes: HashMap<TaskType, ModelRoute>,
    #[serde(default)]
    local_ai: crate::local_ai::LocalAiSettings,
    #[serde(default)]
    updates: UpdateSettings,
    #[serde(default)]
    agent: AgentSettings,
    #[serde(default)]
    permissions: crate::permissions::PermissionSettings,
    #[serde(default)]
    hooks: crate::hooks::HookSettings,
}

impl From<RawSettings> for Settings {
    fn from(raw: RawSettings) -> Self {
        Self {
            llm: raw.llm,
            editor: raw.editor,
            search: raw.search.or(raw.sidecar).unwrap_or_default(),
            providers: raw.providers,
            model_routes: raw.model_routes,
            local_ai: raw.local_ai,
            updates: raw.updates,
            agent: raw.agent,
            permissions: raw.permissions,
            hooks: raw.hooks,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSettings {
    pub provider: LlmProvider,
    pub model: String,
    pub api_key_env: String,
    /// Where to reach the active provider instead of its default address: an
    /// Ollama or LM Studio server on another machine or port, or an
    /// OpenAI-compatible proxy. Every client built from these settings sends
    /// its requests here, so `local_only` judges this address. It belongs to
    /// `provider` ([`Settings::set_provider`] drops it on a switch) and is
    /// ignored by the built-in provider, whose server picks its own port.
    /// [`Settings::check_base_url`] says which addresses are refused.
    pub base_url: Option<String>,
    pub max_tokens: u32,
    /// Conversation tokens the agent may send before compacting older
    /// messages into a summary. Unset: derived from the model's context
    /// window (see `llm::context_budget`).
    #[serde(default)]
    pub context_tokens: Option<usize>,
    /// The model for side jobs (inline completions), on the same provider.
    /// Unset: the provider's own cheaper model where Phaze knows one (see
    /// [`crate::llm::small_model_for`]), otherwise `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub small_model: Option<String>,
}

impl LlmSettings {
    /// [`Self::base_url`] as the clients and the IDE's readiness probe read
    /// it: trimmed, and `None` when unset or blank.
    pub fn base_url_override(&self) -> Option<&str> {
        self.base_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
    }
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
    /// PhazeAI's own llama.cpp server (`[local_ai]` section).
    #[serde(rename = "builtin")]
    BuiltIn,
    #[serde(alias = "grok")]
    Xai,
    DeepSeek,
    Mistral,
    Cohere,
    /// Azure OpenAI: `[llm] base_url` names the resource
    /// (`https://<resource>.openai.azure.com/openai/v1`) and `model` the
    /// deployment.
    Azure,
    Cerebras,
    Fireworks,
    SambaNova,
}

impl LlmProvider {
    /// Every provider, in the order pickers list them. Perplexity's Sonar
    /// Chat Completions (ended September 27, 2026) and GitHub Models
    /// (retired July 30, 2026) are gone, so they are not offered.
    pub const ALL: [LlmProvider; 17] = [
        LlmProvider::Claude,
        LlmProvider::OpenAI,
        LlmProvider::Gemini,
        LlmProvider::Xai,
        LlmProvider::DeepSeek,
        LlmProvider::Mistral,
        LlmProvider::Groq,
        LlmProvider::Cerebras,
        LlmProvider::Together,
        LlmProvider::Fireworks,
        LlmProvider::SambaNova,
        LlmProvider::OpenRouter,
        LlmProvider::Cohere,
        LlmProvider::Azure,
        LlmProvider::BuiltIn,
        LlmProvider::Ollama,
        LlmProvider::LmStudio,
    ];

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
            LlmProvider::BuiltIn => ProviderId::BuiltIn,
            LlmProvider::Xai => ProviderId::Xai,
            LlmProvider::DeepSeek => ProviderId::DeepSeek,
            LlmProvider::Mistral => ProviderId::Mistral,
            LlmProvider::Cohere => ProviderId::Cohere,
            LlmProvider::Azure => ProviderId::Azure,
            LlmProvider::Cerebras => ProviderId::Cerebras,
            LlmProvider::Fireworks => ProviderId::Fireworks,
            LlmProvider::SambaNova => ProviderId::SambaNova,
        }
    }

    /// The provider a [`ProviderId`] stands for; `None` for a custom one.
    pub fn from_provider_id(id: &ProviderId) -> Option<LlmProvider> {
        Self::ALL.into_iter().find(|p| &p.to_provider_id() == id)
    }

    /// The name pickers show, the same as [`ProviderId::name`].
    pub fn display_name(&self) -> &'static str {
        match self {
            LlmProvider::Claude => "Claude (Anthropic)",
            LlmProvider::OpenAI => "OpenAI",
            LlmProvider::Ollama => "Ollama (Local)",
            LlmProvider::Groq => "Groq",
            LlmProvider::Together => "Together.ai",
            LlmProvider::OpenRouter => "OpenRouter",
            LlmProvider::LmStudio => "LM Studio (Local)",
            LlmProvider::Gemini => "Google Gemini",
            LlmProvider::BuiltIn => "PhazeAI Local (built-in)",
            LlmProvider::Xai => "xAI (Grok)",
            LlmProvider::DeepSeek => "DeepSeek",
            LlmProvider::Mistral => "Mistral AI",
            LlmProvider::Cohere => "Cohere",
            LlmProvider::Azure => "Azure OpenAI",
            LlmProvider::Cerebras => "Cerebras",
            LlmProvider::Fireworks => "Fireworks AI",
            LlmProvider::SambaNova => "SambaNova",
        }
    }

    /// The provider whose display name ([`Self::display_name`]) is `name`.
    pub fn from_display_name(name: &str) -> Option<LlmProvider> {
        Self::ALL.into_iter().find(|p| p.display_name() == name)
    }

    /// The model to use after switching to this provider from `model`:
    /// `model` itself when it is one of this provider's known models,
    /// otherwise the provider's default. Another provider's model name would
    /// only get the request refused.
    pub fn model_after_switch(&self, model: &str) -> String {
        let id = self.to_provider_id();
        if crate::llm::ProviderRegistry::known_models(&id)
            .iter()
            .any(|m| m.id == model)
        {
            return model.to_string();
        }
        crate::llm::ProviderRegistry::new()
            .get_config(&id)
            .map(|c| c.default_model.clone())
            .unwrap_or_else(|| model.to_string())
    }

    /// The name `config.toml` stores (`provider = "xai"`), which `--provider`
    /// also takes.
    pub fn slug(&self) -> &'static str {
        match self {
            LlmProvider::Claude => "claude",
            LlmProvider::OpenAI => "openai",
            LlmProvider::Ollama => "ollama",
            LlmProvider::Groq => "groq",
            LlmProvider::Together => "together",
            LlmProvider::OpenRouter => "openrouter",
            LlmProvider::LmStudio => "lmstudio",
            LlmProvider::Gemini => "gemini",
            LlmProvider::BuiltIn => "builtin",
            LlmProvider::Xai => "xai",
            LlmProvider::DeepSeek => "deepseek",
            LlmProvider::Mistral => "mistral",
            LlmProvider::Cohere => "cohere",
            LlmProvider::Azure => "azure",
            LlmProvider::Cerebras => "cerebras",
            LlmProvider::Fireworks => "fireworks",
            LlmProvider::SambaNova => "sambanova",
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
    /// Ask the active AI provider for inline completions while you edit
    /// (`crate::fim`). Each request sends the code around the cursor; the
    /// privacy deny rules and `local_only` still apply. Off: nothing is sent.
    pub ai_completions: bool,
    /// The model inline completions ask, where the provider has a
    /// fill-in-the-middle API (`crate::fim`): Ollama, DeepSeek and Mistral.
    /// Unset: `codestral-latest` on Mistral, otherwise `[llm] small_model`'s
    /// choice. A model that turns such requests down is asked with a chat
    /// prompt instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_model: Option<String>,
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
            ai_completions: true,
            completion_model: None,
        }
    }
}

/// The `[updates]` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Ask GitHub's releases API for a newer version when the IDE starts.
    /// Skipped when the folder the IDE starts in is `local_only`; a project
    /// opened after start is not consulted, because the request has already
    /// gone (see `crate::updater::should_check_for_updates`).
    pub check_at_start: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_at_start: true,
        }
    }
}

/// The `[agent]` table: how the AI agent runs (chat, Composer and the CLI).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    /// The most model turns one request may take before the agent stops
    /// with an error. A turn is one model response and the tools it asked
    /// for. Values below 1 count as 1.
    pub max_iterations: usize,
}

impl AgentSettings {
    /// [`Self::max_iterations`], never below 1.
    pub fn max_iterations(&self) -> usize {
        self.max_iterations.max(1)
    }
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            max_iterations: crate::agent::DEFAULT_MAX_ITERATIONS,
        }
    }
}

/// The `[search]` table: the in-process code search (`crate::search`).
///
/// Unknown keys are ignored, so the `python_path` of an old `[sidecar]` table
/// is dropped on load and not written back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    /// Offer code search in the IDE and to the agent (the `code_search` and
    /// `build_search_index` tools).
    pub enabled: bool,
    /// Build the index when the IDE opens a workspace that looks like a
    /// project (a version-controlled tree or one with a build manifest; never
    /// `/` or a home directory) instead of on the first search. The CLI always
    /// indexes on the first search.
    #[serde(alias = "auto_start")]
    pub auto_index: bool,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_index: true,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            llm: LlmSettings {
                provider: LlmProvider::Ollama,
                model: defaults::DEFAULT_MODEL.to_string(),
                api_key_env: "".to_string(),
                base_url: None,
                max_tokens: defaults::MAX_TOKENS,
                context_tokens: None,
                small_model: None,
            },
            editor: EditorSettings::default(),
            search: SearchSettings::default(),
            providers: Vec::new(),
            model_routes: HashMap::new(),
            local_ai: Default::default(),
            updates: UpdateSettings::default(),
            agent: AgentSettings::default(),
            permissions: Default::default(),
            hooks: Default::default(),
        }
    }
}

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

    pub fn load() -> Self {
        let config_path = Self::config_path();
        if config_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(config) = toml::from_str(&content) {
                    return config;
                }
                eprintln!(
                    "Warning: failed to parse settings at '{}'; using defaults",
                    config_path.display()
                );
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

    /// Make `provider` the active provider. `[llm] base_url` is the address
    /// of the provider it was written for, so switching to another provider
    /// drops it: kept, it would receive the new provider's requests and API
    /// key.
    pub fn set_provider(&mut self, provider: LlmProvider) {
        if self.llm.provider != provider {
            self.llm.base_url = None;
        }
        self.llm.provider = provider;
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
                "builtin" | "built-in" | "phazeai-local" => ProviderId::BuiltIn,
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

        // `[llm] context_tokens` fixes the budget; a local server must then
        // be run with a window that holds it.
        if let Some(budget) = self.llm.context_tokens {
            registry.set_local_window(crate::llm::context_budget::window_for_budget(
                budget.max(crate::llm::context_budget::MIN_BUDGET),
                self.llm.max_tokens as usize,
            ));
        }

        // Set active provider from legacy settings. `[llm] base_url` moves it
        // off its default address; the built-in provider's address is the
        // port its server was started on.
        let provider_id = self.llm.provider.to_provider_id();
        if let Some(url) = self.llm.base_url_override() {
            if provider_id != ProviderId::BuiltIn {
                registry.set_base_url(&provider_id, url);
            }
        }
        registry.set_active(provider_id, self.llm.model.clone());

        registry
    }

    /// Conversation tokens the agent may send to the active model before
    /// compacting. Honours `[llm] context_tokens` when set.
    pub fn context_budget(&self) -> usize {
        use crate::llm::context_budget::{budget_for_window, context_budget};
        let provider = self.llm.provider.to_provider_id();
        if provider == crate::llm::ProviderId::BuiltIn {
            // We start the server ourselves, so the window is known exactly.
            return budget_for_window(
                self.local_ai.effective_ctx_size() as usize,
                self.llm.max_tokens as usize,
                self.llm.context_tokens,
            );
        }
        context_budget(
            &provider,
            &self.llm.model,
            self.llm.max_tokens as usize,
            self.llm.context_tokens,
        )
    }

    /// Build an LLM client from the current settings: the active provider, at
    /// `[llm] base_url` when that is set, behind the model router when
    /// `[model_routes]` are configured. Every client is wrapped in the privacy
    /// guard.
    pub fn build_llm_client(
        &self,
    ) -> Result<Box<dyn crate::llm::LlmClient>, crate::error::PhazeError> {
        self.check_base_url()?;
        let registry = self.build_provider_registry();
        let default_client = registry.build_active_client()?;

        if self.model_routes.is_empty() {
            Ok(default_client)
        } else {
            let router = ModelRouter::new(&self.model_routes, &registry, default_client);
            Ok(Box::new(router))
        }
    }

    /// The provider and model the agent's requests are billed as, for a cost
    /// meter. `None` with `[model_routes]`: a call may then go to any of the
    /// routed models, and its usage does not say which.
    pub fn billed_model(&self) -> Option<(crate::llm::ProviderId, String)> {
        self.model_routes
            .is_empty()
            .then(|| (self.llm.provider.to_provider_id(), self.llm.model.clone()))
    }

    /// The model side jobs run on: `[llm] small_model` when set; otherwise
    /// the active provider's own cheaper model ([`crate::llm::small_model_for`])
    /// unless the price list shows the main model costs no more; otherwise the
    /// main model. A side job never moves to another provider, so it goes
    /// where the main model's requests go.
    pub fn small_model(&self) -> String {
        if let Some(model) = self
            .llm
            .small_model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            return model.to_string();
        }
        let provider = self.llm.provider.to_provider_id();
        let Some(small) = crate::llm::small_model_for(&provider) else {
            return self.llm.model.clone();
        };
        let price = |model: &str| {
            crate::cost::price(&provider, model, 0)
                .map(|p| p.input)
                .or_else(|| {
                    crate::llm::ProviderRegistry::known_models(&provider)
                        .into_iter()
                        .find(|m| m.id == model)
                        .map(|m| m.input_cost_per_m)
                })
        };
        match (price(&self.llm.model), price(small)) {
            (Some(main), Some(cheap)) if main <= cheap => self.llm.model.clone(),
            _ => small.to_string(),
        }
    }

    /// The active provider's models as its own model list gives them (see
    /// [`crate::llm::model_list`]), at the address and with the key the
    /// agent uses. A day-old list is reused unless `refresh` is set.
    pub async fn live_models(
        &self,
        refresh: bool,
    ) -> Result<crate::llm::model_list::ModelList, crate::error::PhazeError> {
        let registry = self.build_provider_registry();
        let config = registry.active_config().cloned().ok_or_else(|| {
            crate::error::PhazeError::Config("No provider is configured.".to_string())
        })?;
        let base_url = crate::llm::provider::request_base_url(&config)?;
        let key = config.api_key();
        crate::llm::model_list::live_models(&config.id, &base_url, key.as_deref(), refresh).await
    }

    /// A client for side jobs: the active provider at the same address as
    /// [`Self::build_llm_client`], with [`Self::small_model`], and never
    /// through `[model_routes]` (those route the agent's own requests).
    pub fn build_small_llm_client(
        &self,
    ) -> Result<Box<dyn crate::llm::LlmClient>, crate::error::PhazeError> {
        self.check_base_url()?;
        let registry = self.build_provider_registry();
        let config = registry.active_config().ok_or_else(|| {
            crate::error::PhazeError::Config(format!(
                "Provider {:?} not configured",
                self.llm.provider.to_provider_id()
            ))
        })?;
        registry.build_client_for(config, &self.small_model())
    }

    /// How `subagent` helpers run for a project at `root`: on
    /// [`Self::small_model`] through [`Self::build_small_llm_client`] (made
    /// when a question is asked, so a client that can't be built only fails
    /// that call), with that model's context budget.
    pub fn subagent_config(&self, root: Option<&std::path::Path>) -> crate::SubagentConfig {
        let model = self.small_model();
        let budget = if self.llm.provider.to_provider_id() == crate::llm::ProviderId::BuiltIn {
            self.context_budget()
        } else {
            crate::llm::context_budget::context_budget(
                &self.llm.provider.to_provider_id(),
                &model,
                self.llm.max_tokens as usize,
                self.llm.context_tokens,
            )
        };
        let settings = self.clone();
        let config = crate::SubagentConfig::new(
            std::sync::Arc::new(move || settings.build_small_llm_client()),
            model,
            budget,
        );
        match root {
            Some(root) => config.with_root(root),
            None => config,
        }
    }

    /// The permission rules and hooks for a run in `root`: the user's
    /// `[permissions]` and `[hooks]`, plus the project's own `deny` and `ask`
    /// rules (see [`crate::permissions`]).
    pub fn permissions(&self, root: Option<&std::path::Path>) -> crate::Permissions {
        crate::Permissions::load(&self.permissions, &self.hooks, root)
    }

    /// Refuse a `[llm] base_url` the clients must not use. Anything that sends
    /// the active provider's key to `[llm] base_url` checks this first.
    ///
    /// - Not a full `http://` or `https://` address: as written it would fail
    ///   at the first request, or for Ollama quietly go to localhost:11434
    ///   instead; and guessing a scheme could put an API key on plain HTTP.
    /// - Plain `http://` to another machine for a provider that needs a key:
    ///   the key and the prompts would cross the network unencrypted. Older
    ///   builds kept `base_url` when the provider changed, so an Ollama or LM
    ///   Studio address left behind looks exactly like this.
    /// - None, for Azure OpenAI with no `[[providers]]` address either: it
    ///   has no address of its own, as each resource has its own host.
    pub fn check_base_url(&self) -> Result<(), crate::error::PhazeError> {
        let id = self.llm.provider.to_provider_id();
        let Some(url) = self.llm.base_url_override() else {
            let registry = self.build_provider_registry();
            let address = registry.active_config().map(|c| c.base_url.trim());
            if id != ProviderId::BuiltIn && address.is_none_or(str::is_empty) {
                return Err(crate::error::PhazeError::Config(format!(
                    "{} has no address of its own. Set [llm] base_url in config.toml to your \
                     resource's address, for example \
                     https://<resource>.openai.azure.com/openai/v1, and [llm] model to your \
                     deployment's name.",
                    id.name()
                )));
            }
            return Ok(());
        };
        if id == ProviderId::BuiltIn {
            return Ok(());
        }
        let Some(parsed) = reqwest::Url::parse(url).ok().filter(|u| {
            matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty())
        }) else {
            return Err(crate::error::PhazeError::Config(format!(
                "[llm] base_url = \"{url}\" in config.toml is not a full http:// or https:// \
                 address (for example http://localhost:11434). Fix it, or remove it to use the \
                 default address of {}.",
                id.name()
            )));
        };
        let host = parsed.host_str().unwrap_or_default();
        if parsed.scheme() == "http"
            && id.needs_api_key()
            && !crate::privacy::is_loopback_host(host)
        {
            // Not "API key": the CLI reads that as a missing key
            // (`setup::explain_build_error`).
            return Err(crate::error::PhazeError::Config(format!(
                "[llm] base_url = \"{url}\" in config.toml would send your {name} key and \
                 prompts to {host} unencrypted. If it is left from a local server you used \
                 before, remove the line to use {name}'s own address; otherwise use an \
                 https:// address or a server on this machine.",
                name = id.name()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config.toml as the Python-sidecar releases wrote it.
    const OLD_CONFIG: &str = r#"
[llm]
provider = "ollama"
model = "qwen2.5-coder:7b"
api_key_env = ""
max_tokens = 4096

[editor]
theme = "Midnight Blue"
font_size = 15.0

[sidecar]
enabled = false
python_path = "/opt/python/bin/python3.12"
auto_start = false
"#;

    #[test]
    fn defaults_enable_search_and_indexing() {
        let s = Settings::default();
        assert!(s.search.enabled);
        assert!(s.search.auto_index);
    }

    #[test]
    fn old_sidecar_table_loads_with_values_preserved() {
        let s: Settings = toml::from_str(OLD_CONFIG).unwrap();
        assert!(!s.search.enabled);
        assert!(!s.search.auto_index);
        assert_eq!(s.llm.model, "qwen2.5-coder:7b");
        assert_eq!(s.llm.max_tokens, 4096);
        assert_eq!(s.editor.font_size, 15.0);
    }

    #[test]
    fn old_sidecar_keys_map_one_to_one() {
        // Each old key must land on its own new key, not just "both false".
        let enabled_only = OLD_CONFIG.replace("enabled = false", "enabled = true");
        let s: Settings = toml::from_str(&enabled_only).unwrap();
        assert!(s.search.enabled);
        assert!(!s.search.auto_index);

        let auto_only = OLD_CONFIG.replace("auto_start = false", "auto_start = true");
        let s: Settings = toml::from_str(&auto_only).unwrap();
        assert!(!s.search.enabled);
        assert!(s.search.auto_index);
    }

    #[test]
    fn old_config_is_rewritten_as_a_search_table() {
        let s: Settings = toml::from_str(OLD_CONFIG).unwrap();
        let saved = toml::to_string_pretty(&s).unwrap();
        let doc: toml::Table = toml::from_str(&saved).unwrap();
        assert!(!doc.contains_key("sidecar"), "{saved}");
        let search = doc["search"].as_table().expect("[search] table");
        let mut keys: Vec<_> = search.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["auto_index", "enabled"], "{saved}");
        assert!(!saved.contains("python"), "{saved}");

        let again: Settings = toml::from_str(&saved).unwrap();
        assert!(!again.search.enabled);
        assert!(!again.search.auto_index);
    }

    #[test]
    fn search_table_round_trips() {
        let mut s = Settings::default();
        s.search.enabled = false;
        s.search.auto_index = true;
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert!(!back.search.enabled);
        assert!(back.search.auto_index);

        s.search.enabled = true;
        s.search.auto_index = false;
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert!(back.search.enabled);
        assert!(!back.search.auto_index);
    }

    #[test]
    fn both_tables_at_once_keep_the_whole_config_and_search_wins() {
        // A hand edit of an old config: [search] added, [sidecar] left behind.
        let both = format!("{OLD_CONFIG}\n[search]\nenabled = true\nauto_index = true\n");
        let s: Settings = toml::from_str(&both).expect("both tables parse");
        assert_eq!(s.llm.model, "qwen2.5-coder:7b");
        assert_eq!(s.editor.font_size, 15.0);
        assert!(s.search.enabled && s.search.auto_index, "[search] wins");

        // The other way round, the old table is the one that disagrees.
        let off = format!("{OLD_CONFIG}\n[search]\nenabled = false\n");
        let s: Settings = toml::from_str(&off).unwrap();
        assert!(!s.search.enabled);
        assert!(
            s.search.auto_index,
            "keys missing from [search] are defaults, not the old table's"
        );

        // And it is written back as the one [search] table.
        let saved = toml::to_string_pretty(&s).unwrap();
        let doc: toml::Table = toml::from_str(&saved).unwrap();
        assert!(!doc.contains_key("sidecar"), "{saved}");
        assert!(doc.contains_key("search"), "{saved}");
    }

    #[test]
    fn a_config_without_the_required_tables_is_still_refused() {
        assert!(toml::from_str::<Settings>("[sidecar]\nenabled = false\n").is_err());
        assert!(toml::from_str::<Settings>("[llm]\nprovider = \"ollama\"\n").is_err());
    }

    // ── [llm] base_url ───────────────────────────────────────────────────────
    // That the clients really send to it is tested end to end, against a
    // scripted server, in tests/llm_base_url.rs.

    /// Every `[llm] provider` value.
    const PROVIDERS: [LlmProvider; 17] = LlmProvider::ALL;

    fn llm(provider: LlmProvider, base_url: Option<&str>) -> Settings {
        let mut s = Settings::default();
        s.llm.provider = provider;
        s.llm.base_url = base_url.map(str::to_string);
        s
    }

    fn with_model(provider: LlmProvider, model: &str, small: Option<&str>) -> Settings {
        let mut s = llm(provider, None);
        s.llm.model = model.to_string();
        s.llm.small_model = small.map(str::to_string);
        s
    }

    #[test]
    fn side_jobs_use_the_providers_cheaper_model() {
        // The family's small model when the main one costs more.
        let sonnet = with_model(LlmProvider::Claude, "claude-sonnet-4-5-20250929", None);
        assert_eq!(sonnet.small_model(), "claude-haiku-4-5-20251001");
        let gpt = with_model(LlmProvider::OpenAI, "gpt-6-sol", None);
        assert_eq!(gpt.small_model(), "gpt-6-luna");
        let luna = with_model(LlmProvider::OpenAI, "gpt-6-luna", None);
        assert_eq!(luna.small_model(), "gpt-6-luna");
        // A main model Phaze has no price for: the small one.
        let newer = with_model(LlmProvider::Claude, "claude-opus-9", None);
        assert_eq!(newer.small_model(), "claude-haiku-4-5-20251001");
        // Already the small model, or no dearer than it: unchanged.
        let haiku = with_model(LlmProvider::Claude, "claude-haiku-4-5-20251001", None);
        assert_eq!(haiku.small_model(), "claude-haiku-4-5-20251001");
        let flash = with_model(LlmProvider::Gemini, "gemini-2.0-flash-lite", None);
        assert_eq!(flash.small_model(), "gemini-2.0-flash-lite");
        // Local servers and multi-vendor hosts keep the main model.
        let local = with_model(LlmProvider::Ollama, "qwen2.5-coder:7b", None);
        assert_eq!(local.small_model(), "qwen2.5-coder:7b");
        let routed = with_model(LlmProvider::OpenRouter, "anthropic/claude-sonnet-4-5", None);
        assert_eq!(routed.small_model(), "anthropic/claude-sonnet-4-5");
        // `[llm] small_model` wins; blank means unset.
        let chosen = with_model(LlmProvider::Ollama, "big", Some(" tiny "));
        assert_eq!(chosen.small_model(), "tiny");
        let blank = with_model(LlmProvider::Claude, "claude-sonnet-4-5-20250929", Some(" "));
        assert_eq!(blank.small_model(), "claude-haiku-4-5-20251001");
    }

    #[test]
    fn grok_side_jobs_use_xais_coding_model() {
        let grok = with_model(LlmProvider::Xai, "grok-4.7", None);
        assert_eq!(grok.small_model(), "grok-build-0.1");
        // grok-4.3 costs more than grok-build-0.1 too.
        let big = with_model(LlmProvider::Xai, "grok-4.3", None);
        assert_eq!(big.small_model(), "grok-build-0.1");
    }

    #[test]
    fn every_provider_is_stored_under_its_slug_and_read_back() {
        for provider in LlmProvider::ALL {
            let s = llm(provider.clone(), None);
            let text = toml::to_string(&s).unwrap();
            assert!(
                text.contains(&format!("provider = \"{}\"", provider.slug())),
                "{provider:?}: {text}"
            );
            let back: Settings = toml::from_str(&text).unwrap();
            assert_eq!(back.llm.provider, provider);
        }
        // What `[[providers]]` entries already accept for xAI.
        let grok: Settings = toml::from_str(
            &toml::to_string(&llm(LlmProvider::Claude, None))
                .unwrap()
                .replace("provider = \"claude\"", "provider = \"grok\""),
        )
        .unwrap();
        assert_eq!(grok.llm.provider, LlmProvider::Xai);
    }

    #[test]
    fn pickers_name_each_provider_as_the_registry_does() {
        for provider in LlmProvider::ALL {
            let id = provider.to_provider_id();
            assert_eq!(provider.display_name(), id.name());
            assert_eq!(
                LlmProvider::from_display_name(id.name()),
                Some(provider.clone())
            );
            assert_eq!(LlmProvider::from_provider_id(&id), Some(provider));
        }
        // Gone from the pickers: both services have shut down.
        assert_eq!(LlmProvider::from_provider_id(&ProviderId::Perplexity), None);
        assert_eq!(
            LlmProvider::from_provider_id(&ProviderId::GithubModels),
            None
        );
    }

    #[test]
    fn switching_provider_drops_a_model_the_new_one_does_not_serve() {
        // A Claude model would only be refused by xAI.
        assert_eq!(
            LlmProvider::Xai.model_after_switch("claude-sonnet-4-5-20250929"),
            "grok-build-0.1"
        );
        // One of xAI's own models stays.
        assert_eq!(LlmProvider::Xai.model_after_switch("grok-4.3"), "grok-4.3");
        assert_eq!(
            LlmProvider::DeepSeek.model_after_switch("gpt-4o"),
            "deepseek-flash"
        );
    }

    #[test]
    fn azure_needs_its_resource_address() {
        let mut s = llm(LlmProvider::Azure, None);
        let err = s.check_base_url().unwrap_err().to_string();
        assert!(
            err.contains("base_url") && err.contains("openai.azure.com"),
            "{err}"
        );
        assert!(!err.to_lowercase().contains("api key"), "{err}");
        s.llm.base_url = Some("https://my-res.openai.azure.com/openai/v1".into());
        assert!(s.check_base_url().is_ok());
        // Every other provider has an address of its own.
        for provider in LlmProvider::ALL {
            if provider != LlmProvider::Azure {
                assert!(
                    llm(provider.clone(), None).check_base_url().is_ok(),
                    "{provider:?}"
                );
            }
        }
    }

    #[test]
    fn small_model_round_trips_and_stays_out_of_config_when_unset() {
        let unset = toml::to_string(&with_model(LlmProvider::Claude, "m", None)).unwrap();
        assert!(!unset.contains("small_model"), "{unset}");
        let set = with_model(LlmProvider::Claude, "m", Some("claude-haiku-4-5-20251001"));
        let back: Settings = toml::from_str(&toml::to_string(&set).unwrap()).unwrap();
        assert_eq!(
            back.llm.small_model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
    }

    /// The address the active provider's client is built for.
    fn active_url(s: &Settings) -> String {
        let registry = s.build_provider_registry();
        registry
            .active_config()
            .expect("active provider")
            .base_url
            .clone()
    }

    #[test]
    fn without_a_base_url_every_provider_keeps_its_default_address() {
        for provider in PROVIDERS {
            let default = provider.to_provider_id().default_base_url().to_string();
            for unset in [None, Some(""), Some(" \t ")] {
                let s = llm(provider.clone(), unset);
                assert_eq!(active_url(&s), default, "{provider:?}, base_url {unset:?}");
            }
        }
    }

    #[test]
    fn base_url_moves_the_active_provider_and_nothing_else() {
        let mut s = llm(LlmProvider::Ollama, Some(" http://gpu-box:11434 "));
        s.providers.push(ProviderEntry {
            name: "myproxy".into(),
            enabled: true,
            api_key_env: "MYPROXY_KEY".into(),
            base_url: "https://proxy.example/v1".into(),
            default_model: "m".into(),
        });
        let registry = s.build_provider_registry();
        let url = |id: ProviderId| registry.get_config(&id).unwrap().base_url.clone();
        assert_eq!(url(ProviderId::Ollama), "http://gpu-box:11434", "trimmed");
        for other in [ProviderId::OpenAI, ProviderId::LmStudio, ProviderId::Claude] {
            assert_eq!(url(other.clone()), other.default_base_url(), "{other:?}");
        }
        assert_eq!(
            url(ProviderId::Custom("myproxy".into())),
            "https://proxy.example/v1",
            "a [[providers]] entry keeps its own address"
        );

        // The built-in provider is reached at the port its server started on.
        let s = llm(LlmProvider::BuiltIn, Some("http://gpu-box:11434"));
        assert_eq!(active_url(&s), "");
    }

    #[test]
    fn a_base_url_that_is_not_a_full_http_address_is_refused() {
        for bad in [
            "192.168.1.5:11434",
            "gpu-box:11434",
            "localhost:11434",
            "ftp://gpu-box/v1",
            "http://",
        ] {
            for provider in [LlmProvider::Ollama, LlmProvider::LmStudio] {
                let err = match llm(provider.clone(), Some(bad)).build_llm_client() {
                    Ok(_) => panic!("{provider:?} with base_url {bad:?} must be refused"),
                    Err(e) => e.to_string(),
                };
                assert!(err.contains("base_url") && err.contains(bad), "{err}");
            }
        }
        // Building sends nothing; a full address on this machine is accepted
        // whatever the privacy policy of a concurrently running test.
        let ok = llm(LlmProvider::Ollama, Some("http://localhost:11434/"));
        assert!(ok.build_llm_client().is_ok());
    }

    #[test]
    fn a_key_never_goes_to_another_machine_unencrypted() {
        // What older builds left behind: the address of an Ollama or LM Studio
        // on another machine, kept when the provider changed. As configured,
        // the provider's key and the prompts would go there over plain HTTP.
        for provider in PROVIDERS {
            if !provider.to_provider_id().needs_api_key() {
                continue;
            }
            for url in [
                "http://192.0.2.2:11434",
                "http://gpu-box:1234/v1",
                "http://api.openai.com/v1",
            ] {
                let err = match llm(provider.clone(), Some(url)).build_llm_client() {
                    Ok(_) => panic!("{provider:?} must not send its key to {url}"),
                    Err(e) => e.to_string(),
                };
                assert!(
                    err.contains("base_url") && err.contains(url) && err.contains("unencrypted"),
                    "{provider:?}, {url}: {err}"
                );
                assert!(
                    !err.to_lowercase().contains("api key"),
                    "the CLI would report a missing key: {err}"
                );
            }
        }

        // Still accepted: a server on this machine, https, and the providers
        // that need no key (an Ollama on another machine is what base_url is
        // for).
        for (provider, url) in [
            (LlmProvider::OpenAI, "http://127.0.0.1:8080/v1"),
            (LlmProvider::OpenAI, "http://localhost:1234/v1"),
            (LlmProvider::Claude, "http://[::1]:8080"),
            (LlmProvider::Claude, "https://gateway.example/anthropic"),
            (LlmProvider::Groq, "https://192.0.2.2/v1"),
            (LlmProvider::Ollama, "http://192.0.2.2:11434"),
            (LlmProvider::LmStudio, "http://gpu-box:1234/v1"),
        ] {
            let s = llm(provider.clone(), Some(url));
            assert!(s.check_base_url().is_ok(), "{provider:?}, {url}");
            assert_eq!(active_url(&s), url, "{provider:?}");
        }
    }

    #[test]
    fn switching_provider_drops_the_old_providers_base_url() {
        let mut s = llm(LlmProvider::Ollama, Some("http://gpu-box:11434"));
        s.set_provider(LlmProvider::Ollama);
        assert_eq!(
            s.llm.base_url.as_deref(),
            Some("http://gpu-box:11434"),
            "the same provider keeps it"
        );

        s.set_provider(LlmProvider::Claude);
        assert_eq!(s.llm.provider, LlmProvider::Claude);
        assert_eq!(
            s.llm.base_url, None,
            "Claude's requests and key must not go to the Ollama server"
        );
        assert_eq!(active_url(&s), ProviderId::Claude.default_base_url());
    }

    #[test]
    fn missing_or_partial_search_table_falls_back_to_defaults() {
        let without = OLD_CONFIG.split("[sidecar]").next().unwrap();
        let s: Settings = toml::from_str(without).unwrap();
        assert!(s.search.enabled);
        assert!(s.search.auto_index);

        let partial = format!("{without}[search]\nauto_index = false\n");
        let s: Settings = toml::from_str(&partial).unwrap();
        assert!(s.search.enabled);
        assert!(!s.search.auto_index);
    }

    #[test]
    fn old_config_without_the_new_keys_loads_with_their_defaults() {
        // OLD_CONFIG predates `[editor] ai_completions` and `[updates]`: both
        // stay on, as they were before there was a setting.
        let s: Settings = toml::from_str(OLD_CONFIG).expect("an old config still loads");
        assert!(s.editor.ai_completions);
        assert!(s.updates.check_at_start);
        assert_eq!(s.editor.theme, "Midnight Blue", "the keys it has are kept");
        // The editor keys it leaves out get their own defaults.
        assert!(!s.editor.organize_imports_on_save);
        assert!(s.editor.inlay_hints);
        assert!(s.editor.code_lens);

        // An `[updates]` table without the key is the default too.
        let s: Settings = toml::from_str(&format!("{OLD_CONFIG}\n[updates]\n")).unwrap();
        assert!(s.updates.check_at_start);
    }

    #[test]
    fn the_agent_turn_limit_defaults_to_the_agent_default_and_round_trips() {
        let s: Settings = toml::from_str(OLD_CONFIG).unwrap();
        assert_eq!(
            s.agent.max_iterations(),
            crate::agent::DEFAULT_MAX_ITERATIONS
        );

        let s: Settings =
            toml::from_str(&format!("{OLD_CONFIG}\n[agent]\nmax_iterations = 50\n")).unwrap();
        assert_eq!(s.agent.max_iterations(), 50);
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(back.agent.max_iterations(), 50);

        // Zero would stop every request before it starts.
        let s: Settings =
            toml::from_str(&format!("{OLD_CONFIG}\n[agent]\nmax_iterations = 0\n")).unwrap();
        assert_eq!(s.agent.max_iterations(), 1);
    }

    #[test]
    fn ai_completions_and_the_update_check_stay_off_once_turned_off() {
        let mut s = Settings::default();
        assert!(s.editor.ai_completions, "on by default");
        assert!(s.updates.check_at_start, "on by default");

        s.editor.ai_completions = false;
        s.updates.check_at_start = false;
        let saved = toml::to_string_pretty(&s).unwrap();
        let doc: toml::Table = toml::from_str(&saved).unwrap();
        assert_eq!(
            doc["editor"]["ai_completions"].as_bool(),
            Some(false),
            "{saved}"
        );
        assert_eq!(
            doc["updates"]["check_at_start"].as_bool(),
            Some(false),
            "{saved}"
        );

        let back: Settings = toml::from_str(&saved).unwrap();
        assert!(!back.editor.ai_completions);
        assert!(!back.updates.check_at_start);
        assert_eq!(
            back.llm.model, s.llm.model,
            "the rest of the file round-trips"
        );
    }
}
