//! How much conversation the agent may send to a model.
//!
//! The budget is the model's context window minus room for its reply and for
//! the tool schemas, so that the request never exceeds the window. Known
//! models use the registry's window; unknown ones get a conservative
//! per-provider figure. A too-small budget only means earlier compaction,
//! while a too-large one makes the provider reject the request, so every
//! fallback errs on the small side. `[llm] context_tokens` in config.toml
//! overrides all of this.

use crate::llm::provider::{ProviderId, ProviderRegistry};

/// Smallest budget ever returned; below this the agent can't work at all.
pub const MIN_BUDGET: usize = 4_096;
/// Tokens kept free for tool schemas and framing.
const SCHEMA_RESERVE: usize = 2_000;

/// Context window of `model` on `provider`, in tokens.
pub fn context_window(provider: &ProviderId, model: &str) -> usize {
    if let Some(info) = ProviderRegistry::known_models(provider)
        .into_iter()
        .find(|m| m.id == model)
    {
        return info.context_window;
    }
    match provider {
        // Every current Claude model has at least a 200k window.
        ProviderId::Claude => 200_000,
        // Every current OpenAI chat model has at least a 128k window.
        ProviderId::OpenAI | ProviderId::Azure | ProviderId::GithubModels => 128_000,
        // Local servers default to small windows (Ollama: 4k–8k unless the
        // model file raises num_ctx). PhazeAI's own model files use 8192.
        ProviderId::Ollama | ProviderId::LmStudio => 8_192,
        // Hosted open models: sizes vary per model, so stay conservative.
        _ => 32_000,
    }
}

/// Tokens the agent may fill with conversation before compacting.
///
/// `max_output_tokens` is the reply length the request asks for; `override`
/// is the user's `[llm] context_tokens`, taken as-is when set.
pub fn context_budget(
    provider: &ProviderId,
    model: &str,
    max_output_tokens: usize,
    override_tokens: Option<usize>,
) -> usize {
    budget_for_window(
        context_window(provider, model),
        max_output_tokens,
        override_tokens,
    )
}

/// [`context_budget`] for a context window already known exactly (the
/// built-in server is started with a specific `-c` size).
pub fn budget_for_window(
    window: usize,
    max_output_tokens: usize,
    override_tokens: Option<usize>,
) -> usize {
    if let Some(explicit) = override_tokens {
        return explicit.max(MIN_BUDGET);
    }
    // Never let the reply reserve eat more than a quarter of a small window.
    let reply = max_output_tokens.min(window / 4);
    window
        .saturating_sub(reply + SCHEMA_RESERVE)
        .max(MIN_BUDGET)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_models_use_the_registry_window() {
        let claude = ProviderRegistry::known_models(&ProviderId::Claude);
        let first = &claude[0];
        assert_eq!(
            context_window(&ProviderId::Claude, &first.id),
            first.context_window
        );
        let budget = context_budget(&ProviderId::Claude, &first.id, 8_192, None);
        assert_eq!(budget, first.context_window - 8_192 - SCHEMA_RESERVE);
    }

    #[test]
    fn unknown_models_fall_back_per_provider() {
        assert_eq!(context_window(&ProviderId::Claude, "claude-next"), 200_000);
        assert_eq!(context_window(&ProviderId::OpenAI, "gpt-next"), 128_000);
        assert_eq!(
            context_window(&ProviderId::Ollama, "qwen2.5-coder:7b"),
            8_192
        );
        assert_eq!(context_window(&ProviderId::Groq, "whatever"), 32_000);
    }

    #[test]
    fn small_windows_keep_a_usable_budget() {
        // 8192 window, 4096 requested output: reply capped to a quarter.
        let b = context_budget(&ProviderId::Ollama, "x", 4_096, None);
        assert_eq!(b, 8_192 - 2_048 - SCHEMA_RESERVE);
        assert!(b >= MIN_BUDGET);
        // An absurd reply reserve is capped, so the budget stays usable.
        assert_eq!(
            context_budget(&ProviderId::Ollama, "x", 1_000_000, None),
            8_192 - 2_048 - SCHEMA_RESERVE
        );
    }

    #[test]
    fn exact_windows_are_used_as_given() {
        assert_eq!(
            budget_for_window(16_384, 4_096, None),
            16_384 - 4_096 - SCHEMA_RESERVE
        );
    }

    #[test]
    fn override_wins_but_not_below_minimum() {
        assert_eq!(
            context_budget(&ProviderId::Claude, "x", 1, Some(50_000)),
            50_000
        );
        assert_eq!(
            context_budget(&ProviderId::Claude, "x", 1, Some(10)),
            MIN_BUDGET
        );
    }
}
