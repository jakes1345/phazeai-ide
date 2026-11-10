//! The models a provider offers right now, read from its own model list.
//!
//! Built-in model tables go stale: providers add models every few weeks and
//! retire old ones. Every provider Phaze talks to publishes the list of
//! models a key can use, at no cost:
//!
//! - Claude: `GET /v1/models`, newest first, paged with `after_id`, each
//!   with `max_input_tokens` (platform.claude.com/docs/en/api/models-list).
//! - Gemini: the native `GET /v1beta/models`, paged with `pageToken`, each
//!   with `inputTokenLimit` and the methods it supports.
//! - Everyone else (OpenAI and the OpenAI-compatible hosts, local servers
//!   included): `GET {base}/models`. OpenRouter adds `context_length` and a
//!   per-token `pricing`; some hosts add a context field of their own.
//!
//! A list is kept for a day in the user's cache folder, so opening the model
//! picker doesn't call the provider each time; local servers are asked
//! afresh every time, since the models loaded there change by the minute.
//! The cached context windows also size the conversation for a model the
//! built-in table doesn't know ([`cached_context_window`]).

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::PhazeError;
use crate::llm::openai::openai_compat_url;
use crate::llm::provider::{ProviderId, ProviderRegistry};

/// How long a fetched list is used before the provider is asked again.
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// The most pages read from one provider (a page holds up to 1000 models).
const MAX_PAGES: usize = 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// One model as the provider lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveModel {
    pub id: String,
    /// The provider's display name, when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Input tokens the model takes, when the list says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
    /// USD per million input and output tokens, when the list says
    /// (OpenRouter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_per_m: Option<(f64, f64)>,
}

/// A provider's list as fetched at one time from one address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelList {
    /// The address it was read from: a list from another address (a proxy,
    /// another Azure resource) is not this one.
    pub base_url: String,
    /// Seconds since the Unix epoch.
    pub fetched_at: u64,
    pub models: Vec<LiveModel>,
}

impl ModelList {
    /// Whether the list is younger than [`CACHE_TTL`].
    pub fn is_fresh(&self) -> bool {
        now_secs().saturating_sub(self.fetched_at) < CACHE_TTL.as_secs()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.models.iter().any(|m| m.id == id)
    }
}

/// A row of the model picker: the live list and the built-in table together.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedModel {
    pub id: String,
    pub name: String,
    pub context_window: Option<usize>,
    /// USD per million input and output tokens, when known.
    pub price_per_m: Option<(f64, f64)>,
    /// Whether the provider's list has it. A built-in model the list leaves
    /// out has been retired or isn't open to this key.
    pub offered: bool,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether a provider runs on the user's machine, where lists aren't cached.
fn is_local(id: &ProviderId) -> bool {
    matches!(
        id,
        ProviderId::Ollama | ProviderId::LmStudio | ProviderId::BuiltIn
    )
}

/// Model ids that are not chat models: embeddings, speech, images, video,
/// moderation and the like. A provider that says which methods a model
/// supports (Gemini, Cohere) is filtered by that instead.
fn is_chat_model(id: &str) -> bool {
    const NOT_CHAT: &[&str] = &[
        "embed",
        "tts",
        "whisper",
        "dall-e",
        "moderation",
        "transcribe",
        "image",
        "audio",
        "realtime",
        "aqa",
        "imagen",
        "veo",
        "rerank",
        "babbage",
        "davinci",
        "guard",
        "lyria",
    ];
    let id = id.to_ascii_lowercase();
    !NOT_CHAT.iter().any(|w| id.contains(w))
}

