use crate::llm::{Message, Role};
use std::collections::VecDeque;

pub struct ConversationHistory {
    messages: VecDeque<Message>,
    max_messages: usize,
    system_prompt: Option<String>,
    /// Summary of messages that were compacted away (see [`Self::compact`]).
    /// Sent to the model as part of the system prompt.
    summary: Option<String>,
    /// Messages removed by [`Self::compact`] since this history was created
    /// or last cleared.
    folded: usize,
}

impl ConversationHistory {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            max_messages: 100,
            system_prompt: None,
            summary: None,
            folded: 0,
        }
    }

    pub fn with_max_messages(mut self, max: usize) -> Self {
        self.max_messages = max;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Start from an earlier compaction: `summary` stands for messages that
    /// are not in this history (a UI that rebuilds the history every turn
    /// carries the summary over this way).
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.set_summary(summary);
        self
    }

    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = Some(prompt.into());
    }

    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// Set the compaction summary; blank text clears it.
    pub fn set_summary(&mut self, summary: impl Into<String>) {
        let summary = summary.into();
        let trimmed = summary.trim();
        self.summary = (!trimmed.is_empty()).then(|| trimmed.to_string());
    }

    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push_back(Message::user(content));
        self.trim_if_needed();
    }

    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push_back(Message::assistant(content));
        self.trim_if_needed();
    }

    pub fn add_tool_result(&mut self, tool_call_id: impl Into<String>, result: impl Into<String>) {
        let mut result_str = result.into();

        // Truncate huge tool results to save context
        const MAX_TOOL_RESULT_LEN: usize = 12000;
        if result_str.len() > MAX_TOOL_RESULT_LEN {
            result_str = format!(
                "{}... [Truncated {} bytes]",
                crate::text::truncate_bytes(&result_str, MAX_TOOL_RESULT_LEN),
                result_str.len() - MAX_TOOL_RESULT_LEN
            );
        }

        self.messages
            .push_back(Message::tool_result(tool_call_id, result_str));
        self.trim_if_needed();
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push_back(message);
        self.trim_if_needed();
    }

    /// Get all messages including system prompt as a system message.
    /// A compaction summary, if any, is appended to the system prompt so it
    /// works with providers that accept a single system message.
    pub fn get_messages(&self) -> Vec<Message> {
        let mut messages = Vec::new();
        match (&self.system_prompt, &self.summary) {
            (Some(system), Some(summary)) => {
                messages.push(Message::system(format!(
                    "{system}\n\n{SUMMARY_HEADING}\n{summary}"
                )));
            }
            (Some(system), None) => messages.push(Message::system(system)),
            (None, Some(summary)) => {
                messages.push(Message::system(format!("{SUMMARY_HEADING}\n{summary}")))
            }
            (None, None) => {}
        }
        messages.extend(self.messages.iter().cloned());
        messages
    }

    /// Get only conversation messages (no system prompt).
    pub fn get_conversation_messages(&self) -> Vec<Message> {
        self.messages.iter().cloned().collect()
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.summary = None;
        self.folded = 0;
    }

    /// The summary of compacted messages, if any compaction has happened.
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    /// How many messages [`Self::compact`] has folded into the summary since
    /// this history was created or last cleared.
    pub fn folded_count(&self) -> usize {
        self.folded
    }

    /// Where to cut so that roughly `keep_recent_tokens` of the newest
    /// messages stay verbatim and everything before the cut can be
    /// summarised. The kept part always starts with a plain user message:
    /// providers reject a conversation that opens with an assistant turn or a
    /// tool result, and starting at a user turn also keeps every tool call
    /// with its results. Returns `None` when there's nothing worth compacting.
    pub fn compaction_point(&self, keep_recent_tokens: usize) -> Option<usize> {
        let len = self.messages.len();
        let mut kept = 0usize;
        let mut cut = len;
        while cut > 0 {
            let cost = message_tokens(&self.messages[cut - 1]);
            if kept + cost > keep_recent_tokens {
                break;
            }
            kept += cost;
            cut -= 1;
        }
        // Keep from the first user turn at or after the budget cut. When the
        // budget would keep no whole turn, keep the newest one verbatim.
        let cut = (cut..len)
            .find(|&i| self.starts_turn(i))
            .or_else(|| (0..len).rev().find(|&i| self.starts_turn(i)))?;
        (cut >= MIN_MESSAGES_TO_COMPACT).then_some(cut)
    }

    /// A plain user message (not a tool result): the only message a
    /// conversation sent to a provider may start with.
    fn starts_turn(&self, i: usize) -> bool {
        let m = &self.messages[i];
        m.role == Role::User && m.tool_call_id.is_none()
    }

    /// A plain-text transcript of the first `upto` messages, for the model
    /// that writes the summary. Long tool output is shortened.
    pub fn transcript(&self, upto: usize) -> String {
        let mut out = String::new();
        for msg in self.messages.iter().take(upto) {
            let who = match (&msg.role, &msg.tool_call_id) {
                (_, Some(_)) => "Tool result",
                (crate::llm::Role::User, None) => "User",
                (crate::llm::Role::Assistant, None) => "Assistant",
                (crate::llm::Role::System, None) => "System",
            };
            let body = crate::text::truncate_bytes(&msg.content, TRANSCRIPT_MESSAGE_LIMIT);
            out.push_str(&format!("[{who}]\n{body}"));
            if body.len() < msg.content.len() {
                out.push_str(&format!(
                    " …[{} more bytes]",
                    msg.content.len() - body.len()
                ));
            }
            out.push('\n');
            if let Some(calls) = &msg.tool_calls {
                for call in calls {
                    let args = crate::text::truncate_bytes(&call.function.arguments, 400);
                    out.push_str(&format!("[Tool call] {}({args})\n", call.function.name));
                }
            }
            out.push('\n');
        }
        out
    }

    /// Replace the first `upto` messages with `summary`. The summary replaces
    /// any earlier one (the caller folds the old summary into the new text).
    pub fn compact(&mut self, upto: usize, summary: impl Into<String>) {
        let upto = upto.min(self.messages.len());
        self.messages.drain(..upto);
        self.folded += upto;
        self.set_summary(summary);
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn last_message(&self) -> Option<&Message> {
        self.messages.back()
    }

    fn trim_if_needed(&mut self) {
        // Keep message count within limits
        while self.messages.len() > self.max_messages {
            self.drop_oldest_message_group();
        }
    }

    /// Trim conversation to stay within a token budget while preserving the system prompt and recent context.
    pub fn trim_to_token_budget(&mut self, max_tokens: usize) {
        while self.estimate_tokens() > max_tokens && !self.messages.is_empty() {
            self.drop_oldest_message_group();
        }
    }

    /// Drop the oldest turn: the front message, the results of any tool calls
    /// it made, and everything else before the next plain user message, so
    /// what remains still opens with a user turn (providers reject a
    /// conversation that starts with an assistant turn or a tool result).
    fn drop_oldest_message_group(&mut self) {
        let Some(front) = self.messages.pop_front() else {
            return;
        };

        // Keep tool-call sequences valid by evicting both sides together.
        // If we drop an assistant tool-call message but keep its tool results,
        // some providers reject the resulting malformed conversation.
        if let Some(tool_calls) = front.tool_calls {
            let evicted_ids: std::collections::HashSet<String> =
                tool_calls.into_iter().map(|tc| tc.id).collect();
            self.messages.retain(|msg| {
                msg.tool_call_id
                    .as_ref()
                    .map(|id| !evicted_ids.contains(id))
                    .unwrap_or(true)
            });
        }

        while !self.messages.is_empty() && !self.starts_turn(0) {
            self.messages.pop_front();
        }
    }

    pub fn estimate_tokens(&self) -> usize {
        let mut total = 0;

        // System prompt tokens — use chars/3 for code-heavy content
        if let Some(ref system) = self.system_prompt {
            total += system.len().div_ceil(3);
        }

        if let Some(ref summary) = self.summary {
            total += summary.len().div_ceil(3);
        }

        // Message tokens — use chars/3 (code is denser than prose, ~3 chars/token)
        total += self.messages.iter().map(message_tokens).sum::<usize>();

        total
    }
}

