//! Context compaction: when a conversation outgrows its token budget, the
//! oldest part is replaced by a summary written by the same model, instead of
//! being silently dropped. Recent messages always stay word for word.

use crate::context::ConversationHistory;
use crate::llm::{LlmClient, Message};
use crate::tools::ToolDefinition;
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

/// What a summary holds, for both ways of asking for one.
macro_rules! sections {
    () => {
        "Write a factual summary in Markdown with these sections, omitting any that would be empty:
- **Goal**: what the user wants overall, in their terms.
- **Decisions & constraints**: choices made, rules the user gave, things to avoid.
- **Work done**: files created/changed (paths) and what changed; commands run and results.
- **Findings**: facts learned about the codebase (paths, function names, how things work).
- **Open items**: what is unfinished, failing, or next.
Keep exact identifiers, paths, error messages and numbers. Do not invent anything. \
Do not address the user."
    };
}
const SECTIONS: &str = sections!();

pub const SYSTEM_PROMPT: &str = concat!(
    "You compress the early part of a coding session so the assistant can keep working \
     without the full transcript. ",
    sections!(),
    " If a previous summary is given, merge it in: keep what is still \
     true, update what changed."
);

/// Characters of the first kept message quoted to mark where the summary ends.
const QUOTE_CHARS: usize = 120;

/// The request that asks for a summary in place: the conversation exactly as
/// it was last sent (`conversation`, from [`ConversationHistory::get_messages`]),
/// with one more user message asking for the summary. Sent with the same
/// tools, it shares the whole earlier request as a prefix, so a provider with
/// a prompt cache reads it from there instead of billing a fresh transcript.
///
/// `keep_from` is the first message that stays: the summary covers what comes
/// before it. `merge_previous` says the system prompt already holds a summary
/// of an earlier part, which the new one has to take in.
pub fn in_place_request(
    mut conversation: Vec<Message>,
    keep_from: &str,
    merge_previous: bool,
    max_words: usize,
) -> Vec<Message> {
    let one_line = keep_from.split_whitespace().collect::<Vec<_>>().join(" ");
    let quote: String = one_line.chars().take(QUOTE_CHARS).collect();
    let more = if quote.len() < one_line.len() {
        "…"
    } else {
        ""
    };
    let merge = if merge_previous {
        " The summary at the end of the system prompt covers an even earlier part: \
         merge it in, keeping what is still true and updating what changed."
    } else {
        ""
    };
    conversation.push(Message::user(format!(
        "The conversation above is about to be shortened to fit the context window. \
         Everything before the user message that begins \"{quote}{more}\" will be replaced \
         by a summary you write now; that message and everything after it stay as they \
         are.{merge}\n\n{SECTIONS}\n\nDo not call any tools. Reply with the summary only, \
         in at most {max_words} words."
    )));
    conversation
}

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
/// The summary is asked for in place first ([`in_place_request`]): the
/// conversation as it is, with `tools`, plus one message asking for the
/// summary, so a provider's prompt cache serves everything but that message.
/// If that fails, or the model answers with tool calls or nothing, it is asked
/// again the older way, from a plain transcript of the part to fold
/// ([`request`]), which is smaller but all new input.
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
    tools: &[ToolDefinition],
    cancel: Option<&AtomicBool>,
) -> Result<Option<Compacted>, String> {
    let (cut, in_place, transcript, previous, tokens_before) = {
        let conv = conversation.lock().await;
        let tokens_before = conv.estimate_tokens();
        if (tokens_before as f64) < budget as f64 * TRIGGER_RATIO {
            return Ok(None);
        }
        let Some(cut) = conv.compaction_point((budget as f64 * KEEP_RECENT_RATIO) as usize) else {
            return Ok(None);
        };
        let keep_from = conv
            .get_conversation_messages()
            .get(cut)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let limits = summary_limits(budget);
        let mut in_place = in_place_request(
            conv.get_messages(),
            &keep_from,
            conv.summary().is_some(),
            limits.max_words,
        );
        // Asked in place, the whole conversation goes out again. Past the
        // budget it might not fit the model's window (and a local server can
        // cut the start silently), so ask from the transcript instead.
        let ask = in_place.last().map_or(0, |m| m.content.len().div_ceil(3));
        if tokens_before + ask > budget {
            in_place.clear();
        }
        (
            cut,
            in_place,
            conv.transcript(cut),
            conv.summary().map(str::to_string),
            tokens_before,
        )
    };

    let limits = summary_limits(budget);
    let first = if in_place.is_empty() {
        Err("the conversation is too close to the budget to ask in place".to_string())
    } else {
        ask(client, &in_place, tools, cancel).await?
    };
    let summary = match first {
        Ok(summary) => summary,
        Err(e) => {
            tracing::info!("no summary in place ({e}); asking from a transcript");
            let messages = request(previous.as_deref(), &transcript, limits.max_words);
            ask(client, &messages, &[], cancel).await??
        }
    };
    // Never let the summary itself blow the budget.
    let summary = fit_summary(&summary, limits.max_bytes).to_string();

    let mut conv = conversation.lock().await;
    if conv.len() < cut || conv.transcript(cut) != transcript {
        return Err("conversation changed while summarising".to_string());
    }
    conv.compact(cut, summary);
    Ok(Some(Compacted {
        messages_folded: cut,
        tokens_before,
        tokens_after: conv.estimate_tokens(),
    }))
}

