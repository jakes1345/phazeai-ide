//! Embedding endpoints: turn chunks of code (and queries) into vectors.
//!
//! Two wire formats cover the local servers people run:
//!
//! * **Ollama** `POST {url}/api/embed` with `{"model", "input": [...]}`,
//!   answering `{"embeddings": [[...], ...]}`.
//! * **OpenAI-compatible** `POST {url}/v1/embeddings` with the same body,
//!   answering `{"data": [{"index", "embedding": [...]}, ...]}`. llama.cpp's
//!   `llama-server --embedding`, LM Studio, vLLM, LocalAI and the hosted
//!   OpenAI-style APIs all speak it.
//!
//! The format is picked from the address: one that ends in `/v1` (or has
//! `/v1/` in it), or uses llama-server's or LM Studio's default port, is
//! OpenAI-compatible; anything else is Ollama. `embedding_api` in `[search]`
//! overrides the guess.
//!
//! Code goes to the endpoint, so it goes through the same gate as a chat
//! request: a project that is `local_only` refuses an endpoint that is not on
//! this machine, and every request is written to the privacy audit log.

use crate::privacy;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

/// Texts per request. Small enough for a CPU-only local server to answer well
/// inside the timeout, large enough to keep the request count down.
pub(crate) const BATCH: usize = 32;
/// Characters of one text sent to the model. Chunks are at most 4 KiB of
/// code (see `chunk::HARD_BYTES`) plus a path line; a longer text is cut so a
/// model with a short context cannot refuse the whole batch.
pub(crate) const MAX_TEXT_CHARS: usize = 6000;
const TIMEOUT: Duration = Duration::from_secs(120);

/// Which request format the endpoint speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingApi {
    Ollama,
    OpenAi,
}

impl EmbeddingApi {
    /// Parse the `embedding_api` setting; anything unknown means "guess".
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ollama" => Some(Self::Ollama),
            "openai" | "openai-compatible" | "openai_compatible" | "llama" | "llama.cpp"
            | "llamacpp" | "lmstudio" | "lm-studio" => Some(Self::OpenAi),
            _ => None,
        }
    }

    /// Guess the format from the address.
    pub fn guess(url: &str) -> Self {
        let u = url.trim().trim_end_matches('/').to_ascii_lowercase();
        if u.ends_with("/v1") || u.contains("/v1/") {
            return Self::OpenAi;
        }
        // llama-server listens on 8080 and LM Studio on 1234 by default.
        let port = u
            .rsplit_once("://")
            .map_or(u.as_str(), |(_, rest)| rest)
            .split('/')
            .next()
            .and_then(|host| host.rsplit_once(':'))
            .map(|(_, p)| p.to_string());
        match port.as_deref() {
            Some("8080") | Some("1234") => Self::OpenAi,
            _ => Self::Ollama,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenAi => "openai",
        }
    }
}

/// Where and how to ask for embeddings.
#[derive(Clone, PartialEq, Eq)]
pub struct EmbeddingConfig {
    pub model: String,
    /// Base address, e.g. `http://localhost:11434` or `http://localhost:8080/v1`.
    pub url: String,
    pub api: EmbeddingApi,
    pub api_key: Option<String>,
}

impl std::fmt::Debug for EmbeddingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingConfig")
            .field("model", &self.model)
            .field("url", &self.url)
            .field("api", &self.api)
            .field("api_key", &self.api_key.as_ref().map(|_| "<set>"))
            .finish()
    }
}

/// Ollama's default address, used when `embedding_url` is not set.
pub const DEFAULT_URL: &str = "http://localhost:11434";

impl EmbeddingConfig {
    /// A config for `model` at `url` (Ollama's default when empty), with the
    /// format from `api` or guessed from the address.
    pub fn new(model: &str, url: Option<&str>, api: Option<&str>) -> Option<Self> {
        let model = model.trim();
        if model.is_empty() {
            return None;
        }
        let url = url
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .unwrap_or(DEFAULT_URL)
            .trim_end_matches('/')
            .to_string();
        let api = api
            .and_then(EmbeddingApi::parse)
            .unwrap_or_else(|| EmbeddingApi::guess(&url));
        Some(Self {
            model: model.to_string(),
            url,
            api,
            api_key: None,
        })
    }

    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|k| !k.trim().is_empty());
        self
    }

    /// The address requests go to.
    pub fn endpoint(&self) -> String {
        match self.api {
            EmbeddingApi::Ollama => format!("{}/api/embed", self.url),
            EmbeddingApi::OpenAi => {
                if self.url.to_ascii_lowercase().ends_with("/v1") {
                    format!("{}/embeddings", self.url)
                } else {
                    format!("{}/v1/embeddings", self.url)
                }
            }
        }
    }

    /// The endpoint is on this machine.
    pub fn is_local(&self) -> bool {
        privacy::url_host(&self.url).is_some_and(|h| privacy::is_loopback_host(&h))
    }

    /// Names the vector space: vectors from two configs with the same key can
    /// be compared. The API key is not part of it.
    pub fn space_key(&self) -> String {
        format!("{}|{}|{}", self.api.name(), self.url, self.model)
    }

    /// Requests are refused: the active project is `local_only` and the
    /// endpoint is elsewhere.
    pub fn refused(&self) -> Option<String> {
        if !self.is_local() && privacy::active().local_only() {
            Some(format!(
                "this project is local_only, so code is not sent to the embedding \
                 endpoint at {}; use a model on this machine",
                self.url
            ))
        } else {
            None
        }
    }
}

