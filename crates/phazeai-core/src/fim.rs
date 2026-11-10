//! Fill-in-the-middle (FIM) inline code completion.
//!
//! Where the active provider has a fill-in-the-middle API, the code before
//! and after the cursor goes to it as they are, and the model writes what
//! belongs between them ([`FimEndpoint`]): PhazeAI Local (llama.cpp's
//! `/infill`), Ollama (`/api/generate` with a `suffix`), DeepSeek
//! (`/beta/completions`) and Mistral (`/v1/fim/completions`, Codestral).
//! Code models are trained on exactly this, so the request carries no
//! instructions and the answer needs no clean-up.
//!
//! Every other provider, and a model that turns such requests down, gets a
//! tight chat prompt that emulates it: the model sees the code with a
//! `<CURSOR>` marker and must output only the text to insert there.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Settings;
use crate::llm::{LlmClient, Message, ProviderId};
use crate::privacy::PrivacyPolicy;

/// Lines of code before the cursor that a request carries.
const PREFIX_LINES: usize = 40;
/// Lines of code after the cursor that a request carries.
const SUFFIX_LINES: usize = 20;
/// The most lines a suggestion shows, so it stays readable.
const MAX_LINES: usize = 6;
/// Tokens a fill-in-the-middle request may generate: enough for
/// [`MAX_LINES`] of code, and no more to pay for.
const MAX_TOKENS: u32 = 96;
/// A suggestion ends at the first blank line: one block at a time.
const STOP: &[&str] = &["\n\n"];
/// A completion that takes longer than this is no use to someone typing.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Whether a background completion may be requested for `path`: AI
/// completions are on (`[editor] ai_completions`) and `policy` does not deny
/// the file. Every completion request checks this first; when it is false
/// nothing is sent.
pub fn should_request_fim(ai_completions: bool, policy: &PrivacyPolicy, path: &Path) -> bool {
    ai_completions && !policy.is_denied(path)
}

/// Request a FIM completion. Returns the suggested insertion text (may be
/// empty if the model returns nothing useful) or an error string.
///
/// `prefix` — everything in the file before the cursor.
/// `suffix` — everything in the file after the cursor.
/// `language` — language hint for the chat prompt ("rust", "python", …).
///
/// This is intentionally synchronous-from-a-thread: callers spawn a thread,
/// call this, then write the result back to a signal via `create_ext_action`.
///
/// `path` is the file being edited: completions are skipped entirely (empty
/// result, nothing sent) when AI completions are off in `config.toml` or the
/// privacy policy denies the file (see [`should_request_fim`]); otherwise it
/// is recorded in the privacy audit log as the file whose content was sent.
pub async fn fim_complete(
    path: &Path,
    prefix: &str,
    suffix: &str,
    language: &str,
) -> Result<String, String> {
    fim_complete_for(&Settings::load(), path, prefix, suffix, language).await
}

/// [`fim_complete`] with `settings` in place of `config.toml`.
pub async fn fim_complete_for(
    settings: &Settings,
    path: &Path,
    prefix: &str,
    suffix: &str,
    language: &str,
) -> Result<String, String> {
    let policy = crate::privacy::active();
    let endpoint = || FimEndpoint::for_settings(settings).map_err(|e| e.to_string());
    let client = || settings.build_small_llm_client().map_err(|e| e.to_string());
    fim_complete_with(
        settings.editor.ai_completions,
        &policy,
        endpoint,
        client,
        path,
        prefix,
        suffix,
        language,
    )
    .await
}

/// [`fim_complete`] with the setting, the policy and the provider passed in.
/// `endpoint` and `client` are only called, and the request only sent, when
/// [`should_request_fim`] allows it.
#[allow(clippy::too_many_arguments)]
async fn fim_complete_with(
    ai_completions: bool,
    policy: &PrivacyPolicy,
    endpoint: impl FnOnce() -> Result<Option<FimEndpoint>, String>,
    client: impl FnOnce() -> Result<Box<dyn LlmClient>, String>,
    path: &Path,
    prefix: &str,
    suffix: &str,
    language: &str,
) -> Result<String, String> {
    // Silent skip: this runs on every typing pause, so it isn't logged as a block.
    if !should_request_fim(ai_completions, policy, path) {
        return Ok(String::new());
    }
    crate::privacy::audit::scope(
        vec![path.to_path_buf()],
        fim_request(endpoint, client, prefix, suffix, language),
    )
    .await
}

