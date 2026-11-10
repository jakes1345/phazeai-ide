//! Provider selection and human-friendly setup / error guidance.
//!
//! Nothing in here ever prints a key value — only the *names* of the
//! environment variables / keyring entries a key is looked up under.

use phazeai_core::config::LlmProvider;
use phazeai_core::llm::provider::KEYRING_SERVICE;
use phazeai_core::{ModelInfo, ProviderId, ProviderRegistry, Settings};

/// Provider names accepted by `--provider` and `/provider`.
pub const PROVIDER_NAMES: &[&str] = &[
    "anthropic",
    "openai",
    "gemini",
    "xai",
    "deepseek",
    "mistral",
    "groq",
    "cerebras",
    "together",
    "fireworks",
    "sambanova",
    "openrouter",
    "cohere",
    "azure",
    "ollama",
    "lmstudio",
    "builtin",
];

/// Parse a user-supplied provider name (case-insensitive, with aliases).
pub fn parse_provider(name: &str) -> Option<LlmProvider> {
    Some(match name.trim().to_lowercase().as_str() {
        "claude" | "anthropic" => LlmProvider::Claude,
        "openai" | "gpt" => LlmProvider::OpenAI,
        "ollama" | "local" => LlmProvider::Ollama,
        "groq" => LlmProvider::Groq,
        "together" | "together.ai" => LlmProvider::Together,
        "openrouter" | "or" => LlmProvider::OpenRouter,
        "lmstudio" | "lm-studio" | "lm_studio" => LlmProvider::LmStudio,
        "gemini" | "google" => LlmProvider::Gemini,
        "builtin" | "built-in" | "phazeai" | "phazeai-local" => LlmProvider::BuiltIn,
        "xai" | "grok" | "x.ai" => LlmProvider::Xai,
        "deepseek" => LlmProvider::DeepSeek,
        "mistral" | "mistral.ai" => LlmProvider::Mistral,
        "cohere" => LlmProvider::Cohere,
        "azure" | "azure-openai" | "azure_openai" => LlmProvider::Azure,
        "cerebras" => LlmProvider::Cerebras,
        "fireworks" | "fireworks.ai" => LlmProvider::Fireworks,
        "sambanova" | "samba" => LlmProvider::SambaNova,
        _ => return None,
    })
}

/// Short lowercase label for headers.
pub fn provider_slug(p: &LlmProvider) -> &'static str {
    match p {
        LlmProvider::Claude => "anthropic",
        other => other.slug(),
    }
}

/// Text for `/local`: what the built-in local AI would run, and whether it can.
pub fn local_ai_status_text(settings: &Settings) -> String {
    use phazeai_core::local_ai::{
        self, catalog, download, install, server, HardwareInfo, LOCAL_AI,
    };
    let models_dir = local_ai::models_dir();
    let hw = HardwareInfo::probe(&models_dir);
    let mut out = String::from("PhazeAI Local (built-in llama.cpp)\n");
    out.push_str(&format!(
        "  Server:       {}\n",
        LOCAL_AI.status().describe()
    ));
    match server::selected_model(&settings.local_ai, &models_dir) {
        Ok(e) => {
            let state = if download::is_downloaded(e, &models_dir) {
                "downloaded"
            } else {
                "not downloaded"
            };
            out.push_str(&format!("  Model:        {} ({state})\n", e.display_name));
        }
        Err(e) => out.push_str(&format!("  Model:        {e}\n")),
    }
    let path = std::env::var_os("PATH");
    let bin = install::find_server_binary(
        settings.local_ai.server_path.as_deref(),
        path.as_deref(),
        &local_ai::llama_cpp_dir(),
    );
    match bin {
        Ok(Some(found)) => out.push_str(&format!("  llama-server: {}\n", found.describe())),
        Ok(None) => out.push_str(&format!(
            "  llama-server: not found. {}\n",
            install::manual_install_hint(std::env::consts::OS)
        )),
        Err(e) => out.push_str(&format!("  llama-server: {e}\n")),
    }
    out.push_str(&format!("  Hardware:     {}\n", hw.summary()));
    if let Some(rec) = catalog::recommend(&hw) {
        out.push_str(&format!("  Recommended:  {}\n", rec.display_name));
    }
    out.push_str(&format!("  Models dir:   {}\n", models_dir.display()));
    out.push_str(
        "Download models from the IDE (Settings → Local AI). Run `phazeai --provider builtin` \
         to use one here; the server starts with the CLI and stops when it exits.",
    );
    out
}

