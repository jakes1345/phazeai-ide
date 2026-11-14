//! `/btw`: a quick side question about the current session, answered by
//! PhazeAI Local (the built-in model on this machine) so it costs nothing per
//! question.
//!
//! The side question sees a snapshot of the conversation and the earlier side
//! questions, but it gets no tools and its answer never enters the
//! conversation: the main agent keeps working and never sees it. It never
//! falls back to a paid provider; when the local model isn't set up it says so.
//!
//! The IDE chat and the CLI build a [`SessionLine`] list from what they show,
//! call [`prompt`] and then [`ask`].

use crate::config::{LlmProvider, Settings};
use crate::llm::text_only_stream;

/// Who a line of the session snapshot is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    User,
    Assistant,
    /// A tool call and its outcome, as the front end summarises it.
    Tool,
}

/// One line of the session snapshot a side question sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLine {
    pub who: Who,
    pub text: String,
}

impl SessionLine {
    pub fn new(who: Who, text: impl Into<String>) -> Self {
        Self {
            who,
            text: text.into(),
        }
    }
}

/// One earlier side question and its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    pub question: String,
    pub answer: String,
}

/// Earlier side questions a new one sees (the newest ones).
pub const MAX_EARLIER: usize = 20;
/// Longest a single session line may be in the prompt; longer ones keep their
/// start and end.
const MAX_LINE_CHARS: usize = 4_000;
/// Longest an earlier question or answer may be in the prompt.
const MAX_EARLIER_CHARS: usize = 1_500;
/// Tokens kept free in the local model's window for the instructions and
/// the answer.
const RESERVED_TOKENS: usize = 1_536;
/// Conservative characters per token for sizing the snapshot.
const CHARS_PER_TOKEN: usize = 3;
/// The snapshot never shrinks below this, even on a tiny window.
const MIN_BUDGET_CHARS: usize = 2_000;

const INSTRUCTIONS: &str = "You're answering a quick side question (\"/btw\") from a developer \
during a coding session in PhazeAI. The main AI assistant may still be working on its own task; \
your answer is shown beside it and is not added to that conversation.\n\
Answer from the session below. You can't read files, run commands or search, so if the answer \
needs that, say so in one sentence. Be brief and direct.";

/// How many characters of session text fit the built-in model's window.
pub fn budget_chars(settings: &Settings) -> usize {
    let ctx = settings.local_ai.effective_ctx_size() as usize;
    (ctx.saturating_sub(RESERVED_TOKENS) * CHARS_PER_TOKEN).max(MIN_BUDGET_CHARS)
}

/// The prompt for a side question: instructions, as much of `session` as fits
/// `budget_chars` (newest kept, older dropped with a marker), the newest
/// [`MAX_EARLIER`] of `earlier`, then the question.
pub fn prompt(
    session: &[SessionLine],
    earlier: &[Exchange],
    question: &str,
    budget_chars: usize,
) -> String {
    let earlier_block: String = earlier
        .iter()
        .rev()
        .take(MAX_EARLIER)
        .rev()
        .map(|e| {
            format!(
                "Q: {}\nA: {}\n",
                clip(e.question.trim(), MAX_EARLIER_CHARS),
                clip(e.answer.trim(), MAX_EARLIER_CHARS)
            )
        })
        .collect();

    // Newest session lines first, until the budget (less what the earlier
    // side questions already use) runs out.
    let mut left = budget_chars.saturating_sub(earlier_block.len());
    let mut kept: Vec<String> = Vec::new();
    for line in session.iter().rev() {
        let text = line.text.trim();
        if text.is_empty() {
            continue;
        }
        let label = match line.who {
            Who::User => "User",
            Who::Assistant => "Assistant",
            Who::Tool => "Tool",
        };
        let entry = format!("{label}: {}", clip(text, MAX_LINE_CHARS));
        if entry.len() > left {
            break;
        }
        left -= entry.len();
        kept.push(entry);
    }
    let omitted = kept.len() < session.iter().filter(|l| !l.text.trim().is_empty()).count();
    kept.reverse();

    let mut out = format!("{INSTRUCTIONS}\n\n<session>\n");
    if omitted {
        out.push_str("[earlier messages left out]\n");
    }
    if kept.is_empty() {
        out.push_str("(nothing yet)\n");
    }
    for entry in &kept {
        out.push_str(entry);
        out.push('\n');
    }
    out.push_str("</session>\n");
    if !earlier_block.is_empty() {
        out.push_str("\n<earlier_side_questions>\n");
        out.push_str(&earlier_block);
        out.push_str("</earlier_side_questions>\n");
    }
    out.push_str(&format!("\nSide question: {}", question.trim()));
    out
}

/// What to tell someone whose side question can't run because PhazeAI Local
/// isn't set up.
pub const SETUP_HINT: &str = "/btw answers with PhazeAI Local, the free model on this computer, \
and it isn't ready: install llama.cpp and download a model in Settings › Local AI (or run \
`phazeai setup`). /btw never uses a paid provider.";