async fn fim_request(
    endpoint: impl FnOnce() -> Result<Option<FimEndpoint>, String>,
    client: impl FnOnce() -> Result<Box<dyn LlmClient>, String>,
    prefix: &str,
    suffix: &str,
    language: &str,
) -> Result<String, String> {
    // Only the code near the cursor is sent, so a large file costs no more
    // than a small one.
    let prefix_trimmed = last_n_lines(prefix, PREFIX_LINES);
    let suffix_trimmed = first_n_lines(suffix, SUFFIX_LINES);

    if let Some(endpoint) = endpoint()?.filter(|e| !e.turned_down()) {
        match endpoint.complete(prefix_trimmed, suffix_trimmed).await {
            Ok(text) => return Ok(tidy(&text, suffix_trimmed)),
            // This model or server has no fill-in-the-middle: ask with the
            // chat prompt, now and for the rest of the session.
            Err(FimError::TurnedDown) => endpoint.remember_turned_down(),
            Err(FimError::Failed(e)) => return Err(e),
        }
    }
    chat_request(client, prefix_trimmed, suffix_trimmed, language).await
}

/// The chat prompt that stands in for a fill-in-the-middle API.
async fn chat_request(
    client: impl FnOnce() -> Result<Box<dyn LlmClient>, String>,
    prefix_trimmed: &str,
    suffix_trimmed: &str,
    language: &str,
) -> Result<String, String> {
    let client = client()?;

    let system = format!(
        "You are an expert {language} code completion engine. \
         The user will show you code with a <CURSOR> marker. \
         Output ONLY the text that should be inserted at <CURSOR> — \
         no explanation, no markdown, no code fences, no repetition of \
         surrounding lines. If no completion is appropriate, output nothing."
    );

    let user = format!(
        "Complete the {language} code at <CURSOR>:\n\n\
         ```{language}\n{prefix_trimmed}<CURSOR>{suffix_trimmed}\n```"
    );

    let messages = vec![Message::system(system), Message::user(user)];

    let resp = client
        .chat(&messages, &[])
        .await
        .map_err(|e| e.to_string())?;

    let raw = resp.message.content.trim().to_string();

    // Strip any accidental markdown fences the model might output.
    let cleaned = raw.trim_start_matches("```").trim_end_matches("```").trim();

    // Drop if the model repeated the prefix (common failure mode).
    if cleaned.is_empty() || prefix_trimmed.contains(cleaned) {
        return Ok(String::new());
    }

    // Limit to 3 lines so ghost text stays readable.
    let limited: String = cleaned.lines().take(3).collect::<Vec<_>>().join("\n");
    Ok(limited)
}

/// Shape a fill-in-the-middle answer for the editor. Leading whitespace is
/// kept (it is part of the insertion: an indent, or the newline that starts
/// a block); trailing whitespace is not. Text the model wrote that is already
/// on the cursor's line after it (a closing `)` or `;`) is dropped, so
/// accepting the suggestion doesn't double it; a later line is left alone,
/// since a block the model closes is often not the one already closed there.
fn tidy(completion: &str, suffix: &str) -> String {
    let lines: Vec<&str> = completion.trim_end().split('\n').take(MAX_LINES).collect();
    let text = lines.join("\n");
    let text = text.trim_end();
    if text.trim().is_empty() {
        return String::new();
    }
    let rest_of_line = suffix.split('\n').next().unwrap_or("").trim_end();
    let mut keep = text.len();
    for (start, _) in text.char_indices() {
        let tail = &text[start..];
        if !tail.contains('\n') && !tail.trim().is_empty() && rest_of_line.starts_with(tail) {
            keep = start;
            break;
        }
    }
    let text = text[..keep].trim_end();
    if text.trim().is_empty() {
        String::new()
    } else {
        text.to_string()
    }
}