/// The provider's default model, as configured in the registry.
pub fn default_model_for(settings: &Settings, p: &LlmProvider) -> String {
    let registry = settings.build_provider_registry();
    registry
        .get_config(&p.to_provider_id())
        .map(|c| c.default_model.clone())
        .unwrap_or_default()
}

/// Name of the env var / keyring entry holding the active provider's key, or
/// `None` for local providers that don't need one.
pub fn active_key_env(settings: &Settings) -> Option<String> {
    let id = settings.llm.provider.to_provider_id();
    if !id.needs_api_key() {
        return None;
    }
    let registry = settings.build_provider_registry();
    let env = registry
        .active_config()
        .map(|c| c.api_key_env.clone())
        .unwrap_or_else(|| id.default_api_key_env().to_string());
    (!env.is_empty()).then_some(env)
}

/// Base URL the active provider will talk to.
pub fn active_base_url(settings: &Settings) -> String {
    settings
        .build_provider_registry()
        .active_config()
        .map(|c| c.base_url.clone())
        .unwrap_or_default()
}

/// Static model metadata (context window, pricing) when the registry knows
/// the model.
pub fn model_info(p: &LlmProvider, model: &str) -> Option<ModelInfo> {
    ProviderRegistry::known_models(&p.to_provider_id())
        .into_iter()
        .find(|m| m.id == model)
}

/// Estimated USD cost of a token count. Uses registry pricing when the model
/// is known, otherwise a conservative per-family estimate; local providers are
/// free.
pub fn estimate_cost(p: &LlmProvider, model: &str, input: u64, output: u64) -> f64 {
    let (inp, out) = if let Some(info) = model_info(p, model) {
        (info.input_cost_per_m, info.output_cost_per_m)
    } else {
        let m = model.to_lowercase();
        match p {
            LlmProvider::Ollama | LlmProvider::LmStudio => (0.0, 0.0),
            LlmProvider::Claude if m.contains("opus") => (15.0, 75.0),
            LlmProvider::Claude if m.contains("haiku") => (0.80, 4.0),
            LlmProvider::Claude => (3.0, 15.0),
            LlmProvider::OpenAI if m.contains("mini") => (0.15, 0.60),
            LlmProvider::OpenAI => (2.50, 10.0),
            LlmProvider::Groq => (0.05, 0.08),
            _ => (0.0, 0.0),
        }
    };
    (input as f64 * inp + output as f64 * out) / 1_000_000.0
}

/// A run's token counts, as the agent reports them: `input` is every prompt
/// token, including the `cache_read` tokens served from the provider's prompt
/// cache and the `cache_write` tokens written to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// [`estimate_cost`] with the prompt cache priced in. Anthropic publishes its
/// cache prices as multiples of the input price: a read costs 0.1x and a
/// 5-minute write 1.25x (newer models read for less, so this is an upper
/// bound). Other providers' cached price differs by model and is not in the
/// model table, so their cache reads are charged at the full input price,
/// which again errs high.
pub fn estimate_cached_cost(p: &LlmProvider, model: &str, tokens: RunTokens) -> f64 {
    if !matches!(p, LlmProvider::Claude) {
        return estimate_cost(p, model, tokens.input, tokens.output);
    }
    let read = tokens.cache_read.min(tokens.input);
    let write = tokens.cache_write.min(tokens.input - read);
    let full_price = tokens.input - read - write;
    let one_input_token = estimate_cost(p, model, 1_000_000, 0) / 1_000_000.0;
    estimate_cost(p, model, full_price, tokens.output)
        + one_input_token * (read as f64 * 0.1 + write as f64 * 1.25)
}

fn config_path_display() -> String {
    let p = Settings::config_path();
    let s = p.display().to_string();
    match dirs::home_dir() {
        Some(home) => {
            let h = home.display().to_string();
            match s.strip_prefix(&h) {
                Some(rest) => format!("~{rest}"),
                None => s,
            }
        }
        None => s,
    }
}

/// Where guidance will be shown, so it names commands that exist there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// The interactive TUI (slash commands available).
    Tui,
    /// `phazeai -p …` (command-line flags only).
    OneShot,
}

impl Surface {
    fn switch_provider(self) -> &'static str {
        match self {
            Surface::Tui => "/provider <name>",
            Surface::OneShot => "--provider <name>",
        }
    }

    fn switch_model(self) -> &'static str {
        match self {
            Surface::Tui => "/model <name>",
            Surface::OneShot => "--model <name>",
        }
    }
}