/// The request for one page: URL and headers.
fn page_request(
    id: &ProviderId,
    base_url: &str,
    key: Option<&str>,
    cursor: Option<&str>,
) -> (String, Vec<(&'static str, String)>) {
    let mut headers = Vec::new();
    let url = match id {
        ProviderId::Claude => {
            if let Some(key) = key {
                headers.push(("x-api-key", key.to_string()));
            }
            headers.push(("anthropic-version", "2023-06-01".to_string()));
            let mut url = format!("{}?limit=1000", openai_compat_url(base_url, "models"));
            if let Some(after) = cursor {
                url.push_str("&after_id=");
                url.push_str(&urlencode(after));
            }
            url
        }
        ProviderId::Gemini if is_google_endpoint(base_url) => {
            if let Some(key) = key {
                headers.push(("x-goog-api-key", key.to_string()));
            }
            let mut url = format!("{}/v1beta/models?pageSize=1000", google_origin(base_url));
            if let Some(token) = cursor {
                url.push_str("&pageToken=");
                url.push_str(&urlencode(token));
            }
            url
        }
        ProviderId::Azure => {
            if let Some(key) = key {
                headers.push(("api-key", key.to_string()));
            }
            openai_compat_url(base_url, "models")
        }
        _ => {
            if let Some(key) = key.filter(|k| !k.is_empty()) {
                headers.push(("authorization", format!("Bearer {key}")));
            }
            openai_compat_url(base_url, "models")
        }
    };
    (url, headers)
}

/// Google's own Gemini address (not a proxy in front of it).
fn is_google_endpoint(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h == "generativelanguage.googleapis.com")
        })
        .unwrap_or(false)
}

fn google_origin(base_url: &str) -> String {
    reqwest::Url::parse(base_url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|_| base_url.trim_end_matches('/').to_string())
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn as_usize(v: &Value) -> Option<usize> {
    v.as_u64()
        .map(|n| n as usize)
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|&n| n > 0)
}

/// A per-token price, as OpenRouter gives it ("0.000003"), per million.
fn per_million(v: &Value) -> Option<f64> {
    let per_token = v
        .as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))?;
    (per_token >= 0.0).then_some(per_token * 1_000_000.0)
}

/// One entry of a list, or `None` for one that isn't a chat model.
fn parse_entry(entry: &Value) -> Option<LiveModel> {
    let raw_id = entry
        .get("id")
        .or_else(|| entry.get("name"))
        .and_then(Value::as_str)?
        .trim();
    let id = raw_id.strip_prefix("models/").unwrap_or(raw_id).to_string();
    if id.is_empty() {
        return None;
    }
    if entry
        .get("lifecycle")
        .and_then(Value::as_str)
        .is_some_and(|l| l == "retired")
    {
        return None;
    }
    // Lists that say what a model is for.
    if let Some(methods) = entry
        .get("supportedGenerationMethods")
        .and_then(Value::as_array)
    {
        if !methods
            .iter()
            .any(|m| m.as_str() == Some("generateContent"))
        {
            return None;
        }
    } else if let Some(endpoints) = entry.get("endpoints").and_then(Value::as_array) {
        if !endpoints.iter().any(|e| e.as_str() == Some("chat")) {
            return None;
        }
    } else if !is_chat_model(&id) {
        return None;
    }
    let name = ["display_name", "displayName", "name"]
        .iter()
        .filter_map(|k| entry.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .find(|n| !n.is_empty() && *n != raw_id && *n != id)
        .map(str::to_string);
    let context_window = [
        "max_input_tokens",
        "inputTokenLimit",
        "context_length",
        "context_window",
        "max_context_length",
        "max_model_len",
    ]
    .iter()
    .find_map(|k| entry.get(*k).and_then(as_usize))
    .or_else(|| {
        entry
            .pointer("/top_provider/context_length")
            .and_then(as_usize)
    });
    let price_per_m = entry.get("pricing").and_then(|p| {
        Some((
            per_million(p.get("prompt")?)?,
            per_million(p.get("completion")?)?,
        ))
    });
    Some(LiveModel {
        id,
        name,
        context_window,
        price_per_m,
    })
}

/// The models on one page of a list and the cursor for the next page.
pub fn parse_page(id: &ProviderId, body: &Value) -> (Vec<LiveModel>, Option<String>) {
    let entries = body
        .get("data")
        .or_else(|| body.get("models"))
        .unwrap_or(body)
        .as_array()
        .cloned()
        .unwrap_or_default();
    let models = entries.iter().filter_map(parse_entry).collect();
    let next = match id {
        ProviderId::Claude => body
            .get("has_more")
            .and_then(Value::as_bool)
            .filter(|&more| more)
            .and_then(|_| body.get("last_id").and_then(Value::as_str))
            .map(str::to_string),
        _ => body
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_string),
    };
    (models, next)
}

