//! First-run help for API keys: where to get one, and a cheap check that a pasted
//! key actually works. Used by the settings UI; no state is kept here.

use std::time::Duration;

use super::openai::openai_compat_url;
use super::provider::ProviderId;

/// Where to get a key for a provider, and what to expect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyHelp {
    pub signup_url: &'static str,
    /// Has a no-card free tier (limits and terms change; the UI should say so).
    pub free_tier: bool,
    pub note: &'static str,
}

const FREE_NOTE: &str =
    "Free tier: usage limits apply and prompts may be used for training. Check the provider's terms.";

/// Sign-up info for the providers worth pointing a new user at.
pub fn key_help(id: &ProviderId) -> Option<KeyHelp> {
    let (signup_url, free_tier, note) = match id {
        ProviderId::Gemini => ("https://aistudio.google.com/apikey", true, FREE_NOTE),
        ProviderId::Groq => ("https://console.groq.com/keys", true, FREE_NOTE),
        ProviderId::OpenRouter => (
            "https://openrouter.ai/keys",
            true,
            "Only models tagged \":free\" cost nothing, with low daily limits. Prompts may be used for training.",
        ),
        ProviderId::Mistral => ("https://console.mistral.ai/api-keys", true, FREE_NOTE),
        ProviderId::Claude => (
            "https://console.anthropic.com/settings/keys",
            false,
            "Paid per use. New accounts may get a small trial credit.",
        ),
        ProviderId::OpenAI => (
            "https://platform.openai.com/api-keys",
            false,
            "Paid per use. A ChatGPT subscription does not include API access.",
        ),
        _ => return None,
    };
    Some(KeyHelp {
        signup_url,
        free_tier,
        note,
    })
}

/// Result of probing a provider with a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCheck {
    Valid,
    /// 401/403: the provider answered and refused the key.
    Rejected,
    /// Could not reach the provider (offline, DNS, timeout).
    Unreachable(String),
    /// Any other HTTP status; the key may or may not be fine.
    Unexpected(u16),
}

impl KeyCheck {
    pub fn message(&self) -> String {
        match self {
            Self::Valid => "Key works.".into(),
            Self::Rejected => "The provider rejected this key.".into(),
            Self::Unreachable(e) => format!("Could not reach the provider: {e}"),
            Self::Unexpected(code) => {
                format!("Unexpected response (HTTP {code}); the key may still work.")
            }
        }
    }
}

/// Map an HTTP status from the models endpoint to a verdict.
pub fn interpret_status(status: u16) -> KeyCheck {
    match status {
        200..=299 => KeyCheck::Valid,
        401 | 403 => KeyCheck::Rejected,
        other => KeyCheck::Unexpected(other),
    }
}

/// The request used to test a key: a GET of the provider's model list, which costs
/// nothing. Returns `(url, headers)`, or `None` for providers we cannot probe this way.
pub fn probe_request(
    id: &ProviderId,
    base_url: &str,
    key: &str,
) -> Option<(String, Vec<(&'static str, String)>)> {
    if base_url.is_empty() || !id.needs_api_key() {
        return None;
    }
    match id {
        ProviderId::Claude => Some((
            openai_compat_url(base_url, "models"),
            vec![
                ("x-api-key", key.to_string()),
                ("anthropic-version", "2023-06-01".to_string()),
            ],
        )),
        _ => Some((
            openai_compat_url(base_url, "models"),
            vec![("authorization", format!("Bearer {key}"))],
        )),
    }
}

/// Check a key against the provider (blocking-free: call from an async context or a
/// dedicated runtime thread). `base_url` should be the configured one.
pub async fn check_api_key(id: &ProviderId, base_url: &str, key: &str) -> KeyCheck {
    let Some((url, headers)) = probe_request(id, base_url, key) else {
        return KeyCheck::Unexpected(0);
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => return KeyCheck::Unreachable(e.to_string()),
    };
    let mut req = client.get(url);
    for (name, value) in headers {
        req = req.header(name, value);
    }
    match req.send().await {
        Ok(resp) => interpret_status(resp.status().as_u16()),
        Err(e) => KeyCheck::Unreachable(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_providers_have_signup_links_and_paid_ones_say_so() {
        for id in [
            ProviderId::Gemini,
            ProviderId::Groq,
            ProviderId::OpenRouter,
            ProviderId::Mistral,
        ] {
            let h = key_help(&id).unwrap();
            assert!(h.free_tier && h.signup_url.starts_with("https://"));
            assert!(
                h.note.to_lowercase().contains("train"),
                "free tiers must warn about training"
            );
        }
        for id in [ProviderId::Claude, ProviderId::OpenAI] {
            assert!(!key_help(&id).unwrap().free_tier);
        }
        assert!(key_help(&ProviderId::Ollama).is_none());
    }

    #[test]
    fn status_codes_map_to_verdicts() {
        assert_eq!(interpret_status(200), KeyCheck::Valid);
        assert_eq!(interpret_status(401), KeyCheck::Rejected);
        assert_eq!(interpret_status(403), KeyCheck::Rejected);
        assert_eq!(interpret_status(429), KeyCheck::Unexpected(429));
        assert_eq!(interpret_status(500), KeyCheck::Unexpected(500));
    }

    #[test]
    fn probes_use_each_providers_auth_scheme_and_versioned_url() {
        let (url, h) =
            probe_request(&ProviderId::Claude, "https://api.anthropic.com", "k").unwrap();
        assert_eq!(url, "https://api.anthropic.com/v1/models");
        assert!(h.contains(&("x-api-key", "k".to_string())));
        assert!(h.iter().any(|(n, _)| *n == "anthropic-version"));

        let (url, h) =
            probe_request(&ProviderId::Mistral, "https://api.mistral.ai/v1", "k").unwrap();
        assert_eq!(url, "https://api.mistral.ai/v1/models");
        assert_eq!(h, vec![("authorization", "Bearer k".to_string())]);

        let (url, _) = probe_request(
            &ProviderId::Gemini,
            "https://generativelanguage.googleapis.com/v1beta/openai/",
            "k",
        )
        .unwrap();
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/openai/models"
        );
    }

    #[test]
    fn local_and_unconfigured_providers_are_not_probed() {
        assert!(probe_request(&ProviderId::Ollama, "http://localhost:11434", "").is_none());
        assert!(probe_request(&ProviderId::Azure, "", "k").is_none());
    }
}