/// Ask the built-in local model, handing each piece of the answer to
/// `on_text` as it arrives. Starts PhazeAI Local if it isn't running (loading
/// a model can take a while the first time). Goes through the privacy policy
/// and audit log like every request; never uses a paid provider.
pub async fn ask(
    settings: &Settings,
    prompt: &str,
    on_text: impl FnMut(&str),
) -> Result<String, String> {
    crate::local_ai::LOCAL_AI
        .ensure_running(&settings.local_ai)
        .await
        .map_err(|e| format!("{SETUP_HINT}\n({e})"))?;
    let client = local_client(settings).map_err(|e| e.to_string())?;
    let reply = text_only_stream(client.as_ref(), prompt, on_text).await;
    match reply.error {
        Some(e) if reply.text.trim().is_empty() => Err(e),
        // Keep what arrived before a failure, and say it was cut short.
        Some(e) => Ok(format!("{}\n\n(cut short: {e})", reply.text.trim_end())),
        None => Ok(reply.text.trim().to_string()),
    }
}

/// A client for the built-in model: `settings` with the provider set to
/// PhazeAI Local and no `[model_routes]` (they route the agent's own calls),
/// built the way every client is, so the privacy guard wraps it.
fn local_client(
    settings: &Settings,
) -> Result<Box<dyn crate::llm::LlmClient>, crate::error::PhazeError> {
    let mut local = settings.clone();
    local.set_provider(LlmProvider::BuiltIn);
    local.model_routes.clear();
    local.build_llm_client()
}

/// `text` cut to about `max` characters: its start and end with a marker
/// between, at character boundaries.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let half = max / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{} […] {}", &text[..head], &text[tail..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<SessionLine> {
        vec![
            SessionLine::new(Who::User, "fix the bug in stock_value"),
            SessionLine::new(Who::Tool, "read_file src/lib.rs: ok"),
            SessionLine::new(
                Who::Assistant,
                "It adds price + qty instead of multiplying.",
            ),
        ]
    }

    #[test]
    fn prompt_holds_the_session_in_order_then_the_question() {
        let p = prompt(&lines(), &[], "  what's qty?  ", 10_000);
        assert!(p.starts_with(INSTRUCTIONS));
        let user = p.find("User: fix the bug").unwrap();
        let tool = p.find("Tool: read_file").unwrap();
        let asst = p.find("Assistant: It adds").unwrap();
        assert!(user < tool && tool < asst);
        assert!(p.ends_with("Side question: what's qty?"));
        assert!(!p.contains("left out"));
        assert!(!p.contains("earlier_side_questions"));
    }

    #[test]
    fn a_small_budget_keeps_the_newest_lines() {
        let p = prompt(&lines(), &[], "q", 60);
        assert!(p.contains("Assistant: It adds"), "{p}");
        assert!(!p.contains("User: fix the bug"), "{p}");
        assert!(p.contains("[earlier messages left out]"));
        // Nothing fits at all.
        let p = prompt(&lines(), &[], "q", 5);
        assert!(p.contains("(nothing yet)") && p.contains("left out"));
    }

    #[test]
    fn earlier_side_questions_are_included_newest_twenty() {
        let earlier: Vec<Exchange> = (0..25)
            .map(|i| Exchange {
                question: format!("q{i}"),
                answer: format!("a{i}"),
            })
            .collect();
        let p = prompt(&lines(), &earlier, "next", 10_000);
        assert!(!p.contains("Q: q4\n") && p.contains("Q: q5\nA: a5"));
        assert!(p.contains("Q: q24\nA: a24"));
        let block = p.find("<earlier_side_questions>").unwrap();
        assert!(block > p.find("</session>").unwrap());
        assert!(block < p.find("Side question: next").unwrap());
    }

    #[test]
    fn long_lines_keep_start_and_end() {
        let long = format!("START{}END", "é".repeat(5_000));
        let p = prompt(&[SessionLine::new(Who::Tool, long)], &[], "q", 50_000);
        assert!(p.contains("Tool: START") && p.contains("END\n"));
        assert!(p.contains(" […] "));
        assert!(p.len() < 6_000);
    }

    #[test]
    fn budget_follows_the_local_window() {
        let mut s = Settings::default();
        s.local_ai.ctx_size = Some(16_384);
        assert_eq!(
            budget_chars(&s),
            (16_384 - RESERVED_TOKENS) * CHARS_PER_TOKEN
        );
        s.local_ai.ctx_size = Some(512);
        assert_eq!(budget_chars(&s), MIN_BUDGET_CHARS);
    }

    #[test]
    fn the_client_is_always_the_built_in_provider() {
        let mut s = Settings::default();
        s.set_provider(LlmProvider::Claude);
        let mut local = s.clone();
        local.set_provider(LlmProvider::BuiltIn);
        assert_eq!(
            local.llm.provider.to_provider_id(),
            crate::llm::ProviderId::BuiltIn
        );
        // Building it with no server running is an error, never another provider.
        if crate::local_ai::LOCAL_AI.base_url().is_none() {
            let err = local_client(&s).err().expect("no server, no client");
            assert!(err.to_string().contains("PhazeAI Local"), "{err}");
        }
    }
}