/// Reads the provider's whole list from `base_url`.
pub async fn fetch_models(
    id: &ProviderId,
    base_url: &str,
    key: Option<&str>,
) -> Result<Vec<LiveModel>, PhazeError> {
    if base_url.trim().is_empty() {
        return Err(PhazeError::Config(format!(
            "{} has no address set, so its model list can't be read.",
            id.name()
        )));
    }
    if id.needs_api_key() && key.is_none_or(|k| k.trim().is_empty()) {
        return Err(PhazeError::Config(format!(
            "{} needs an API key to list its models.",
            id.name()
        )));
    }
    let mut builder = reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
    if is_local(id) {
        builder = builder.no_proxy();
    }
    let client = builder
        .build()
        .map_err(|e| PhazeError::Llm(format!("Couldn't start an HTTP client: {e}")))?;
    let mut models: Vec<LiveModel> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let (url, headers) = page_request(id, base_url, key, cursor.as_deref());
        let mut req = client.get(&url);
        for (name, value) in headers {
            req = req.header(name, value);
        }
        let resp = req.send().await.map_err(|e| {
            PhazeError::Llm(format!("Couldn't reach {} for its models: {e}", id.name()))
        })?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let detail = crate::text::truncate_bytes(body.trim(), 300);
            return Err(PhazeError::Llm(match status.as_u16() {
                401 | 403 => format!(
                    "{} turned the API key down when asked for its models ({status}).",
                    id.name()
                ),
                404 => format!("{} has no model list at {url} (404).", id.name()),
                _ => format!("{} answered {status} for its models: {detail}", id.name()),
            }));
        }
        let body: Value = resp.json().await.map_err(|e| {
            PhazeError::Llm(format!(
                "{}'s model list wasn't readable JSON: {e}",
                id.name()
            ))
        })?;
        let (page, next) = parse_page(id, &body);
        for model in page {
            if !models.iter().any(|m| m.id == model.id) {
                models.push(model);
            }
        }
        match next {
            Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
            _ => break,
        }
    }
    // Claude and Gemini list newest first; the others in no useful order.
    if !matches!(id, ProviderId::Claude | ProviderId::Gemini) {
        models.sort_by(|a, b| a.id.cmp(&b.id));
    }
    Ok(models)
}

/// The file a provider's list is cached in.
fn cache_file(id: &ProviderId) -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join(format!("{}.json", file_stem(id))))
}

fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join(crate::constants::paths::CONFIG_DIR).join("models"))
}

