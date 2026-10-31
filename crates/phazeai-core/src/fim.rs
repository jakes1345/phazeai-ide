//! Fill-in-Middle (FIM) inline code completion.
//!
//! Uses the active LLM with a tight chat prompt that emulates FIM behaviour.
//! The model sees the code before and after the cursor and must output ONLY
//! the text to insert at the cursor — no explanation, no markdown, no repeating
//! the surrounding code.

use std::path::Path;

use crate::config::Settings;
use crate::llm::{LlmClient, Message};
use crate::privacy::PrivacyPolicy;

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
/// `language` — language hint for the prompt ("rust", "python", …).
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
    let settings = Settings::load();
    let policy = crate::privacy::active();
    let client = || settings.build_llm_client().map_err(|e| e.to_string());
    fim_complete_with(
        settings.editor.ai_completions,
        &policy,
        client,
        path,
        prefix,
        suffix,
        language,
    )
    .await
}

/// [`fim_complete`] with the setting, the policy and the provider passed in.
/// `client` is only called, and the request only sent, when
/// [`should_request_fim`] allows it.
async fn fim_complete_with(
    ai_completions: bool,
    policy: &PrivacyPolicy,
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
        fim_request(client, prefix, suffix, language),
    )
    .await
}

async fn fim_request(
    client: impl FnOnce() -> Result<Box<dyn LlmClient>, String>,
    prefix: &str,
    suffix: &str,
    language: &str,
) -> Result<String, String> {
    let client = client()?;

    // Trim prefix to last ~40 lines and suffix to next ~20 lines so we
    // don't blow the context window on large files.
    let prefix_trimmed = last_n_lines(prefix, 40);
    let suffix_trimmed = first_n_lines(suffix, 20);

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

fn last_n_lines(s: &str, n: usize) -> &str {
    let mut count = 0;
    let mut pos = s.len();
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
        let result =
            fim_complete_with(on, policy, provider, path, "let total = 1", ";\n", "rust").await;
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
}