fn http_client(direct: bool) -> &'static reqwest::Client {
    static VIA_PROXY: OnceLock<reqwest::Client> = OnceLock::new();
    static DIRECT: OnceLock<reqwest::Client> = OnceLock::new();
    let (cell, builder) = if direct {
        (&DIRECT, reqwest::Client::builder().no_proxy())
    } else {
        (&VIA_PROXY, reqwest::Client::builder())
    };
    cell.get_or_init(|| {
        builder
            .timeout(TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Cut a text to [`MAX_TEXT_CHARS`] on a character boundary.
pub(crate) fn clip(text: &str) -> &str {
    match text.char_indices().nth(MAX_TEXT_CHARS) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// Ask for the vectors of `texts` (at most [`BATCH`] per request; longer
/// lists are split). `files` names the files the texts come from, for the
/// audit log. The vectors come back in the order of `texts`.
pub(crate) async fn embed(
    cfg: &EmbeddingConfig,
    texts: &[String],
    files: &[String],
) -> Result<Vec<Vec<f32>>, String> {
    if let Some(reason) = cfg.refused() {
        privacy::audit::record_blocked_provider("embeddings", &cfg.model, &reason);
        return Err(reason);
    }
    let mut out = Vec::with_capacity(texts.len());
    for batch in texts.chunks(BATCH) {
        out.extend(embed_batch(cfg, batch, files).await?);
    }
    Ok(out)
}

async fn embed_batch(
    cfg: &EmbeddingConfig,
    texts: &[String],
    files: &[String],
) -> Result<Vec<Vec<f32>>, String> {
    let input: Vec<&str> = texts.iter().map(|t| clip(t)).collect();
    privacy::audit::record_request(
        "embeddings",
        &cfg.model,
        input.iter().map(|t| t.chars().count()).sum(),
        files.to_vec(),
    );
    let body = json!({ "model": cfg.model, "input": input });
    let mut request = http_client(cfg.is_local()).post(cfg.endpoint()).json(&body);
    if let Some(key) = &cfg.api_key {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("embedding request to {} failed: {e}", cfg.url))?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        let detail: String = text.chars().take(300).collect();
        return Err(
            if status == 404 && detail.to_ascii_lowercase().contains("not found") {
                format!(
                    "the embedding model {} was not found at {} ({status}): {detail}",
                    cfg.model, cfg.url
                )
            } else {
                format!("embedding request failed ({status}): {detail}")
            },
        );
    }
    let answer: Value = serde_json::from_str(&text)
        .map_err(|e| format!("the embedding endpoint sent something that is not JSON: {e}"))?;
    let vectors = parse_answer(cfg.api, &answer)?;
    if vectors.len() != texts.len() {
        return Err(format!(
            "asked for {} embeddings, the endpoint sent {}",
            texts.len(),
            vectors.len()
        ));
    }
    let dim = vectors.first().map_or(0, Vec::len);
    if dim == 0 || vectors.iter().any(|v| v.len() != dim) {
        return Err("the embedding endpoint sent empty or uneven vectors".into());
    }
    Ok(vectors)
}

fn floats(v: &Value) -> Option<Vec<f32>> {
    v.as_array()?
        .iter()
        .map(|x| x.as_f64().map(|f| f as f32))
        .collect()
}

/// The vectors in an answer, in input order.
pub(crate) fn parse_answer(api: EmbeddingApi, answer: &Value) -> Result<Vec<Vec<f32>>, String> {
    if let Some(err) = answer.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| err.as_str())
            .unwrap_or("unknown error");
        return Err(format!(
            "the embedding endpoint answered with an error: {msg}"
        ));
    }
    let bad = || "the embedding endpoint's answer has no vectors in the expected place".to_string();
    // Accept either shape whichever format was asked for: some servers that
    // speak one also answer in the other.
    let ollama = |a: &Value| -> Option<Vec<Vec<f32>>> {
        a.get("embeddings")?
            .as_array()?
            .iter()
            .map(floats)
            .collect()
    };
    let openai = |a: &Value| -> Option<Vec<Vec<f32>>> {
        let mut rows: Vec<(usize, Vec<f32>)> = a
            .get("data")?
            .as_array()?
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let idx = item
                    .get("index")
                    .and_then(Value::as_u64)
                    .map_or(i, |n| n as usize);
                floats(item.get("embedding")?).map(|v| (idx, v))
            })
            .collect::<Option<_>>()?;
        rows.sort_by_key(|(i, _)| *i);
        Some(rows.into_iter().map(|(_, v)| v).collect())
    };
    let parsed = match api {
        EmbeddingApi::Ollama => ollama(answer).or_else(|| openai(answer)),
        EmbeddingApi::OpenAi => openai(answer).or_else(|| ollama(answer)),
    };
    parsed.ok_or_else(bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_format_follows_the_address_unless_set() {
        assert_eq!(
            EmbeddingApi::guess("http://localhost:11434"),
            EmbeddingApi::Ollama
        );
        assert_eq!(
            EmbeddingApi::guess("http://localhost:8080"),
            EmbeddingApi::OpenAi
        );
        assert_eq!(
            EmbeddingApi::guess("http://127.0.0.1:1234/"),
            EmbeddingApi::OpenAi
        );
        assert_eq!(
            EmbeddingApi::guess("https://api.openai.com/v1"),
            EmbeddingApi::OpenAi
        );
        assert_eq!(
            EmbeddingApi::guess("http://box:9000/v1/"),
            EmbeddingApi::OpenAi
        );
        let cfg = EmbeddingConfig::new("m", Some("http://localhost:8080"), Some("ollama")).unwrap();
        assert_eq!(cfg.api, EmbeddingApi::Ollama);
        assert_eq!(cfg.endpoint(), "http://localhost:8080/api/embed");
    }

    #[test]
    fn endpoints_and_defaults() {
        let o = EmbeddingConfig::new("nomic-embed-text", None, None).unwrap();
        assert_eq!(o.endpoint(), "http://localhost:11434/api/embed");
        assert!(o.is_local());
        let v1 = EmbeddingConfig::new("e", Some("http://localhost:8080/v1/"), None).unwrap();
        assert_eq!(v1.endpoint(), "http://localhost:8080/v1/embeddings");
        let bare = EmbeddingConfig::new("e", Some("http://127.0.0.1:8080"), None).unwrap();
        assert_eq!(bare.endpoint(), "http://127.0.0.1:8080/v1/embeddings");
        let remote = EmbeddingConfig::new("e", Some("https://api.example.com/v1"), None).unwrap();
        assert!(!remote.is_local());
        assert!(EmbeddingConfig::new("  ", None, None).is_none());
        // The key is not part of the vector space and is never printed.
        let keyed = remote.clone().with_api_key(Some("sk-secret".into()));
        assert_eq!(keyed.space_key(), remote.space_key());
        assert!(!format!("{keyed:?}").contains("sk-secret"));
    }

    #[test]
    fn both_answer_shapes_parse_in_input_order() {
        let ollama = json!({"embeddings": [[1.0, 0.0], [0.0, 1.0]]});
        assert_eq!(
            parse_answer(EmbeddingApi::Ollama, &ollama).unwrap(),
            vec![vec![1.0, 0.0], vec![0.0, 1.0]]
        );
        // Out of order on the wire; `index` decides.
        let openai = json!({"data": [
            {"index": 1, "embedding": [0.0, 1.0]},
            {"index": 0, "embedding": [1.0, 0.0]}
        ]});
        assert_eq!(
            parse_answer(EmbeddingApi::OpenAi, &openai).unwrap(),
            vec![vec![1.0, 0.0], vec![0.0, 1.0]]
        );
        // A server that speaks the other shape still works.
        assert!(parse_answer(EmbeddingApi::OpenAi, &ollama).is_ok());
        let err = json!({"error": {"message": "model not loaded"}});
        assert!(parse_answer(EmbeddingApi::Ollama, &err)
            .unwrap_err()
            .contains("model not loaded"));
        assert!(parse_answer(EmbeddingApi::Ollama, &json!({"x": 1})).is_err());
    }

    #[test]
    fn long_texts_are_clipped_on_a_char_boundary() {
        let s = "é".repeat(MAX_TEXT_CHARS + 10);
        assert_eq!(clip(&s).chars().count(), MAX_TEXT_CHARS);
        assert_eq!(clip("short"), "short");
    }
}
