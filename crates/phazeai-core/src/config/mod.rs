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
    pub base_url: Option<String>,
    pub max_tokens: u32,
    /// Conversation tokens the agent may send before compacting older
    /// messages into a summary. Unset: derived from the model's context
    /// window (see `llm::context_budget`).
    #[serde(default)]
    pub context_tokens: Option<usize>,
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
            LlmProvider::BuiltIn => ProviderId::BuiltIn,
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
            },
            editor: EditorSettings::default(),
            search: SearchSettings::default(),
            providers: Vec::new(),
            model_routes: HashMap::new(),
            local_ai: Default::default(),
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

    /// Refuse a `[llm] base_url` that is not a full `http://` or `https://`
    /// address. As written it would fail at the first request, or for Ollama
    /// quietly go to localhost:11434 instead; and guessing a scheme could put
    /// an API key on plain HTTP.
    fn check_base_url(&self) -> Result<(), crate::error::PhazeError> {
        let Some(url) = self.llm.base_url_override() else {
            return Ok(());
        };
        if self.llm.provider == LlmProvider::BuiltIn {
            return Ok(());
        }
        let full = reqwest::Url::parse(url).is_ok_and(|u| {
            matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty())
        });
        if full {
            return Ok(());
        }
        Err(crate::error::PhazeError::Config(format!(
            "[llm] base_url = \"{url}\" in config.toml is not a full http:// or https:// address \
             (for example http://localhost:11434). Fix it, or remove it to use the default \
             address of {}.",
            self.llm.provider.to_provider_id().name()
        )))
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
    const PROVIDERS: [LlmProvider; 9] = [
        LlmProvider::Claude,
        LlmProvider::OpenAI,
        LlmProvider::Ollama,
        LlmProvider::Groq,
        LlmProvider::Together,
        LlmProvider::OpenRouter,
        LlmProvider::LmStudio,
        LlmProvider::Gemini,
        LlmProvider::BuiltIn,
    ];

    fn llm(provider: LlmProvider, base_url: Option<&str>) -> Settings {
        let mut s = Settings::default();
        s.llm.provider = provider;
        s.llm.base_url = base_url.map(str::to_string);
        s
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
}
