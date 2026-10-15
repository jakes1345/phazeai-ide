//! Context compaction: when a conversation outgrows its token budget, the
//! oldest part is replaced by a summary written by the same model, instead of
//! being silently dropped. Recent messages always stay word for word.

use crate::context::ConversationHistory;
use crate::llm::{LlmClient, Message};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Compact once the conversation passes this share of the budget…
pub const TRIGGER_RATIO: f64 = 0.8;
/// …keeping about this share of the budget as verbatim recent messages.
pub const KEEP_RECENT_RATIO: f64 = 0.4;
/// The summary may use at most this share of the budget.
pub const SUMMARY_RATIO: f64 = 0.15;
/// Error returned when the cancel flag is raised while the summary is written.
pub const CANCELLED: &str = "Cancelled";

/// A tiny budget still gets a summary long enough to be useful.
const MIN_SUMMARY_TOKENS: usize = 250;
/// How often the cancel flag is checked during the summary request.
const CANCEL_POLL: Duration = Duration::from_millis(100);

pub const SYSTEM_PROMPT: &str = "\
You compress the early part of a coding session so the assistant can keep working \
without the full transcript. Write a factual summary in Markdown with these sections, \
omitting any that would be empty:
- **Goal**: what the user wants overall, in their terms.
- **Decisions & constraints**: choices made, rules the user gave, things to avoid.
- **Work done**: files created/changed (paths) and what changed; commands run and results.
- **Findings**: facts learned about the codebase (paths, function names, how things work).
- **Open items**: what is unfinished, failing, or next.
Keep exact identifiers, paths, error messages and numbers. Do not invent anything. \
Do not address the user. If a previous summary is given, merge it in: keep what is still \
true, update what changed.";

/// The request that asks the model for a summary.
pub fn request(previous: Option<&str>, transcript: &str, max_words: usize) -> Vec<Message> {
    let mut user = String::new();
    if let Some(prev) = previous {
        user.push_str("Previous summary:\n<previous_summary>\n");
        user.push_str(prev);
        user.push_str("\n</previous_summary>\n\n");
    }
    user.push_str("Transcript to fold in:\n<transcript>\n");
    user.push_str(transcript);
    user.push_str(&format!(
        "</transcript>\n\nWrite the updated summary in at most {max_words} words."
    ));
    vec![Message::system(SYSTEM_PROMPT), Message::user(user)]
}

/// How long one summary may be: the word count the model is asked for and
/// the byte cap enforced on what it returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryLimits {
    pub max_words: usize,
    pub max_bytes: usize,
}

/// Limits for a summary under `budget`. Both come from one token allowance
/// ([`SUMMARY_RATIO`] of the budget): ~0.6 words and ~4 bytes per token, so a
/// summary that respects the word limit (6–7 bytes per word including the
/// space) fits under the byte cap instead of being cut off.
pub fn summary_limits(budget: usize) -> SummaryLimits {
    let tokens = ((budget as f64 * SUMMARY_RATIO) as usize).max(MIN_SUMMARY_TOKENS);
    SummaryLimits {
        max_words: tokens * 3 / 5,
        max_bytes: tokens * 4,
    }
}

/// `summary` cut down to at most `max_bytes`, at the last paragraph break,
/// else sentence end, else line break, else word break before the limit,
/// never mid-word. A break is only used if it keeps at least half the allowed
/// text, so a stray early paragraph break doesn't throw most of it away.
pub fn fit_summary(summary: &str, max_bytes: usize) -> &str {
    if summary.len() <= max_bytes {
        return summary;
    }
    let head = crate::text::truncate_bytes(summary, max_bytes);
    let floor = max_bytes / 2;
    let cut = [
        head.rfind("\n\n"),
        last_sentence_end(summary, head.len()),
        head.rfind('\n'),
    ]
    .into_iter()
    .flatten()
    .find(|&i| i >= floor)
    .or_else(|| head.rfind(char::is_whitespace))
    .unwrap_or(head.len());
    head[..cut].trim_end()
}