fn key_instructions(provider_name: &str, env: &str, surface: Surface) -> String {
    let paste = match surface {
        Surface::Tui => format!("Paste your key here:   /key <your-{provider_name}-key>"),
        Surface::OneShot => {
            format!("Run `phazeai` and paste it with:   /key <your-{provider_name}-key>")
        }
    };
    let local = match surface {
        Surface::Tui => "/provider ollama",
        Surface::OneShot => "phazeai --provider ollama",
    };
    format!(
        "To fix it, do one of:\n\
         \n  1. {paste}\n     \
         (stored in the OS keyring under service \"{KEYRING_SERVICE}\", entry \"{env}\")\n\
         \n  2. Set an environment variable, then restart phazeai:\n     \
         macOS / Linux:  export {env}=...\n     \
         Windows:        setx {env} \"...\"   (then open a new terminal)\n\
         \n  3. Use a free local model instead:   {local}\n\
         \nProvider and model are set in {}  ([llm] provider / model).",
        config_path_display()
    )
}

/// Explain a failure from `Settings::build_llm_client()`.
pub fn explain_build_error(settings: &Settings, err: &str, surface: Surface) -> String {
    let id = settings.llm.provider.to_provider_id();
    if let Some(env) = active_key_env(settings) {
        if err.contains(&env) || err.to_lowercase().contains("api key") {
            return format!(
                "No API key found for {}.\n\n{}",
                id.name(),
                key_instructions(provider_slug(&settings.llm.provider), &env, surface)
            );
        }
    }
    format!(
        "Couldn't set up {} ({err}).\n\nCheck {} or pick another provider with {}.",
        id.name(),
        config_path_display(),
        surface.switch_provider()
    )
}

