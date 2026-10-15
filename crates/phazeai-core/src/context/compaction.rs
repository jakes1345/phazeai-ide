//! Context compaction: when a conversation outgrows its token budget, the
//! oldest part is replaced by a summary written by the same model, instead of
//! being silently dropped. Recent messages always stay word for word.

use crate::context::ConversationHistory;
use crate::llm::{LlmClient, Message};

/// Compact once the conversation passes this share of the budget…
pub const TRIGGER_RATIO: f64 = 0.8;
/// …keeping about this share of the budget as verbatim recent messages.
pub const KEEP_RECENT_RATIO: f64 = 0.4;
/// The summary may use at most this share of the budget.
pub const SUMMARY_RATIO: f64 = 0.15;

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
pub async fn maybe_compact(
    conversation: &tokio::sync::Mutex<ConversationHistory>,
    client: &dyn LlmClient,
    budget: usize,
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

    // ~0.75 words per token.
    let max_words = ((budget as f64 * SUMMARY_RATIO) * 0.75).max(150.0) as usize;
    let reply = client
        .chat(&request(previous.as_deref(), &transcript, max_words), &[])
        .await
        .map_err(|e| e.to_string())?;
    let summary = reply.message.content.trim().to_string();
    if summary.is_empty() {
        return Err("the model returned an empty summary".to_string());
    }
    // Never let the summary itself blow the budget.
    let max_bytes = (budget as f64 * SUMMARY_RATIO * 3.0) as usize;
    let summary = crate::text::truncate_bytes(&summary, max_bytes).to_string();

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

    #[tokio::test]
    async fn folds_the_oldest_messages_into_a_model_written_summary() {
        let conversation = tokio::sync::Mutex::new(ConversationHistory::new());
        {
            let mut c = conversation.lock().await;
            for n in 0..12 {
                // 300 chars ≈ 100 tokens each → 1200 tokens total.
                let text = format!("turn {n:02} {}", "x".repeat(292));
                if n % 2 == 0 {
                    c.add_user_message(text);
                } else {
                    c.add_assistant_message(text);
                }
            }
        }
        let (base, request_rx) = serve_completion("**Goal**: finish turn 11.").await;
        let client = OpenAIClient::new("test-key").with_base_url(base);

        // Budget 1000: 1200 is over the 80% trigger; ~400 tokens (4 messages) stay.
        let done = maybe_compact(&conversation, &client, 1000)
            .await
            .unwrap()
            .expect("compaction should run");
        assert_eq!(done.messages_folded, 8);
        assert!(done.tokens_after < done.tokens_before);

        let c = conversation.lock().await;
        assert_eq!(c.len(), 4);
        assert_eq!(c.summary(), Some("**Goal**: finish turn 11."));
        assert!(c.get_conversation_messages()[0]
            .content
            .starts_with("turn 08"));

        // The model saw the folded transcript and our instructions.
        let sent = request_rx.await.unwrap();
        assert!(sent.contains("turn 07"));
        assert!(!sent.contains("turn 08"));
        assert!(sent.contains("Transcript to fold in"));
    }

    #[tokio::test]
    async fn does_nothing_under_the_trigger() {
        let conversation = tokio::sync::Mutex::new(ConversationHistory::new());
        conversation.lock().await.add_user_message("hi");
        // No server: a request would fail, so this proves none was made.
        let client = OpenAIClient::new("k").with_base_url("http://127.0.0.1:9");
        assert_eq!(maybe_compact(&conversation, &client, 1000).await, Ok(None));
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
}