/// Byte index just past the last `.`, `!` or `?` in `text[..limit]` that is
/// followed by whitespace or the end of `text` (so `a.rs` and `3.5` don't count).
fn last_sentence_end(text: &str, limit: usize) -> Option<usize> {
    text[..limit]
        .char_indices()
        .filter(|&(i, c)| {
            matches!(c, '.' | '!' | '?')
                && text[i + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
        .map(|(i, c)| i + c.len_utf8())
        .next_back()
}

/// Outcome of one compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compacted {
    pub messages_folded: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
}

/// Compact `conversation` if it is over [`TRIGGER_RATIO`] of `budget`.
///
/// The conversation lock is not held while the model writes the summary;
/// `lock` is called again to apply it, and the result is only applied if the
/// compacted prefix is unchanged. Returns `Ok(None)` when nothing needed doing.
///
/// `cancel`, when given, is polled every 100 ms while the model writes the
/// summary; once raised, the request is dropped and `Err(CANCELLED)` returned.
pub async fn maybe_compact(
    conversation: &tokio::sync::Mutex<ConversationHistory>,
    client: &dyn LlmClient,
    budget: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Option<Compacted>, String> {
    let (cut, transcript, previous, tokens_before, prefix_fingerprint) = {
        let conv = conversation.lock().await;
        let tokens_before = conv.estimate_tokens();
        if (tokens_before as f64) < budget as f64 * TRIGGER_RATIO {
            return Ok(None);
        }
        let Some(cut) = conv.compaction_point((budget as f64 * KEEP_RECENT_RATIO) as usize) else {
            return Ok(None);
        };
        let transcript = conv.transcript(cut);
        (
            cut,
            transcript.clone(),
            conv.summary().map(str::to_string),
            tokens_before,
            transcript,
        )
    };

    let limits = summary_limits(budget);
    let messages = request(previous.as_deref(), &transcript, limits.max_words);
    let chat = client.chat(&messages, &[]);
    let reply = tokio::select! {
        reply = chat => reply.map_err(|e| e.to_string())?,
        _ = cancelled(cancel) => return Err(CANCELLED.to_string()),
    };
    let summary = reply.message.content.trim().to_string();
    if summary.is_empty() {
        return Err("the model returned an empty summary".to_string());
    }
    // Never let the summary itself blow the budget.
    let summary = fit_summary(&summary, limits.max_bytes).to_string();

    let mut conv = conversation.lock().await;
    if conv.len() < cut || conv.transcript(cut) != prefix_fingerprint {
        return Err("conversation changed while summarising".to_string());
    }
    conv.compact(cut, summary);
    Ok(Some(Compacted {
        messages_folded: cut,
        tokens_before,
        tokens_after: conv.estimate_tokens(),
    }))
}

/// Resolves once `flag` is raised; never, without a flag.
async fn cancelled(flag: Option<&AtomicBool>) {
    let Some(flag) = flag else {
        return std::future::pending().await;
    };
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(CANCEL_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::OpenAIClient;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serve one chat completion from a real local socket and return the
    /// request body it received.
    async fn serve_completion(content: &str) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let body = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": content, "tool_calls": []}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })
        .to_string();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 256 * 1024];
            let mut total = 0;
            // Read until the JSON body is complete (Content-Length).
            loop {
                let n = sock.read(&mut buf[total..]).await.unwrap();
                total += n;
                let text = String::from_utf8_lossy(&buf[..total]);
                if let Some(idx) = text.find("\r\n\r\n") {
                    let len: usize = text
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .or_else(|| {
                            text.lines()
                                .find_map(|l| l.strip_prefix("Content-Length: "))
                        })
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if total >= idx + 4 + len || n == 0 {
                        let _ = tx.send(text[idx + 4..].to_string());
                        break;
                    }
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(body.as_bytes()).await.unwrap();
            sock.shutdown().await.unwrap();
        });
        (format!("http://{addr}"), rx)
    }

    /// Accept one connection and never answer on it.
    async fn serve_nothing() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        format!("http://{addr}")
    }

    /// Twelve alternating turns of ~100 tokens each (~1200 tokens in all).
    async fn long_conversation() -> tokio::sync::Mutex<ConversationHistory> {
        let conversation = tokio::sync::Mutex::new(ConversationHistory::new());
        {
            let mut c = conversation.lock().await;
            for n in 0..12 {
                let text = format!("turn {n:02} {}", "x".repeat(292));
                if n % 2 == 0 {
                    c.add_user_message(text);
                } else {
                    c.add_assistant_message(text);
                }
            }
        }
        conversation
    }

    #[tokio::test]
    async fn folds_the_oldest_messages_into_a_model_written_summary() {
        let conversation = long_conversation().await;
        let (base, request_rx) = serve_completion("**Goal**: finish turn 11.").await;
        let client = OpenAIClient::new("test-key").with_base_url(base);

        // Budget 1000: 1200 is over the 80% trigger; ~400 tokens (4 messages) stay.
        let done = maybe_compact(&conversation, &client, 1000, None)
            .await
            .unwrap()
            .expect("compaction should run");
        assert_eq!(done.messages_folded, 8);
        assert!(done.tokens_after < done.tokens_before);

        let c = conversation.lock().await;
        assert_eq!(c.len(), 4);
        assert_eq!(c.folded_count(), 8);
        assert_eq!(c.summary(), Some("**Goal**: finish turn 11."));
        assert!(c.get_conversation_messages()[0]
            .content
            .starts_with("turn 08"));

        // The model saw the folded transcript and our instructions.
        let sent = request_rx.await.unwrap();
        assert!(sent.contains("turn 07"));
        assert!(!sent.contains("turn 08"));
        assert!(sent.contains("Transcript to fold in"));
        // The word limit comes from the budget's summary allowance.
        let words = summary_limits(1000).max_words;
        assert!(sent.contains(&format!("at most {words} words")));
    }

    #[tokio::test]
    async fn does_nothing_under_the_trigger() {
        let conversation = tokio::sync::Mutex::new(ConversationHistory::new());
        conversation.lock().await.add_user_message("hi");
        // No server: a request would fail, so this proves none was made.
        let client = OpenAIClient::new("k").with_base_url("http://127.0.0.1:9");
        assert_eq!(
            maybe_compact(&conversation, &client, 1000, None).await,
            Ok(None)
        );
    }

    #[tokio::test]
    async fn stop_interrupts_the_summary_request() {
        let conversation = long_conversation().await;
        let client = OpenAIClient::new("k").with_base_url(serve_nothing().await);
        let cancel = AtomicBool::new(false);

        let started = std::time::Instant::now();
        let (result, _) = tokio::join!(
            maybe_compact(&conversation, &client, 1000, Some(&cancel)),
            async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                cancel.store(true, Ordering::Relaxed);
            }
        );
        let elapsed = started.elapsed();

        assert_eq!(result, Err(CANCELLED.to_string()));
        assert!(
            elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(1),
            "returned after {elapsed:?}"
        );
        // Nothing was applied.
        let c = conversation.lock().await;
        assert_eq!(c.len(), 12);
        assert_eq!(c.summary(), None);
        assert_eq!(c.folded_count(), 0);
    }

    #[test]
    fn request_includes_previous_summary_and_word_limit() {
        let msgs = request(Some("old"), "[User]\nhello\n", 150);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, SYSTEM_PROMPT);
        assert!(msgs[1]
            .content
            .contains("<previous_summary>\nold\n</previous_summary>"));
        assert!(msgs[1].content.contains("at most 150 words"));
        assert!(!request(None, "t", 10)[1]
            .content
            .contains("previous_summary"));
    }

    #[test]
    fn word_limit_and_byte_cap_share_one_token_allowance() {
        // 15% of a 32k budget is 4915 tokens: ~2949 words, ~19660 bytes.
        let limits = summary_limits(32_768);
        assert_eq!(limits.max_words, 2949);
        assert_eq!(limits.max_bytes, 19_660);
        assert!(limits.max_bytes >= limits.max_words * 6);
        // A summary right at the word limit, made of average-length words
        // (five letters and a space), is not cut.
        let at_limit = vec!["about"; limits.max_words].join(" ");
        assert_eq!(fit_summary(&at_limit, limits.max_bytes), at_limit);
        // Before the fix, the byte cap (0.15 × budget × 3) was below what the
        // word limit allowed; now the cap always covers it.
        let old_cap = (32_768.0 * SUMMARY_RATIO * 3.0) as usize;
        assert!(at_limit.len() > old_cap && at_limit.len() <= limits.max_bytes);
        // Tiny budgets still get room for a usable summary.
        assert_eq!(
            summary_limits(100),
            SummaryLimits {
                max_words: 150,
                max_bytes: 1000
            }
        );
    }

    #[test]
    fn an_oversized_summary_is_cut_at_a_boundary_not_mid_word() {
        let text = "- **Goal**: ship it.\n\n\
                    - **Work done**: edited `src/a.rs`. Ran tests!\n\n\
                    - **Open items**: fix flaky test.";
        assert_eq!(text.len(), 103);
        // Fits: untouched.
        assert_eq!(fit_summary(text, text.len()), text);
        // A little too long: drop the last paragraph.
        assert_eq!(
            fit_summary(text, text.len() - 5),
            "- **Goal**: ship it.\n\n- **Work done**: edited `src/a.rs`. Ran tests!"
        );
        // The paragraph break would keep under half: cut at the sentence end
        // instead (and `a.rs` is not one).
        assert_eq!(
            fit_summary(text, 60),
            "- **Goal**: ship it.\n\n- **Work done**: edited `src/a.rs`."
        );
        // The early paragraph break keeps at least half of 25 bytes.
        assert_eq!(fit_summary(text, 25), "- **Goal**: ship it.");
        // No punctuation at all: the last word break.
        assert_eq!(fit_summary("alpha beta gamma delta", 12), "alpha beta");
        // Lines without sentence punctuation break at the line.
        assert_eq!(
            fit_summary("- path src/x.rs\n- path src/y.rs\n- path src/z.rs", 40),
            "- path src/x.rs\n- path src/y.rs"
        );
        // Never inside a multi-byte character or a word.
        assert_eq!(fit_summary("héllo wörld", 9), "héllo");
        // One unbreakable word still respects the cap.
        assert_eq!(fit_summary("abcdefghij", 4), "abcd");
    }
}