/// Heading that introduces the compaction summary in the system prompt.
pub const SUMMARY_HEADING: &str = "## Summary of the earlier part of this conversation";

/// Compacting fewer messages than this isn't worth an extra model call.
const MIN_MESSAGES_TO_COMPACT: usize = 4;

/// Longest single message copied into a compaction transcript.
const TRANSCRIPT_MESSAGE_LIMIT: usize = 4000;

/// Rough token cost of one message (~3 characters per token for code-heavy
/// text), including tool-call arguments.
fn message_tokens(m: &Message) -> usize {
    let calls: usize = m
        .tool_calls
        .iter()
        .flatten()
        .map(|c| c.function.name.len() + c.function.arguments.len())
        .sum();
    (m.content.len() + calls).div_ceil(3)
}

impl Default for ConversationHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{FunctionCall, ToolCall};

    #[test]
    fn trim_removes_tool_results_with_evicted_tool_call() {
        let mut history = ConversationHistory::new().with_max_messages(2);
        let tool_call = ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: "{\"path\":\"Cargo.toml\"}".to_string(),
            },
        };

        history.add_message(Message::assistant_with_tools("tooling", vec![tool_call]));
        history.add_tool_result("call_1", "file contents");
        history.add_user_message("next turn");

        let conversation = history.get_conversation_messages();
        assert_eq!(conversation.len(), 1);
        assert_eq!(conversation[0].content, "next turn");
    }

    fn msg(role_user: bool, n: usize) -> Message {
        // 30 chars → 10 estimated tokens each.
        let text = format!("message {n:02} {}", "x".repeat(19));
        if role_user {
            Message::user(text)
        } else {
            Message::assistant(text)
        }
    }

    #[test]
    fn compaction_point_keeps_the_recent_budget_verbatim() {
        let mut h = ConversationHistory::new();
        for n in 0..10 {
            h.add_message(msg(n % 2 == 0, n));
        }
        // 25 tokens of recent context fit two 10-token messages.
        assert_eq!(h.compaction_point(25), Some(8));
        // Too little to compact.
        let mut small = ConversationHistory::new();
        for n in 0..3 {
            small.add_message(msg(true, n));
        }
        assert_eq!(small.compaction_point(1000), None);
        // The newest turn always stays, even with no recent budget.
        assert_eq!(h.compaction_point(0), Some(8));
        // Nothing to compact: no underflow, no panic.
        assert_eq!(ConversationHistory::new().compaction_point(0), None);
        assert_eq!(ConversationHistory::new().compaction_point(1000), None);
    }

    #[test]
    fn compaction_point_keeps_from_a_plain_user_message() {
        // user/assistant alternate; the budget cut lands on an assistant turn.
        let mut h = ConversationHistory::new();
        for n in 0..10 {
            h.add_message(msg(n % 2 == 0, n));
        }
        // 15 tokens keep only message 9 (assistant); the cut moves back to 8.
        assert_eq!(h.compaction_point(15), Some(8));
        // 35 tokens keep 7, 8, 9: 7 is an assistant turn, so keep from 8.
        assert_eq!(h.compaction_point(35), Some(8));
        // 45 tokens keep 6..=9 and 6 is a user turn.
        assert_eq!(h.compaction_point(45), Some(6));

        // The first kept message is never a tool result either.
        let mut h = ConversationHistory::new();
        h.add_message(msg(true, 0));
        h.add_message(msg(false, 1));
        h.add_message(msg(true, 2));
        h.add_message(msg(false, 3));
        h.add_message(msg(true, 4));
        h.add_message(Message::assistant_with_tools(
            "x".repeat(30),
            vec![ToolCall {
                id: "a".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "bash".to_string(),
                    arguments: "{}".to_string(),
                },
            }],
        ));
        h.add_tool_result("a", "y".repeat(30));
        h.add_message(msg(false, 7));
        // Budget for the tool result and the reply only: back up to message 4.
        assert_eq!(h.compaction_point(20), Some(4));
        // No budget at all: still the newest user turn.
        assert_eq!(h.compaction_point(0), Some(4));

        // A history with no plain user message after the cut, and none before
        // it either, cannot be compacted.
        let mut h = ConversationHistory::new();
        for n in 0..6 {
            h.add_message(msg(false, n));
        }
        assert_eq!(h.compaction_point(10), None);
    }

    #[test]
    fn trimming_never_leaves_an_assistant_turn_or_tool_result_first() {
        let call = |id: &str| ToolCall {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "bash".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let first_is_plain_user = |h: &ConversationHistory| {
            h.get_conversation_messages().first().is_none_or(|m| {
                m.role == Role::User && m.tool_call_id.is_none() && m.tool_calls.is_none()
            })
        };

        // Token-budget trim: dropping the first user message takes its
        // whole turn (assistant reply, tool call, tool results) with it.
        let mut h = ConversationHistory::new();
        h.add_message(msg(true, 0));
        h.add_message(Message::assistant_with_tools("", vec![call("c1")]));
        h.add_tool_result("c1", "r".repeat(30));
        h.add_message(msg(false, 3));
        h.add_message(msg(true, 4));
        h.add_message(msg(false, 5));
        // 25 tokens fit the last turn (20) but not the assistant reply before it.
        h.trim_to_token_budget(25);
        let kept = h.get_conversation_messages();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].content, msg(true, 4).content);
        assert!(first_is_plain_user(&h));

        // Message-count trim, one message at a time.
        let mut h = ConversationHistory::new().with_max_messages(3);
        h.add_message(msg(true, 0));
        h.add_message(msg(false, 1));
        h.add_message(msg(true, 2));
        assert!(first_is_plain_user(&h));
        h.add_message(msg(false, 3)); // over the cap: drops 0 and its reply 1
        assert_eq!(h.len(), 2);
        assert!(first_is_plain_user(&h));
        assert_eq!(
            h.get_conversation_messages()[0].content,
            msg(true, 2).content
        );

        // Trimming a history that opens with an assistant message skips to the
        // first user turn.
        let mut h = ConversationHistory::new();
        h.add_message(msg(false, 0));
        h.add_message(msg(false, 1));
        h.add_message(msg(true, 2));
        h.add_message(msg(false, 3));
        h.trim_to_token_budget(25);
        assert_eq!(h.len(), 2);
        assert!(first_is_plain_user(&h));

        // Over budget with every message after the first being a tool
        // exchange: nothing valid can be kept, so nothing is.
        let mut h = ConversationHistory::new();
        h.add_message(msg(true, 0));
        h.add_message(Message::assistant_with_tools("", vec![call("c2")]));
        h.add_tool_result("c2", "r".repeat(300));
        h.trim_to_token_budget(50);
        assert!(h.is_empty());
    }

    #[test]
    fn summary_can_be_seeded_and_folded_messages_are_counted() {
        let mut h = ConversationHistory::new().with_summary("  carried over  ");
        assert_eq!(h.summary(), Some("carried over"));
        assert_eq!(h.folded_count(), 0);
        for n in 0..6 {
            h.add_message(msg(n % 2 == 0, n));
        }
        // The seeded summary is sent with the messages and counts as tokens.
        let messages = h.get_messages();
        assert_eq!(messages.len(), 7);
        assert_eq!(
            messages[0].content,
            format!("{SUMMARY_HEADING}\ncarried over")
        );
        let without = ConversationHistory::new();
        assert!(h.estimate_tokens() > without.estimate_tokens() + 6 * 10);

        h.compact(2, "merged");
        assert_eq!(h.folded_count(), 2);
        h.compact(2, "merged again");
        assert_eq!(h.folded_count(), 4);
        assert_eq!(h.summary(), Some("merged again"));
        assert_eq!(h.len(), 2);

        h.set_summary("   ");
        assert_eq!(h.summary(), None);
        h.set_summary("back");
        assert_eq!(h.summary(), Some("back"));
        h.clear();
        assert_eq!(h.folded_count(), 0);
        assert_eq!(h.summary(), None);
    }

    #[test]
    fn compaction_point_never_splits_a_tool_call_from_its_results() {
        let mut h = ConversationHistory::new();
        h.add_message(msg(true, 0));
        let call = |id: &str| ToolCall {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
        };
        h.add_message(Message::assistant_with_tools(
            "x".repeat(30),
            vec![call("a"), call("b")],
        ));
        h.add_tool_result("a", "y".repeat(30));
        h.add_tool_result("b", "z".repeat(30));
        h.add_message(msg(true, 4));
        h.add_message(msg(false, 5));
        // 35 tokens would cut between the two tool results; the cut moves
        // forward to the next user message instead.
        assert_eq!(h.compaction_point(35), Some(4));
    }

    #[test]
    fn compact_replaces_the_prefix_with_a_summary_in_the_system_message() {
        let mut h = ConversationHistory::new().with_system_prompt("SYS");
        for n in 0..6 {
            h.add_message(msg(n % 2 == 0, n));
        }
        let before = h.estimate_tokens();
        h.compact(4, "  Goal: finish the thing.  ");
        assert_eq!(h.len(), 2);
        assert_eq!(h.summary(), Some("Goal: finish the thing."));
        let messages = h.get_messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages[0].content,
            format!("SYS\n\n{SUMMARY_HEADING}\nGoal: finish the thing.")
        );
        assert_eq!(messages[1].content, msg(true, 4).content);
        assert!(h.estimate_tokens() < before);
        // The summary counts toward the budget.
        let without: usize = h
            .get_conversation_messages()
            .iter()
            .map(|m| m.content.len().div_ceil(3))
            .sum();
        assert!(h.estimate_tokens() > without + 1);
        h.clear();
        assert_eq!(h.summary(), None);
    }

    #[test]
    fn transcript_labels_roles_and_shortens_long_output() {
        let mut h = ConversationHistory::new();
        h.add_user_message("hello");
        h.add_message(Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "c1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "bash".to_string(),
                    arguments: "{\"command\":\"ls\"}".to_string(),
                },
            }],
        ));
        h.add_tool_result("c1", "q".repeat(5000));
        let t = h.transcript(3);
        assert!(t.starts_with("[User]\nhello\n"));
        assert!(t.contains("[Tool call] bash({\"command\":\"ls\"})"));
        assert!(t.contains("[Tool result]\n"));
        assert!(t.contains("…[1000 more bytes]"));
        assert!(t.len() < 5000);
    }
}