fn file_stem(id: &ProviderId) -> String {
    let name = match id {
        ProviderId::Custom(name) => format!("custom-{name}"),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| other.name().to_string()),
    };
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// The provider's cached list, whatever its age, from any address.
pub fn cached(id: &ProviderId) -> Option<ModelList> {
    let text = std::fs::read_to_string(cache_file(id)?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save(id: &ProviderId, list: &ModelList) {
    let Some(path) = cache_file(id) else { return };
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    if let Ok(text) = serde_json::to_string(list) {
        // Write beside it and rename, so a reader never sees half a file.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// The provider's models: from the cache when it holds a fresh list from
/// this address and `refresh` is false, otherwise from the provider (and
/// then cached). Local servers are always asked.
pub async fn live_models(
    id: &ProviderId,
    base_url: &str,
    key: Option<&str>,
    refresh: bool,
) -> Result<ModelList, PhazeError> {
    if !refresh && !is_local(id) {
        if let Some(list) = cached(id).filter(|l| l.base_url == base_url && l.is_fresh()) {
            return Ok(list);
        }
    }
    let models = fetch_models(id, base_url, key).await?;
    let list = ModelList {
        base_url: base_url.to_string(),
        fetched_at: now_secs(),
        models,
    };
    if !is_local(id) {
        save(id, &list);
    }
    Ok(list)
}

/// The context window the cached list gives for `model`, if any.
pub fn cached_context_window(id: &ProviderId, model: &str) -> Option<usize> {
    if is_local(id) {
        return None;
    }
    cached(id)?
        .models
        .into_iter()
        .find(|m| m.id == model)
        .and_then(|m| m.context_window)
}

/// The picker's rows: the provider's list in its order, filled in from the
/// built-in table and the price list where those know a model, then the
/// built-in models the list leaves out (marked not offered). With no list,
/// the built-in table alone, all marked offered.
pub fn merge(id: &ProviderId, live: Option<&ModelList>) -> Vec<ListedModel> {
    let known = ProviderRegistry::known_models(id);
    let price = |model: &str, listed: Option<(f64, f64)>| {
        crate::cost::price(id, model, 0)
            .map(|p| (p.input, p.output))
            .or(listed)
            .or_else(|| {
                known
                    .iter()
                    .find(|k| k.id == model && k.input_cost_per_m > 0.0)
                    .map(|k| (k.input_cost_per_m, k.output_cost_per_m))
            })
    };
    let Some(live) = live else {
        return known
            .iter()
            .map(|k| ListedModel {
                id: k.id.clone(),
                name: k.name.clone(),
                context_window: Some(k.context_window),
                price_per_m: price(&k.id, None),
                offered: true,
            })
            .collect();
    };
    let mut rows: Vec<ListedModel> = live
        .models
        .iter()
        .map(|m| {
            let built_in = known.iter().find(|k| k.id == m.id);
            ListedModel {
                id: m.id.clone(),
                name: m
                    .name
                    .clone()
                    .or_else(|| built_in.map(|k| k.name.clone()))
                    .unwrap_or_else(|| m.id.clone()),
                context_window: m.context_window.or(built_in.map(|k| k.context_window)),
                price_per_m: price(&m.id, m.price_per_m),
                offered: true,
            }
        })
        .collect();
    for k in &known {
        if !live.contains(&k.id) {
            rows.push(ListedModel {
                id: k.id.clone(),
                name: k.name.clone(),
                context_window: Some(k.context_window),
                price_per_m: price(&k.id, None),
                offered: false,
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_pages_carry_the_window_and_the_next_cursor() {
        let body = json!({
            "data": [
                {"type": "model", "id": "claude-opus-5-5", "display_name": "Claude Opus 5.5",
                 "created_at": "2026-10-01T00:00:00Z", "max_input_tokens": 1000000,
                 "max_tokens": 128000},
                {"type": "model", "id": "claude-old", "display_name": "Old",
                 "lifecycle": "retired"},
            ],
            "has_more": true,
            "first_id": "claude-opus-5-5",
            "last_id": "claude-old",
        });
        let (models, next) = parse_page(&ProviderId::Claude, &body);
        assert_eq!(
            models,
            [LiveModel {
                id: "claude-opus-5-5".into(),
                name: Some("Claude Opus 5.5".into()),
                context_window: Some(1_000_000),
                price_per_m: None,
            }]
        );
        assert_eq!(next.as_deref(), Some("claude-old"));
        let last = json!({"data": [], "has_more": false, "last_id": null});
        assert_eq!(parse_page(&ProviderId::Claude, &last).1, None);
    }

    #[test]
    fn gemini_keeps_models_that_generate_text() {
        let body = json!({
            "models": [
                {"name": "models/gemini-3.8-flash", "displayName": "Gemini 3.8 Flash",
                 "inputTokenLimit": 1048576,
                 "supportedGenerationMethods": ["generateContent", "countTokens"]},
                {"name": "models/text-embedding-005", "displayName": "Embedding",
                 "supportedGenerationMethods": ["embedContent"]},
            ],
            "nextPageToken": "abc",
        });
        let (models, next) = parse_page(&ProviderId::Gemini, &body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini-3.8-flash");
        assert_eq!(models[0].context_window, Some(1_048_576));
        assert_eq!(next.as_deref(), Some("abc"));
    }

    #[test]
    fn openrouter_prices_and_windows_and_openai_filtering() {
        let body = json!({"data": [
            {"id": "deepseek/deepseek-v4", "name": "DeepSeek: V4",
             "context_length": 163840,
             "pricing": {"prompt": "0.0000003", "completion": "0.0000012"}},
            {"id": "openai/text-embedding-3-small", "name": "Embeddings"},
        ]});
        let (models, _) = parse_page(&ProviderId::OpenRouter, &body);
        assert_eq!(models.len(), 1);
        let (input, output) = models[0].price_per_m.unwrap();
        assert!((input - 0.3).abs() < 1e-9 && (output - 1.2).abs() < 1e-9);
        assert_eq!(models[0].context_window, Some(163_840));

        // A bare array (Together) and OpenAI's speech and image models.
        let body = json!([
            {"id": "gpt-6-luna", "object": "model"},
            {"id": "gpt-6-luna-tts", "object": "model"},
            {"id": "whisper-1", "object": "model"},
            {"id": "gpt-image-2", "object": "model"},
        ]);
        let (models, _) = parse_page(&ProviderId::OpenAI, &body);
        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-6-luna"]);
    }

    #[test]
    fn requests_use_each_providers_own_paging_and_key_header() {
        let (url, headers) = page_request(
            &ProviderId::Claude,
            "https://api.anthropic.com",
            Some("k"),
            Some("claude x"),
        );
        assert_eq!(
            url,
            "https://api.anthropic.com/v1/models?limit=1000&after_id=claude%20x"
        );
        assert!(headers.contains(&("x-api-key", "k".to_string())));
        let (url, headers) = page_request(
            &ProviderId::Gemini,
            "https://generativelanguage.googleapis.com/v1beta/openai/",
            Some("g"),
            None,
        );
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000"
        );
        assert_eq!(headers, [("x-goog-api-key", "g".to_string())]);
        let (url, headers) =
            page_request(&ProviderId::Ollama, "http://localhost:11434", None, None);
        assert_eq!(url, "http://localhost:11434/v1/models");
        assert!(headers.is_empty());
    }

    #[test]
    fn merging_marks_built_in_models_the_provider_no_longer_lists() {
        let list = ModelList {
            base_url: "https://api.anthropic.com".into(),
            fetched_at: now_secs(),
            models: vec![
                LiveModel {
                    id: "claude-new".into(),
                    name: Some("Claude New".into()),
                    context_window: Some(500_000),
                    price_per_m: None,
                },
                LiveModel {
                    id: "claude-haiku-4-5-20251001".into(),
                    name: None,
                    context_window: None,
                    price_per_m: None,
                },
            ],
        };
        let rows = merge(&ProviderId::Claude, Some(&list));
        assert_eq!(rows[0].id, "claude-new");
        assert_eq!(rows[0].context_window, Some(500_000));
        assert!(rows[0].offered);
        // A listed model the table knows gets its name, window and price.
        assert_eq!(rows[1].name, "Claude Haiku 4.5");
        assert_eq!(rows[1].context_window, Some(200_000));
        assert_eq!(rows[1].price_per_m, Some((1.0, 5.0)));
        // The table's other models come last, not offered.
        assert!(rows[2..].iter().all(|r| !r.offered));
        assert!(rows.iter().any(|r| r.id == "claude-opus-4-6" && !r.offered));
        // No list: the table alone.
        assert!(merge(&ProviderId::Claude, None).iter().all(|r| r.offered));
    }

    #[test]
    fn cache_files_are_named_safely() {
        assert_eq!(file_stem(&ProviderId::OpenRouter), "openrouter");
        assert_eq!(file_stem(&ProviderId::BuiltIn), "builtin");
        assert_eq!(
            file_stem(&ProviderId::Custom("My Proxy/2".into())),
            "custom-my_proxy_2"
        );
    }
}