/// A provider's fill-in-the-middle API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FimApi {
    /// llama.cpp's server (PhazeAI Local): `POST /infill`.
    LlamaInfill,
    /// Ollama: `POST /api/generate` with a `suffix`.
    Ollama,
    /// DeepSeek: `POST /beta/completions` with a `suffix`.
    DeepSeek,
    /// Mistral: `POST /v1/fim/completions` (Codestral).
    Mistral,
}

impl FimApi {
    /// The API `provider` has, if Phaze speaks it.
    pub fn for_provider(provider: &ProviderId) -> Option<Self> {
        match provider {
            ProviderId::BuiltIn => Some(Self::LlamaInfill),
            ProviderId::Ollama => Some(Self::Ollama),
            ProviderId::DeepSeek => Some(Self::DeepSeek),
            ProviderId::Mistral => Some(Self::Mistral),
            _ => None,
        }
    }

    /// The address of the API at a provider whose requests go to `base_url`.
    fn url(self, base_url: &str) -> String {
        let base = base_url.trim().trim_end_matches('/');
        // Ollama's and DeepSeek's FIM paths sit beside `/v1`, not under it.
        let root = base.strip_suffix("/v1").unwrap_or(base);
        match self {
            Self::LlamaInfill => format!("{root}/infill"),
            Self::Ollama => format!("{root}/api/generate"),
            Self::DeepSeek => format!("{root}/beta/completions"),
            Self::Mistral => crate::llm::openai::openai_compat_url(base, "fim/completions"),
        }
    }
}

/// Where and how a completion is asked for natively: the active provider's
/// fill-in-the-middle API, with the model and key the request uses.
#[derive(Clone)]
pub struct FimEndpoint {
    api: FimApi,
    provider: ProviderId,
    /// The provider's address, as the privacy guard judges it.
    base_url: String,
    url: String,
    model: String,
    api_key: Option<String>,
}

impl std::fmt::Debug for FimEndpoint {
    // Leaves the key out.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FimEndpoint")
            .field("api", &self.api)
            .field("url", &self.url)
            .field("model", &self.model)
            .finish()
    }
}

/// Why a fill-in-the-middle request brought no completion.
enum FimError {
    /// The server or model has no fill-in-the-middle (404, 501, or a 400 or
    /// 422 that says so): ask with the chat prompt instead.
    TurnedDown,
    /// Anything else: a refused key, a full quota, a dropped connection, or
    /// `local_only`. The chat prompt would fail the same way.
    Failed(String),
}