/// One summary request. The outer error is Stop; the inner one a request
/// that failed or gave no summary (an empty reply, or tool calls instead).
async fn ask(
    client: &dyn LlmClient,
    messages: &[Message],
    tools: &[ToolDefinition],
    cancel: Option<&AtomicBool>,
) -> Result<Result<String, String>, String> {
    let reply = tokio::select! {
        reply = client.chat(messages, tools) => reply,
        _ = cancelled(cancel) => return Err(CANCELLED.to_string()),
    };
    Ok(match reply {
        Err(e) => Err(e.to_string()),
        Ok(reply) => {
            let summary = reply.message.content.trim().to_string();
            if reply
                .message
                .tool_calls
                .as_ref()
                .is_some_and(|c| !c.is_empty())
            {
                Err("the model called a tool instead of writing the summary".to_string())
            } else if summary.is_empty() {
                Err("the model returned an empty summary".to_string())
            } else {
                Ok(summary)
            }
        }
    })
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
        let (base, mut bodies) = serve_messages(vec![serde_json::json!({
            "role": "assistant", "content": content, "tool_calls": []
        })])
        .await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if let Some(body) = bodies.recv().await {
                let _ = tx.send(body);
            }
        });
        (base, rx)
    }

    /// Answer one request per message in `replies`, in order, on a real
    /// local socket; each request body is sent on the returned channel.
    async fn serve_messages(
        replies: Vec<serde_json::Value>,
    ) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for message in replies {
                let body = serde_json::json!({
                    "choices": [{"message": message}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1}
                })
                .to_string();
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
                            .find_map(|l| {
                                l.strip_prefix("content-length: ")
                                    .or_else(|| l.strip_prefix("Content-Length: "))
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
            }
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

    /// A budget that [`roomy_conversation`] passes the trigger of, with room
    /// left for the in-place ask (~700 tokens, most of it the sections).
    const ROOMY: usize = 4400;

    /// Twelve alternating turns of ~100 tokens each (~1200 tokens in all).
    async fn long_conversation() -> tokio::sync::Mutex<ConversationHistory> {
        conversation_of(100).await
    }

    /// Twelve alternating turns of ~300 tokens each (~3600 tokens in all).
    async fn roomy_conversation() -> tokio::sync::Mutex<ConversationHistory> {
        conversation_of(300).await
    }

    /// Twelve alternating turns of `tokens` tokens each.
    async fn conversation_of(tokens: usize) -> tokio::sync::Mutex<ConversationHistory> {
        let conversation = tokio::sync::Mutex::new(ConversationHistory::new());
        {
            let mut c = conversation.lock().await;
            for n in 0..12 {
                let text = format!("turn {n:02} {}", "x".repeat(tokens * 3 - 8));
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
        let conversation = roomy_conversation().await;
        let (base, request_rx) = serve_completion("**Goal**: finish turn 11.").await;
        let client = OpenAIClient::new("test-key").with_base_url(base);

        // 3600 tokens is over the 80% trigger, and there is room to ask in place;
        // ~1760 tokens (5 messages, from the user turn 08) stay.
        let done = maybe_compact(&conversation, &client, ROOMY, &[], None)
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

        // The summary was asked for in place: the conversation as it was,
        // then one message naming where the kept part starts.
        let sent: serde_json::Value = serde_json::from_str(&request_rx.await.unwrap()).unwrap();
        let messages = sent["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 13);
        assert!(messages[0]["content"]
            .as_str()
            .unwrap()
            .starts_with("turn 00"));
        assert!(messages[11]["content"]
            .as_str()
            .unwrap()
            .starts_with("turn 11"));
        let ask = messages[12]["content"].as_str().unwrap();
        assert!(ask.contains("about to be shortened"));
        assert!(ask.contains(&format!("begins \"turn 08 {}…\"", "x".repeat(112))));
        assert!(ask.contains("Do not call any tools"));
        assert!(!ask.contains("system prompt covers"));
        // The word limit comes from the budget's summary allowance.
        let words = summary_limits(ROOMY).max_words;
        assert!(ask.contains(&format!("at most {words} words")));
    }

    #[tokio::test]
    async fn the_summary_request_carries_the_agent_tools_so_its_prefix_matches() {
        let conversation = roomy_conversation().await;
        let (base, mut bodies) = serve_messages(vec![serde_json::json!({
            "role": "assistant", "content": "summary", "tool_calls": []
        })])
        .await;
        let client = OpenAIClient::new("k").with_base_url(base);
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];
        maybe_compact(&conversation, &client, ROOMY, &tools, None)
            .await
            .unwrap()
            .expect("compaction should run");
        let sent: serde_json::Value = serde_json::from_str(&bodies.recv().await.unwrap()).unwrap();
        assert_eq!(sent["tools"][0]["function"]["name"], "read_file");
    }

    #[tokio::test]
    async fn falls_back_to_a_transcript_when_the_model_calls_a_tool_instead() {
        let conversation = roomy_conversation().await;
        conversation.lock().await.set_summary("earlier work");
        let (base, mut bodies) = serve_messages(vec![
            serde_json::json!({
                "role": "assistant", "content": "",
                "tool_calls": [{"id": "c", "type": "function",
                    "function": {"name": "read_file", "arguments": "{}"}}]
            }),
            serde_json::json!({"role": "assistant", "content": "  from the transcript  "}),
        ])
        .await;
        let client = OpenAIClient::new("k").with_base_url(base);
        maybe_compact(&conversation, &client, ROOMY, &[], None)
            .await
            .unwrap()
            .expect("compaction should run");

        let first = bodies.recv().await.unwrap();
        assert!(first.contains("system prompt covers an even earlier part"));
        let second = bodies.recv().await.unwrap();
        assert!(second.contains("Transcript to fold in"));
        assert!(second.contains("<previous_summary>\\nearlier work"));
        assert!(second.contains("turn 07") && !second.contains("turn 08"));
        assert_eq!(
            conversation.lock().await.summary(),
            Some("from the transcript")
        );
    }

    #[tokio::test]
    async fn asks_from_a_transcript_when_asking_in_place_would_pass_the_budget() {
        // ~1200 tokens against a 1300 budget: over the trigger, and the
        // conversation plus the in-place ask would not fit.
        let conversation = long_conversation().await;
        let (base, mut bodies) = serve_messages(vec![
            serde_json::json!({"role": "assistant", "content": "from the transcript"}),
        ])
        .await;
        let client = OpenAIClient::new("k").with_base_url(base);
        maybe_compact(&conversation, &client, 1300, &[], None)
            .await
            .unwrap()
            .expect("compaction should run");
        let only = bodies.recv().await.unwrap();
        assert!(only.contains("Transcript to fold in"));
        assert!(!only.contains("about to be shortened"));
        assert_eq!(
            conversation.lock().await.summary(),
            Some("from the transcript")
        );
    }

    #[tokio::test]
    async fn falls_back_to_a_transcript_when_the_in_place_reply_is_empty() {
        let conversation = roomy_conversation().await;
        let (base, mut bodies) = serve_messages(vec![
            serde_json::json!({"role": "assistant", "content": " "}),
            serde_json::json!({"role": "assistant", "content": "ok"}),
        ])
        .await;
        let client = OpenAIClient::new("k").with_base_url(base);
        maybe_compact(&conversation, &client, ROOMY, &[], None)
            .await
            .unwrap()
            .expect("compaction should run");
        assert!(bodies
            .recv()
            .await
            .unwrap()
            .contains("about to be shortened"));
        assert!(bodies
            .recv()
            .await
            .unwrap()
            .contains("Transcript to fold in"));
        assert_eq!(conversation.lock().await.summary(), Some("ok"));
    }

    #[tokio::test]
    async fn does_nothing_under_the_trigger() {
        let conversation = tokio::sync::Mutex::new(ConversationHistory::new());
        conversation.lock().await.add_user_message("hi");
        // No server: a request would fail, so this proves none was made.
        let client = OpenAIClient::new("k").with_base_url("http://127.0.0.1:9");
        assert_eq!(
            maybe_compact(&conversation, &client, 1000, &[], None).await,
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
            maybe_compact(&conversation, &client, 1000, &[], Some(&cancel)),
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