/// Add actionable guidance to a runtime error from the agent, if we recognise
/// it. Returns `None` when the raw error is the best we can do.
pub fn explain_runtime_error(settings: &Settings, err: &str, surface: Surface) -> Option<String> {
    let lower = err.to_lowercase();
    let id = settings.llm.provider.to_provider_id();
    let base = active_base_url(settings);

    let unreachable = lower.contains("connection refused")
        || lower.contains("error sending request")
        || lower.contains("tcp connect")
        || lower.contains("dns error")
        || lower.contains("failed to connect");
    if unreachable && id.is_local() {
        let start = match id {
            ProviderId::Ollama => "Start it with `ollama serve` (install from https://ollama.com),",
            _ => "Start LM Studio and enable its local server,",
        };
        return Some(format!(
            "Couldn't reach {} at {base}.\n{start} or switch to a cloud provider with {}.",
            id.name(),
            surface.switch_provider()
        ));
    }
    if unreachable {
        return Some(format!(
            "Couldn't reach {} at {base}. Check your network connection or proxy settings.",
            id.name()
        ));
    }

    let auth = lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("invalid x-api-key")
        || lower.contains("invalid api key")
        || lower.contains("incorrect api key")
        || lower.contains("authentication");
    if auth {
        if let Some(env) = active_key_env(settings) {
            return Some(format!(
                "{} rejected the API key.\n\n{}",
                id.name(),
                key_instructions(provider_slug(&settings.llm.provider), &env, surface)
            ));
        }
    }

    if lower.contains("404") && (lower.contains("model") || id.is_local()) {
        let hint = if id == ProviderId::Ollama {
            format!("Pull it with `ollama pull {}` or", settings.llm.model)
        } else if surface == Surface::Tui {
            "See /models, then".to_string()
        } else {
            "Check the model name, or".to_string()
        };
        return Some(format!(
            "Model `{}` wasn't found on {}. {hint} switch with {}.",
            settings.llm.model,
            id.name(),
            surface.switch_model()
        ));
    }

    if lower.contains("429") || lower.contains("rate limit") {
        let retry = match surface {
            Surface::Tui => "Wait a moment and /retry.",
            Surface::OneShot => "Wait a moment and try again.",
        };
        return Some(format!("{} is rate-limiting requests. {retry}", id.name()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(p: LlmProvider) -> Settings {
        let mut s = Settings::default();
        s.llm.provider = p;
        s
    }

    #[test]
    fn parses_provider_aliases() {
        assert_eq!(parse_provider("Anthropic"), Some(LlmProvider::Claude));
        assert_eq!(parse_provider("claude"), Some(LlmProvider::Claude));
        assert_eq!(parse_provider("lm-studio"), Some(LlmProvider::LmStudio));
        assert_eq!(parse_provider("gemini"), Some(LlmProvider::Gemini));
        assert_eq!(parse_provider("bogus"), None);
        for name in PROVIDER_NAMES {
            assert!(parse_provider(name).is_some(), "{name}");
        }
        assert_eq!(parse_provider("grok"), Some(LlmProvider::Xai));
        assert_eq!(parse_provider("xAI"), Some(LlmProvider::Xai));
        assert_eq!(parse_provider("deepseek"), Some(LlmProvider::DeepSeek));
        // Every provider can be picked, under the name config.toml stores.
        for provider in LlmProvider::ALL {
            assert_eq!(parse_provider(provider.slug()), Some(provider.clone()));
            let slug = provider_slug(&provider);
            assert!(PROVIDER_NAMES.contains(&slug), "{slug} not listed");
        }
    }

    #[test]
    fn key_env_names() {
        assert_eq!(
            active_key_env(&settings(LlmProvider::Claude)).as_deref(),
            Some("ANTHROPIC_API_KEY")
        );
        assert_eq!(
            active_key_env(&settings(LlmProvider::OpenAI)).as_deref(),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(active_key_env(&settings(LlmProvider::Ollama)), None);
    }

    #[test]
    fn missing_key_message_is_actionable() {
        let s = settings(LlmProvider::Claude);
        let err = "Configuration error: Set ANTHROPIC_API_KEY environment variable for Claude";
        let msg = explain_build_error(&s, err, Surface::Tui);
        assert!(msg.contains("No API key found"));
        assert!(msg.contains("/key"));
        assert!(msg.contains("export ANTHROPIC_API_KEY="));
        assert!(msg.contains("setx ANTHROPIC_API_KEY"));
        assert!(msg.contains("config.toml"));

        let msg = explain_build_error(&s, err, Surface::OneShot);
        assert!(msg.contains("Run `phazeai`"));
        assert!(msg.contains("phazeai --provider ollama"));
    }

    #[test]
    fn runtime_errors_get_hints() {
        let s = settings(LlmProvider::Ollama);
        let msg = explain_runtime_error(
            &s,
            "LLM error: Ollama stream error: error sending request: Connection refused",
            Surface::Tui,
        )
        .unwrap();
        assert!(msg.contains("ollama serve"));

        let s = settings(LlmProvider::OpenAI);
        let msg = explain_runtime_error(&s, "HTTP 401 Unauthorized", Surface::Tui).unwrap();
        assert!(msg.contains("rejected the API key"));
        assert!(msg.contains("OPENAI_API_KEY"));

        assert!(explain_runtime_error(&s, "something odd", Surface::OneShot).is_none());
    }

    #[test]
    fn cost_uses_registry_then_fallback() {
        let c = estimate_cost(&LlmProvider::Claude, "claude-opus-4-6", 1_000_000, 0);
        assert!((c - 15.0).abs() < 1e-9);
        let c = estimate_cost(&LlmProvider::Ollama, "llama3", 1_000_000, 1_000_000);
        assert_eq!(c, 0.0);
        let c = estimate_cost(&LlmProvider::Claude, "claude-unknown-sonnet", 0, 1_000_000);
        assert!((c - 15.0).abs() < 1e-9);
    }

    #[test]
    fn claude_cache_reads_and_writes_are_priced_from_the_input_price() {
        // claude-opus-4-6 is $15 per million input tokens and $75 output.
        let model = "claude-opus-4-6";
        let all_fresh = RunTokens {
            input: 1_000_000,
            ..RunTokens::default()
        };
        let c = estimate_cached_cost(&LlmProvider::Claude, model, all_fresh);
        assert!((c - 15.0).abs() < 1e-9);

        // 800k read at 0.1x, 100k written at 1.25x, 100k fresh, 10k out.
        let cached = RunTokens {
            input: 1_000_000,
            output: 10_000,
            cache_read: 800_000,
            cache_write: 100_000,
        };
        let c = estimate_cached_cost(&LlmProvider::Claude, model, cached);
        let expected = 0.8 * 15.0 * 0.1 + 0.1 * 15.0 * 1.25 + 0.1 * 15.0 + 0.01 * 75.0;
        assert!((c - expected).abs() < 1e-9, "{c} vs {expected}");

        // Elsewhere a cache read is charged at the full input price.
        let c = estimate_cached_cost(&LlmProvider::OpenAI, "gpt-unknown", cached);
        assert!(
            (c - estimate_cost(&LlmProvider::OpenAI, "gpt-unknown", 1_000_000, 10_000)).abs()
                < 1e-9
        );
    }

    #[test]
    fn default_models_exist() {
        let s = Settings::default();
        assert!(!default_model_for(&s, &LlmProvider::OpenAI).is_empty());
        assert!(!default_model_for(&s, &LlmProvider::Claude).is_empty());
    }
}