impl FimEndpoint {
    /// The active provider's fill-in-the-middle API in `settings`, at the
    /// address and with the key its chat requests use; `None` when the
    /// provider has none Phaze speaks.
    pub fn for_settings(settings: &Settings) -> Result<Option<Self>, crate::PhazeError> {
        let provider = settings.llm.provider.to_provider_id();
        let Some(api) = FimApi::for_provider(&provider) else {
            return Ok(None);
        };
        settings.check_base_url()?;
        let registry = settings.build_provider_registry();
        let config = registry.active_config().ok_or_else(|| {
            crate::PhazeError::Config(format!("Provider {provider:?} not configured"))
        })?;
        let base_url = crate::llm::provider::request_base_url(config)?;
        let chosen = settings
            .editor
            .completion_model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string);
        let model = match api {
            // PhazeAI Local answers with the model it has loaded.
            FimApi::LlamaInfill => crate::local_ai::LOCAL_AI
                .running_model()
                .unwrap_or_else(|| settings.small_model()),
            // Codestral is Mistral's FIM model; its chat models have none.
            FimApi::Mistral => chosen.unwrap_or_else(|| "codestral-latest".to_string()),
            _ => chosen.unwrap_or_else(|| settings.small_model()),
        };
        let api_key = if provider.needs_api_key() {
            Some(config.api_key().ok_or_else(|| {
                crate::PhazeError::Config(format!(
                    "Set {} environment variable for {}",
                    config.api_key_env,
                    provider.name()
                ))
            })?)
        } else {
            None
        };
        Ok(Some(Self::new(api, provider, base_url, model, api_key)))
    }

    fn new(
        api: FimApi,
        provider: ProviderId,
        base_url: String,
        model: String,
        api_key: Option<String>,
    ) -> Self {
        Self {
            url: api.url(&base_url),
            api,
            provider,
            base_url,
            model,
            api_key,
        }
    }

    /// The models and servers that turned a fill-in-the-middle request down
    /// this session, by address and model.
    fn turned_down_list() -> &'static Mutex<HashSet<(String, String)>> {
        static LIST: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
        LIST.get_or_init(Default::default)
    }

    fn turned_down(&self) -> bool {
        Self::turned_down_list()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(self.url.clone(), self.model.clone()))
    }

    fn remember_turned_down(&self) {
        Self::turned_down_list()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((self.url.clone(), self.model.clone()));
    }

    fn body(&self, prefix: &str, suffix: &str) -> Value {
        match self.api {
            FimApi::LlamaInfill => json!({
                "input_prefix": prefix,
                "input_suffix": suffix,
                "n_predict": MAX_TOKENS,
                "temperature": 0.1,
                "stop": STOP,
                // Reuse the server's cache of the code before the cursor.
                "cache_prompt": true,
            }),
            FimApi::Ollama => json!({
                "model": self.model,
                "prompt": prefix,
                "suffix": suffix,
                "stream": false,
                "options": {
                    "num_predict": MAX_TOKENS,
                    "temperature": 0.1,
                    "stop": STOP,
                },
            }),
            FimApi::DeepSeek | FimApi::Mistral => json!({
                "model": self.model,
                "prompt": prefix,
                "suffix": suffix,
                "max_tokens": MAX_TOKENS,
                "temperature": 0.0,
                "stop": STOP,
                "stream": false,
            }),
        }
    }

    /// The completion in a successful answer.
    fn completion(&self, answer: &Value) -> Option<String> {
        let text = match self.api {
            FimApi::LlamaInfill => answer["content"].as_str(),
            FimApi::Ollama => answer["response"].as_str(),
            FimApi::DeepSeek => answer["choices"][0]["text"].as_str(),
            FimApi::Mistral => {
                let choice = &answer["choices"][0];
                choice["message"]["content"]
                    .as_str()
                    .or_else(|| choice["text"].as_str())
            }
        };
        text.map(str::to_string)
    }

    /// One fill-in-the-middle request. It goes through the same checks as
    /// a chat request: `local_only` first, then the privacy audit log.
    async fn complete(&self, prefix: &str, suffix: &str) -> Result<String, FimError> {
        crate::privacy::check_provider(&self.provider, &self.base_url, &self.model)
            .map_err(|e| FimError::Failed(e.to_string()))?;
        crate::privacy::audit::record_request(
            self.provider.name(),
            &self.model,
            prefix.chars().count() + suffix.chars().count(),
            crate::privacy::audit::take_files(),
        );

        let direct = crate::privacy::is_local_endpoint(&self.provider, &self.base_url);
        let mut request = http_client(direct)
            .post(&self.url)
            .json(&self.body(prefix, suffix));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| FimError::Failed(format!("{} completion: {e}", self.provider.name())))?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(if turned_down(status, &text) {
                FimError::TurnedDown
            } else {
                FimError::Failed(format!(
                    "{} completion failed ({status}): {}",
                    self.provider.name(),
                    text.chars().take(300).collect::<String>()
                ))
            });
        }
        let answer: Value = serde_json::from_str(&text)
            .map_err(|e| FimError::Failed(format!("{} completion: {e}", self.provider.name())))?;
        Ok(self.completion(&answer).unwrap_or_default())
    }
}

/// Whether an error answer means the server or model has no
/// fill-in-the-middle, rather than that this request failed: llama.cpp
/// answers 501 for a model without FIM tokens, a server without the route
/// answers 404, and Ollama answers 400 "does not support insert".
fn turned_down(status: u16, body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    match status {
        404 | 405 | 501 => true,
        400 | 422 => {
            lower.contains("support") || lower.contains("suffix") || lower.contains("model")
        }
        _ => false,
    }
}

/// The HTTP client for fill-in-the-middle requests: one that goes through
/// the configured proxy, and one that never does, for a server on this
/// machine (a proxy would carry the code off it while `local_only` allows it).
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

fn last_n_lines(s: &str, n: usize) -> &str {
    let mut count = 0;
    let mut pos = 0;
    for (i, b) in s.bytes().enumerate().rev() {
        if b == b'\n' {
            count += 1;
            if count == n {
                pos = i + 1;
                break;
            }
        }
    }
    &s[pos..]
}

fn first_n_lines(s: &str, n: usize) -> &str {
    let mut count = 0;
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            count += 1;
            if count == n {
                return &s[..i];
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PhazeError;
    use crate::llm::{LlmResponse, StreamEvent};
    use crate::tools::ToolDefinition;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn nothing_is_requested_while_ai_completions_are_off() {
        let policy = PrivacyPolicy::defaults();
        assert!(!should_request_fim(
            false,
            &policy,
            Path::new("/p/src/main.rs")
        ));
    }

    #[test]
    fn on_by_default_for_a_file_the_policy_allows() {
        let on = Settings::default().editor.ai_completions;
        assert!(should_request_fim(
            on,
            &PrivacyPolicy::defaults(),
            Path::new("/p/src/main.rs")
        ));
    }

    #[test]
    fn a_denied_file_is_never_sent_even_when_on() {
        // The built-in secrets list…
        let defaults = PrivacyPolicy::defaults();
        assert!(!should_request_fim(true, &defaults, Path::new("/p/.env")));
        // …and the project's own deny rules.
        let dir = tempfile::tempdir().unwrap();
        let policy = PrivacyPolicy::parse(dir.path(), "deny = [\"secret/**\"]\n").unwrap();
        let root = policy.root().unwrap().to_path_buf();
        assert!(!should_request_fim(
            true,
            &policy,
            &root.join("secret/key.rs")
        ));
        assert!(should_request_fim(true, &policy, &root.join("src/main.rs")));
    }

    /// Stands in for the provider: counts the requests that reach it.
    struct CountingProvider(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl LlmClient for CountingProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
        ) -> Result<LlmResponse, PhazeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(LlmResponse {
                message: Message::assistant("+ 1;"),
                usage: None,
            })
        }
        async fn chat_stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
        ) -> Result<futures::channel::mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(PhazeError::Other("not used by completions".into()))
        }
    }

    /// One completion through the path `fim_complete` takes, with
    /// `[editor] ai_completions = on`. Returns the result and how many
    /// requests reached the provider.
    async fn complete(
        on: bool,
        policy: &PrivacyPolicy,
        path: &str,
    ) -> (Result<String, String>, usize) {
        let requests = Arc::new(AtomicUsize::new(0));
        let provider = {
            let requests = Arc::clone(&requests);
            move || Ok(Box::new(CountingProvider(requests)) as Box<dyn LlmClient>)
        };
        let path = Path::new(path);
        let result = fim_complete_with(
            on,
            policy,
            || Ok(None),
            provider,
            path,
            "let total = 1",
            ";\n",
            "rust",
        )
        .await;
        (result, requests.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn no_request_reaches_the_provider_while_ai_completions_are_off() {
        let policy = PrivacyPolicy::defaults();
        let (result, requests) = complete(false, &policy, "/p/src/main.rs").await;
        assert_eq!(requests, 0, "nothing is sent while AI completions are off");
        assert_eq!(result, Ok(String::new()));

        // The same completion with the switch on does reach the provider, so
        // the zero above comes from the switch.
        let (result, requests) = complete(true, &policy, "/p/src/main.rs").await;
        assert_eq!(requests, 1);
        assert_eq!(result, Ok("+ 1;".to_string()));
    }

    #[tokio::test]
    async fn no_request_reaches_the_provider_for_a_denied_file() {
        let (result, requests) = complete(true, &PrivacyPolicy::defaults(), "/p/.env").await;
        assert_eq!(requests, 0, "a file the policy denies is never sent");
        assert_eq!(result, Ok(String::new()));
    }

    // ── Shaping a fill-in-the-middle answer ──────────────────────────────

    #[test]
    fn a_short_file_sends_all_of_its_code_before_the_cursor() {
        // Fewer lines than the limit: everything goes (it used to send nothing).
        assert_eq!(
            last_n_lines("fn main() {\n    let x", 40),
            "fn main() {\n    let x"
        );
        assert_eq!(last_n_lines("a\nb\nc\nd", 2), "c\nd");
        assert_eq!(first_n_lines("a\nb\nc", 2), "a\nb");
        assert_eq!(first_n_lines("a", 2), "a");
    }

    #[test]
    fn text_already_on_the_cursors_line_is_not_suggested_twice() {
        // `add(|)` → the model writes `a, b)`; the `)` is already there.
        assert_eq!(tidy("a, b)", ")\n}"), "a, b");
        assert_eq!(tidy("x + 1;", ";\n"), "x + 1");
        // Nothing in common: kept whole.
        assert_eq!(tidy("a, b", ")"), "a, b");
    }

    #[test]
    fn a_block_the_model_closes_is_kept_even_when_one_closes_below() {
        // The `}` below closes the function; the model's closes its own `if`.
        let completion = "\n    if x {\n        y();\n    }";
        assert_eq!(tidy(completion, "\n}\n"), completion);
    }

    #[test]
    fn leading_indent_is_kept_and_trailing_space_and_extra_lines_dropped() {
        assert_eq!(tidy("    let y = 2;  \n\n", ""), "    let y = 2;");
        let long = "1\n2\n3\n4\n5\n6\n7\n8";
        assert_eq!(tidy(long, ""), "1\n2\n3\n4\n5\n6");
        assert_eq!(tidy("  \n ", ""), "");
        assert_eq!(tidy(")", ")"), "", "only what is already there");
    }

    #[test]
    fn each_provider_api_has_its_own_address() {
        let url = |api: FimApi, base: &str| api.url(base);
        assert_eq!(
            url(FimApi::LlamaInfill, "http://127.0.0.1:8090"),
            "http://127.0.0.1:8090/infill"
        );
        assert_eq!(
            url(FimApi::Ollama, "http://localhost:11434"),
            "http://localhost:11434/api/generate"
        );
        assert_eq!(
            url(FimApi::Ollama, "http://localhost:11434/v1/"),
            "http://localhost:11434/api/generate"
        );
        assert_eq!(
            url(FimApi::DeepSeek, "https://api.deepseek.com/v1"),
            "https://api.deepseek.com/beta/completions"
        );
        assert_eq!(
            url(FimApi::Mistral, "https://api.mistral.ai/v1"),
            "https://api.mistral.ai/v1/fim/completions"
        );
        assert_eq!(
            url(FimApi::Mistral, "https://codestral.mistral.ai"),
            "https://codestral.mistral.ai/v1/fim/completions"
        );
        assert_eq!(FimApi::for_provider(&ProviderId::OpenAI), None);
        assert_eq!(
            FimApi::for_provider(&ProviderId::BuiltIn),
            Some(FimApi::LlamaInfill)
        );
    }

    #[test]
    fn only_an_answer_that_says_there_is_no_fill_in_the_middle_falls_back() {
        assert!(turned_down(501, "Infill is not supported by this model"));
        assert!(turned_down(404, ""));
        assert!(turned_down(
            400,
            r#"{"error":"qwen3 does not support insert"}"#
        ));
        assert!(!turned_down(401, "invalid api key"));
        assert!(!turned_down(429, "rate limited"));
        assert!(!turned_down(500, "model crashed"));
    }

    // ── PhazeAI Local's /infill ──────────────────────────────────────────

    /// A llama.cpp server on 127.0.0.1 that answers `/infill` with `status`
    /// and `body`, and records each request's line and JSON body.
    async fn llama_server(
        status: u16,
        body: &'static str,
    ) -> (String, Arc<std::sync::Mutex<Vec<(String, Value)>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&log);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 16 * 1024];
                let request = loop {
                    let Ok(n) = sock.read(&mut chunk).await else {
                        break None;
                    };
                    if n == 0 {
                        break None;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).to_string();
                    let len: usize = head
                        .to_ascii_lowercase()
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .map(|v| v.trim().to_string())
                        })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    if buf.len() < end + 4 + len {
                        continue;
                    }
                    let line = head.lines().next().unwrap_or_default().to_string();
                    let json =
                        serde_json::from_slice(&buf[end + 4..end + 4 + len]).unwrap_or(Value::Null);
                    break Some((line, json));
                };
                let Some(request) = request else { continue };
                seen.lock().unwrap().push(request);
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        (address, log)
    }

    fn local(base: &str) -> Option<FimEndpoint> {
        Some(FimEndpoint::new(
            FimApi::LlamaInfill,
            ProviderId::BuiltIn,
            base.to_string(),
            "qwen2.5-coder-1.5b-instruct".to_string(),
            None,
        ))
    }

    #[tokio::test]
    async fn phazeai_local_fills_in_the_middle_with_no_prompt_around_the_code() {
        let (server, log) = llama_server(200, r#"{"content":"a + b\n"}"#).await;
        let requests = Arc::new(AtomicUsize::new(0));
        let chat = {
            let requests = Arc::clone(&requests);
            move || Ok(Box::new(CountingProvider(requests)) as Box<dyn LlmClient>)
        };
        let result = fim_complete_with(
            true,
            &PrivacyPolicy::defaults(),
            || Ok(local(&server)),
            chat,
            Path::new("/p/src/lib.rs"),
            "fn add(a: i32, b: i32) -> i32 {\n    ",
            "\n}\n",
            "rust",
        )
        .await;

        assert_eq!(result, Ok("a + b".to_string()));
        assert_eq!(requests.load(Ordering::SeqCst), 0, "no chat request");
        let seen = log.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "POST /infill HTTP/1.1");
        let body = &seen[0].1;
        assert_eq!(
            body["input_prefix"],
            "fn add(a: i32, b: i32) -> i32 {\n    "
        );
        assert_eq!(body["input_suffix"], "\n}\n");
        assert_eq!(body["n_predict"], MAX_TOKENS);
        assert!(
            !body.to_string().contains("CURSOR"),
            "no chat prompt: {body}"
        );
    }

    #[tokio::test]
    async fn a_model_without_fill_in_the_middle_is_asked_by_chat_from_then_on() {
        let (server, log) = llama_server(
            501,
            r#"{"error":{"message":"Infill is not supported by this model"}}"#,
        )
        .await;
        let requests = Arc::new(AtomicUsize::new(0));
        let chat = || {
            let requests = Arc::clone(&requests);
            move || Ok(Box::new(CountingProvider(requests)) as Box<dyn LlmClient>)
        };
        let policy = PrivacyPolicy::defaults();
        let ask = |chat| {
            fim_complete_with(
                true,
                &policy,
                || Ok(local(&server)),
                chat,
                Path::new("/p/src/lib.rs"),
                "let total = 1",
                ";\n",
                "rust",
            )
        };

        assert_eq!(ask(chat()).await, Ok("+ 1;".to_string()));
        assert_eq!(requests.load(Ordering::SeqCst), 1, "answered by chat");
        // The next completion skips the fill-in-the-middle request.
        assert_eq!(ask(chat()).await, Ok("+ 1;".to_string()));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(log.lock().unwrap().len(), 1, "asked /infill once");
    }

    #[tokio::test]
    async fn a_failed_request_is_not_retried_by_chat() {
        let (server, _log) = llama_server(503, r#"{"error":"loading model"}"#).await;
        let requests = Arc::new(AtomicUsize::new(0));
        let chat = {
            let requests = Arc::clone(&requests);
            move || Ok(Box::new(CountingProvider(requests)) as Box<dyn LlmClient>)
        };
        let result = fim_complete_with(
            true,
            &PrivacyPolicy::defaults(),
            || Ok(local(&server)),
            chat,
            Path::new("/p/src/lib.rs"),
            "let total = 1",
            ";\n",
            "rust",
        )
        .await;
        assert!(result.unwrap_err().contains("503"));
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn nothing_is_sent_natively_for_a_denied_file_either() {
        let (server, log) = llama_server(200, r#"{"content":"x"}"#).await;
        let result = fim_complete_with(
            true,
            &PrivacyPolicy::defaults(),
            || Ok(local(&server)),
            || Err("no chat".to_string()),
            Path::new("/p/.env"),
            "KEY=",
            "",
            "text",
        )
        .await;
        assert_eq!(result, Ok(String::new()));
        assert!(log.lock().unwrap().is_empty());
    }
}
