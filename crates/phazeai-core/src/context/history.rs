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
            // No count cap by default: the token budget bounds the history,
            // and dropping a message per new one would change the start of
            // every request, so no prefix cache could serve it.
            max_messages: usize::MAX,
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

    /// A user message with pictures for a vision model to look at.
    pub fn add_user_message_with_images(
        &mut self,
        content: impl Into<String>,
        images: Vec<crate::llm::ImageInput>,
    ) {
        self.messages
            .push_back(Message::user_with_images(content, images));
        self.trim_if_needed();
    }

    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push_back(Message::assistant(content));
        self.trim_if_needed();
    }

    pub fn add_tool_result(&mut self, tool_call_id: impl Into<String>, result: impl Into<String>) {
        let result_str = stored_tool_result(result.into());
        self.messages
            .push_back(Message::tool_result(tool_call_id, result_str));
        self.trim_if_needed();
    }

    /// The id of an earlier call of one of `tools` whose result, as it is
    /// still stored in this conversation, is exactly what `result` would be
    /// stored as; the most recent such call if there are several.
    ///
    /// Only a result the model can still read counts: one that compaction
    /// folded into the summary is gone from the messages, and one that
    /// [`Self::trim_to_token_budget`] shortened no longer matches. So when
    /// this returns an id, sending `result` again would repeat, word for
    /// word, text that is already in the request.
    pub fn repeat_of(&self, tools: &[&str], result: &str) -> Option<String> {
        let stored = stored_tool_result(result.to_string());
        let mut found = None;
        for (i, m) in self.messages.iter().enumerate() {
            let Some(id) = m.tool_call_id.as_deref() else {
                continue;
            };
            if m.content != stored {
                continue;
            }
            let called = self.messages.iter().take(i).rev().find_map(|earlier| {
                earlier
                    .tool_calls
                    .as_ref()?
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.function.name.as_str())
            });
            if called.is_some_and(|name| tools.contains(&name)) {
                found = Some(id.to_string());
            }
        }
        found
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
            // The summariser is a text call: it gets a note that pictures were
            // sent, not the pictures.
            for img in &msg.images {
                out.push_str(&format!("[Picture attached: {}]\n", img.label()));
            }
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
    ///
    /// The agent's checklist as it last stood and the files it changed in
    /// the folded part are appended to the summary word for word, merged with
    /// what the earlier summary kept (see [`super::carried`]): the model's
    /// summary can drop either, and a long task needs both to carry on.
    pub fn compact(&mut self, upto: usize, summary: impl Into<String>) {
        let upto = upto.min(self.messages.len());
        let summary = summary.into();
        // The model saw the earlier kept section and may have copied it; the
        // section is rebuilt from the calls instead.
        let (text, _) = super::carried::Carried::split(&summary);
        let (_, previous) =
            super::carried::Carried::split(self.summary.as_deref().unwrap_or_default());
        let folded: Vec<Message> = self.messages.range(..upto).cloned().collect();
        let kept: Vec<Message> = self.messages.range(upto..).cloned().collect();
        let carried = super::carried::Carried::after_folding(previous, &folded, &kept);
        let summary = super::carried::join(text, &carried);
        self.messages.drain(..upto);
        self.folded += upto;
        self.set_summary(summary);
    }

    /// Clear old tool results in one batch, before the conversation gets
    /// near the point where it would be compacted.
    ///
    /// Past [`CLEAR_TRIGGER_RATIO`] of `budget`, every tool result of an
    /// earlier turn is replaced by a one-line note naming the call, so the
    /// model can run it again if it still needs the output. The turn being
    /// worked on keeps its results (the model may be building its answer from
    /// all of them; if that turn alone is too big, the trim behind compaction
    /// shrinks it), and so do the newest [`KEEP_TOOL_RESULTS`] results
    /// overall. The calls themselves and every reply stay. Results of
    /// [`NEVER_CLEARED`] tools and short results (under
    /// [`MIN_CLEARED_BYTES`]) stay too.
    ///
    /// Each clearing changes the start of the request, so a provider's prefix
    /// cache has to write it again once. That is worth it only for a batch:
    /// nothing is cleared unless the batch frees at least
    /// [`CLEAR_AT_LEAST_RATIO`] of `budget`, and a batch takes every result
    /// it may, so the next one comes only after the conversation has grown
    /// back. Returns `None` when nothing was cleared.
    pub fn clear_old_tool_results(&mut self, budget: usize) -> Option<Cleared> {
        let tokens_before = self.estimate_tokens();
        if (tokens_before as f64) < budget as f64 * CLEAR_TRIGGER_RATIO {
            return None;
        }
        let results: Vec<usize> = (0..self.messages.len())
            .filter(|&i| self.messages[i].tool_call_id.is_some())
            .collect();
        let older = results.len().saturating_sub(KEEP_TOOL_RESULTS);
        let this_turn = self.last_turn_start().unwrap_or(0);
        let mut notes = Vec::new();
        let mut freed = 0usize;
        for &i in results[..older].iter().take_while(|&&i| i < this_turn) {
            let m = &self.messages[i];
            if m.content.len() < MIN_CLEARED_BYTES || m.content.starts_with(CLEARED_PREFIX) {
                continue;
            }
            let call = self.call_for(i);
            if call.is_some_and(|c| NEVER_CLEARED.contains(&c.function.name.as_str())) {
                continue;
            }
            let note = cleared_note(call, &m.content);
            if note.len() >= m.content.len() {
                continue;
            }
            freed += m.content.len().div_ceil(3) - note.len().div_ceil(3);
            notes.push((i, note));
        }
        if notes.is_empty() || (freed as f64) < budget as f64 * CLEAR_AT_LEAST_RATIO {
            return None;
        }
        let results = notes.len();
        for (i, note) in notes {
            self.messages[i].content = note;
        }
        Some(Cleared {
            results,
            tokens_before,
            tokens_after: self.estimate_tokens(),
        })
    }

    /// The tool call the result at `i` answers: the nearest earlier assistant
    /// message's call with its id (ids can repeat across replies with some
    /// providers, so the nearest one is the right one).
    fn call_for(&self, i: usize) -> Option<&crate::llm::ToolCall> {
        let id = self.messages[i].tool_call_id.as_deref()?;
        self.messages
            .iter()
            .take(i)
            .rev()
            .find_map(|m| m.tool_calls.as_ref()?.iter().find(|c| c.id == id))
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
        // Keep the message count within limits by dropping whole earlier
        // turns. The last turn (the question being worked on and its tool
        // rounds) is never cut here, however many messages a long run gives
        // it: dropping its question mid-run leaves the model without its task,
        // and the results still to come would have no call. The token budget
        // bounds it instead (see `trim_to_token_budget`).
        while self.messages.len() > self.max_messages && self.has_earlier_turn() {
            self.drop_oldest_message_group();
        }
    }

    /// Fit the conversation, with the system prompt and summary, into
    /// `max_tokens` (as [`Self::estimate_tokens`] counts them).
    ///
    /// Earlier turns go first, a whole turn at a time, oldest first. The last
    /// turn (the question being worked on) is never dropped. If it alone is
    /// too big it is shrunk instead:
    /// 1. while it would not fit even with every tool output cut to a bare
    ///    marker, its oldest step (an assistant reply, or a tool call message
    ///    with its results) is removed, never the newest tool round, and a
    ///    note on the question tells the model that steps were removed;
    /// 2. its tool outputs are cut, the longest first, to one common length
    ///    that just fits, each keeping its start and end around a marker that
    ///    says how many bytes were cut.
    ///
    /// So the result fits whenever the system prompt, the summary, the
    /// question (with that note, if steps had to go) and the newest tool round
    /// (its outputs cut to markers) do.
    /// A tool call is never separated from its results, and what is left
    /// still opens with a user turn.
    pub fn trim_to_token_budget(&mut self, max_tokens: usize) {
        while self.estimate_tokens() > max_tokens && self.has_earlier_turn() {
            self.drop_oldest_message_group();
        }
        if self.estimate_tokens() > max_tokens {
            self.shrink_last_turn(max_tokens);
        }
    }

    /// Where the last turn starts: the last plain user message.
    fn last_turn_start(&self) -> Option<usize> {
        (0..self.messages.len())
            .rev()
            .find(|&i| self.starts_turn(i))
    }

    /// Whether anything comes before the last turn. A history without a
    /// plain user message has no turn to keep, so all of it counts.
    fn has_earlier_turn(&self) -> bool {
        match self.last_turn_start() {
            Some(start) => start > 0,
            None => !self.messages.is_empty(),
        }
    }

    /// Step 1 and 2 of [`Self::trim_to_token_budget`] for the last turn.
    fn shrink_last_turn(&mut self, max_tokens: usize) {
        let Some(start) = self.last_turn_start() else {
            return;
        };
        while self.tokens_with_outputs_cut_to(start, 0) > max_tokens
            && self.remove_oldest_step(start)
        {
            let question = &mut self.messages[start].content;
            if !question.ends_with(STEPS_REMOVED_NOTE) {
                question.push_str(STEPS_REMOVED_NOTE);
            }
        }
        // The longest cut that fits, or 0 if none does: the total only grows
        // as the cut gets longer.
        let longest = self
            .outputs(start)
            .map(|m| m.content.len())
            .max()
            .unwrap_or(0);
        let (mut keep, mut too_long) = (0, longest);
        while keep < too_long {
            let mid = keep + (too_long - keep).div_ceil(2);
            if self.tokens_with_outputs_cut_to(start, mid) <= max_tokens {
                keep = mid;
            } else {
                too_long = mid - 1;
            }
        }
        let limit = keep + CUT_MARKER_MAX;
        for m in self.messages.iter_mut().skip(start + 1) {
            if m.tool_call_id.is_some() && m.content.len() > limit {
                m.content = cut_output(&m.content, keep);
            }
        }
    }

    /// The tool results of the last turn, which starts at `start`.
    fn outputs(&self, start: usize) -> impl Iterator<Item = &Message> {
        self.messages
            .iter()
            .skip(start + 1)
            .filter(|m| m.tool_call_id.is_some())
    }

    /// Estimated tokens if every tool output of the last turn (which starts
    /// at `start`) that is longer than `keep` bytes plus a marker were cut to
    /// that. An upper bound: a real marker can be shorter.
    fn tokens_with_outputs_cut_to(&self, start: usize, keep: usize) -> usize {
        let limit = keep + CUT_MARKER_MAX;
        let saved: usize = self
            .outputs(start)
            .filter(|m| m.content.len() > limit)
            .map(|m| message_tokens(m) - limit.div_ceil(3))
            .sum();
        self.estimate_tokens() - saved
    }

    /// Remove the oldest step of the last turn (which starts at `start`): an
    /// assistant reply, or an assistant tool call message together with the
    /// results that follow it. The newest tool round is never removed.
    /// Returns false when there is nothing left to remove.
    fn remove_oldest_step(&mut self, start: usize) -> bool {
        let len = self.messages.len();
        let is_step = |i: usize| self.messages[i].role == Role::Assistant;
        let newest_round = (start + 1..len)
            .rev()
            .find(|&i| self.messages[i].tool_calls.is_some());
        let Some(first) = (start + 1..len).find(|&i| is_step(i) && Some(i) != newest_round) else {
            return false;
        };
        let end = (first + 1..len).find(|&i| is_step(i)).unwrap_or(len);
        self.messages.drain(first..end);
        true
    }

    /// Drop the oldest turn: the front message and everything after it up to
    /// the next plain user message, so what remains still opens with a user
    /// turn (providers reject a conversation that starts with an assistant
    /// turn or a tool result). The results of the turn's tool calls go with
    /// it: they come before the next user message. They are not looked up by
    /// id, because ids repeat across replies with some providers (Ollama
    /// numbers them per reply), which would take later turns' results too.
    fn drop_oldest_message_group(&mut self) {
        if self.messages.pop_front().is_none() {
            return;
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

/// Old tool results are cleared once the conversation passes this share of
/// the budget, below compaction's trigger (0.8), so clearing usually spares
/// the conversation a summary call.
pub const CLEAR_TRIGGER_RATIO: f64 = 0.6;

/// The newest tool results are never cleared: the model is usually still
/// working from them.
pub const KEEP_TOOL_RESULTS: usize = 3;

/// A clearing has to free at least this share of the budget; a smaller one
/// would cost a cache write for little gain.
pub const CLEAR_AT_LEAST_RATIO: f64 = 0.1;

/// Results shorter than this are left alone: the note would save next to
/// nothing.
pub const MIN_CLEARED_BYTES: usize = 600;

/// Tools whose results are state the model keeps working from, not output it
/// can fetch again: the checklist and saved memories.
pub const NEVER_CLEARED: &[&str] = &["todo", "memory"];

/// Tools that only read, so the note can tell the model to run them again.
/// For any other tool the note says the call already ran, so the model does
/// not repeat a write or a command to see its output.
const RERUNNABLE: &[&str] = &[
    "read_file",
    "grep",
    "glob",
    "list_files",
    "find_path",
    "fetch",
    "web_search",
    "diagnostics",
    "browse",
];

/// How a cleared result starts, so a later batch skips it.
pub const CLEARED_PREFIX: &str = "[Output cleared to save context: ";

/// What a clearing did, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cleared {
    pub results: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
}

/// The note that stands in for a cleared result: which call made it, how big
/// it was, and that running the call again brings it back.
fn cleared_note(call: Option<&crate::llm::ToolCall>, content: &str) -> String {
    let lines = content.lines().count();
    let rerun = call.is_some_and(|c| RERUNNABLE.contains(&c.function.name.as_str()));
    let what = match call {
        Some(c) => {
            let args = c
                .function
                .arguments
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let shown = crate::text::truncate_bytes(&args, 160);
            let more = if shown.len() < args.len() { "…" } else { "" };
            format!("{} {shown}{more}", c.function.name)
        }
        None => "a tool call".to_string(),
    };
    let then = if rerun {
        "Run it again if you still need it."
    } else {
        "The call already ran; only its output is gone."
    };
    format!(
        "{CLEARED_PREFIX}{what} returned {lines} line{} ({} bytes). {then}]",
        if lines == 1 { "" } else { "s" },
        content.len()
    )
}

/// Heading that introduces the compaction summary in the system prompt.
pub const SUMMARY_HEADING: &str = "## Summary of the earlier part of this conversation";

/// Compacting fewer messages than this isn't worth an extra model call.
const MIN_MESSAGES_TO_COMPACT: usize = 4;

/// Longest single message copied into a compaction transcript.
const TRANSCRIPT_MESSAGE_LIMIT: usize = 4000;

/// Rough token cost of one message (~3 characters per token for code-heavy
/// text), including tool-call arguments, the reasoning sent back with it and
/// its pictures (counted by their pixels, not their base64 length).
fn message_tokens(m: &Message) -> usize {
    let calls: usize = m
        .tool_calls
        .iter()
        .flatten()
        .map(|c| c.function.name.len() + c.function.arguments.len())
        .sum();
    let reasoning = m.reasoning.as_ref().map_or(0, String::len);
    let images: usize = m.images.iter().map(|i| i.estimated_tokens()).sum();
    (m.content.len() + calls + reasoning).div_ceil(3) + images
}

/// Appended to the question when trimming removes steps of its turn, so the
/// model knows that work it did is no longer shown.
const STEPS_REMOVED_NOTE: &str = "\n\n[Earlier tool calls and replies made for this request \
     were removed to fit the context budget; their output is no longer shown.]";

/// The marker left in a tool output cut to fit the context budget is
/// `CUT_PREFIX`, the number of bytes cut, then `CUT_SUFFIX`.
const CUT_PREFIX: &str = "[... ";
const CUT_SUFFIX: &str = " bytes of this tool output were cut to fit the context budget ...]";
/// The longest a marker and the line breaks around it can be.
const CUT_MARKER_MAX: usize = CUT_PREFIX.len() + 20 + CUT_SUFFIX.len() + 2;

/// `output` cut down to at most `keep` bytes of its text around a marker:
/// about two thirds from the start (what was asked for) and the rest from the
/// end (where errors and totals usually are). An output that was cut before
/// is cut again from what is left of it, and its marker counts every byte cut
/// so far.
fn cut_output(output: &str, keep: usize) -> String {
    use crate::text::{tail_bytes, truncate_bytes};
    let head_keep = keep * 2 / 3;
    let (head, tail, cut) = match split_cut(output) {
        Some((old_head, old_cut, old_tail)) => {
            // A short tail leaves its share to the head.
            let head = truncate_bytes(old_head, head_keep.max(keep.saturating_sub(old_tail.len())));
            let tail = tail_bytes(old_tail, keep - head.len());
            // Saturating: the "earlier marker" may be text the tool printed,
            // with any number in it.
            let cut = old_cut
                .saturating_add(old_head.len() - head.len())
                .saturating_add(old_tail.len() - tail.len());
            (head, tail, cut)
        }
        None => {
            let head = truncate_bytes(output, head_keep);
            let rest = &output[head.len()..];
            let tail = tail_bytes(rest, keep - head.len());
            (head, tail, rest.len() - tail.len())
        }
    };
    let marker = format!("{CUT_PREFIX}{cut}{CUT_SUFFIX}");
    [head, marker.as_str(), tail]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// An output cut by [`cut_output`] as (head, bytes cut, tail).
fn split_cut(output: &str) -> Option<(&str, usize, &str)> {
    let end = output.find(CUT_SUFFIX)?;
    let start = output[..end].rfind(CUT_PREFIX)?;
    let cut = output[start + CUT_PREFIX.len()..end].parse().ok()?;
    let head = &output[..start];
    let tail = &output[end + CUT_SUFFIX.len()..];
    Some((
        head.strip_suffix('\n').unwrap_or(head),
        cut,
        tail.strip_prefix('\n').unwrap_or(tail),
    ))
}

impl Default for ConversationHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// A tool result as the conversation stores it: huge results are cut to
/// save context, keeping the start and the end (errors and totals are
/// usually last) around a marker.
fn stored_tool_result(result: String) -> String {
    const MAX_TOOL_RESULT_LEN: usize = 12000;
    if result.len() > MAX_TOOL_RESULT_LEN {
        crate::text::cut_middle(&result, MAX_TOOL_RESULT_LEN).into_owned()
    } else {
        result
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

    fn call(id: &str, tool: &str) -> Message {
        Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: id.to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: tool.to_string(),
                    arguments: "{}".to_string(),
                },
            }],
        )
    }

    #[test]
    fn repeat_of_finds_the_latest_identical_result_of_a_listed_tool() {
        let mut h = ConversationHistory::new();
        h.add_message(call("a", "read_file"));
        h.add_tool_result("a", "contents");
        h.add_message(call("b", "bash"));
        h.add_tool_result("b", "contents");
        h.add_message(call("c", "read_file"));
        h.add_tool_result("c", "contents");

        assert_eq!(
            h.repeat_of(&["read_file"], "contents").as_deref(),
            Some("c")
        );
        assert_eq!(h.repeat_of(&["read_file"], "other"), None);
        // `bash` is not listed: its identical output alone does not count.
        assert_eq!(h.repeat_of(&["grep"], "contents"), None);
    }

    #[test]
    fn repeat_of_compares_the_stored_form_of_a_huge_result() {
        let huge = "y".repeat(30_000);
        let mut h = ConversationHistory::new();
        h.add_message(call("a", "read_file"));
        h.add_tool_result("a", huge.clone());
        assert_eq!(h.repeat_of(&["read_file"], &huge).as_deref(), Some("a"));
    }

    #[test]
    fn a_result_that_is_trimmed_or_folded_is_no_longer_a_repeat() {
        let text = "z".repeat(4_000);
        let mut h = ConversationHistory::new();
        h.add_user_message("q");
        h.add_message(call("a", "read_file"));
        h.add_tool_result("a", text.clone());
        h.add_user_message("q2");
        // Shrinking the request shortens the old result: it no longer matches.
        h.trim_to_token_budget(300);
        assert_eq!(h.repeat_of(&["read_file"], &text), None);

        let mut folded = ConversationHistory::new();
        folded.add_user_message("q");
        folded.add_message(call("a", "read_file"));
        folded.add_tool_result("a", text.clone());
        folded.add_user_message("q2");
        let upto = folded.len() - 1;
        folded.compact(upto, "read a file");
        assert_eq!(folded.repeat_of(&["read_file"], &text), None);
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

        // Over budget in the last turn alone: the question and the tool call
        // stay, and the tool output is cut to fit. (This used to drop all
        // three, sending the model no question at all.)
        let mut h = ConversationHistory::new();
        h.add_message(msg(true, 0));
        h.add_message(Message::assistant_with_tools("", vec![call("c2")]));
        h.add_tool_result("c2", "r".repeat(300));
        h.trim_to_token_budget(50);
        assert_eq!(h.len(), 3);
        assert!(first_is_plain_user(&h));
        assert!(h.estimate_tokens() <= 50, "{}", h.estimate_tokens());
        let output = &h.get_conversation_messages()[2].content;
        assert!(output.contains(CUT_SUFFIX), "{output}");
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
    fn pictures_count_by_pixels_and_show_in_the_transcript() {
        let png = crate::llm::ImageInput::from_bytes(crate::llm::image::tests::PNG_1X1).unwrap();
        let mut plain = ConversationHistory::new();
        plain.add_user_message("look at this");
        let mut with_picture = ConversationHistory::new();
        with_picture.add_user_message_with_images("look at this", vec![png.clone()]);
        // A 1x1 picture costs one token, not its base64 length / 3.
        assert_eq!(with_picture.estimate_tokens(), plain.estimate_tokens() + 1);
        assert_eq!(with_picture.get_messages()[0].images, vec![png]);
        assert!(with_picture
            .transcript(1)
            .contains("[Picture attached: PNG 1x1]"));
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

    #[test]
    fn an_output_that_prints_a_cut_marker_is_cut_without_overflowing() {
        // Tool output is arbitrary text: this one quotes a marker carrying the
        // largest count there is, so it reads as an earlier cut.
        let quoted = format!(
            "start\n{CUT_PREFIX}{}{CUT_SUFFIX}\n{}",
            usize::MAX,
            "x".repeat(5000)
        );
        let cut = cut_output(&quoted, 100);
        assert!(cut.len() <= 100 + CUT_MARKER_MAX, "{} bytes", cut.len());
        let (head, count, tail) = split_cut(&cut).expect("a marker");
        assert_eq!((head, count), ("start", usize::MAX));
        assert_eq!(tail, "x".repeat(95));
    }

    #[test]
    fn a_long_tool_result_keeps_its_end() {
        let mut history = ConversationHistory::new();
        let output = format!(
            "START\n{}\nerror: build failed (TAIL)\n",
            "x".repeat(50_000)
        );
        history.add_tool_result("call_1", output);
        let kept = &history.get_messages()[0].content;
        assert!(kept.starts_with("START"), "lost the start");
        assert!(
            kept.ends_with("error: build failed (TAIL)\n"),
            "lost the end: {}",
            &kept[kept.len().saturating_sub(80)..]
        );
        assert!(kept.contains("bytes omitted"));
        assert!(kept.len() < 12_100);
    }

    fn call_with(id: &str, tool: &str, args: &str) -> Message {
        Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: id.to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: tool.to_string(),
                    arguments: args.to_string(),
                },
            }],
        )
    }

    /// `n` turns, each a question and one `read_file` returning `bytes`
    /// bytes of distinct text.
    fn reads(n: usize, bytes: usize) -> ConversationHistory {
        let mut h = ConversationHistory::new().with_system_prompt("sys");
        for i in 0..n {
            h.add_user_message(if i == 0 {
                "look around".to_string()
            } else {
                format!("and file {i}")
            });
            let id = format!("r{i}");
            h.add_message(call_with(
                &id,
                "read_file",
                &format!("{{\"path\": \"src/f{i}.rs\"}}"),
            ));
            let line = format!("fn f{i}() {{}} // padding padding padding\n");
            h.add_tool_result(&id, line.repeat(bytes / line.len() + 1));
        }
        h
    }

    #[test]
    fn nothing_is_cleared_below_the_trigger() {
        let mut h = reads(6, 3000);
        let before = h.get_conversation_messages();
        let budget = (h.estimate_tokens() as f64 / CLEAR_TRIGGER_RATIO) as usize + 10;
        assert_eq!(h.clear_old_tool_results(budget), None);
        assert_eq!(h.get_conversation_messages().len(), before.len());
        for (a, b) in before.iter().zip(h.get_conversation_messages()) {
            assert_eq!(a.content, b.content);
        }
    }

    #[test]
    fn old_results_become_notes_and_the_newest_stay() {
        let mut h = reads(8, 3000);
        let budget = h.estimate_tokens();
        let done = h.clear_old_tool_results(budget).expect("cleared");
        assert_eq!(done.results, 8 - KEEP_TOOL_RESULTS);
        assert!(done.tokens_after < done.tokens_before / 2, "{done:?}");
        assert_eq!(done.tokens_after, h.estimate_tokens());

        let msgs = h.get_conversation_messages();
        let results: Vec<&Message> = msgs.iter().filter(|m| m.tool_call_id.is_some()).collect();
        for (i, r) in results.iter().enumerate() {
            if i < 8 - KEEP_TOOL_RESULTS {
                assert!(r.content.starts_with(CLEARED_PREFIX), "{}", r.content);
                // The note names the call and its file, and says how to get it back.
                assert!(r
                    .content
                    .contains(&format!("read_file {{\"path\": \"src/f{i}.rs\"}}")));
                assert!(r.content.contains("Run it again"), "{}", r.content);
            } else {
                assert!(r.content.starts_with(&format!("fn f{i}()")));
            }
        }
        // Every call and question is untouched, so each turn still pairs up.
        assert_eq!(msgs.iter().filter(|m| m.tool_calls.is_some()).count(), 8);
        assert_eq!(msgs[0].content, "look around");
    }

    #[test]
    fn a_batch_too_small_to_pay_for_the_cache_write_is_skipped() {
        // Four reads: only the oldest may go, about 1,000 tokens. A long
        // system prompt puts the conversation past the trigger of a 20,000
        // token budget, where a batch must free 2,000.
        let mut h = reads(4, 3000);
        h.set_system_prompt("x".repeat(36_000));
        assert!(h.estimate_tokens() as f64 > 20_000.0 * CLEAR_TRIGGER_RATIO);
        assert_eq!(h.clear_old_tool_results(20_000), None);
        // A budget small enough that 1,000 tokens is a tenth of it clears.
        let mut small = reads(4, 3000);
        assert_eq!(
            small
                .clear_old_tool_results(small.estimate_tokens())
                .map(|c| c.results),
            Some(1)
        );
    }

    #[test]
    fn a_cleared_conversation_is_not_cleared_again_until_it_grows() {
        let mut h = reads(8, 3000);
        let budget = h.estimate_tokens();
        assert!(h.clear_old_tool_results(budget).is_some());
        assert_eq!(h.clear_old_tool_results(budget), None);
        // It grows back past the trigger: only the results that aged out of
        // the newest three go, the notes stay as they are.
        for i in 8..14 {
            let id = format!("r{i}");
            h.add_user_message(format!("and file {i}"));
            h.add_message(call_with(&id, "read_file", "{}"));
            h.add_tool_result(&id, "y".repeat(3000));
        }
        let notes_before: Vec<String> = h
            .get_conversation_messages()
            .into_iter()
            .filter(|m| m.content.starts_with(CLEARED_PREFIX))
            .map(|m| m.content)
            .collect();
        let again = h.clear_old_tool_results(budget).expect("cleared again");
        assert_eq!(again.results, 6);
        let notes_after: Vec<String> = h
            .get_conversation_messages()
            .into_iter()
            .filter(|m| m.content.starts_with(CLEARED_PREFIX))
            .map(|m| m.content)
            .collect();
        assert_eq!(&notes_after[..notes_before.len()], &notes_before[..]);
        assert_eq!(notes_after.len(), 14 - KEEP_TOOL_RESULTS);
    }

    #[test]
    fn the_checklist_memories_short_results_and_writes_are_handled_apart() {
        let mut h = ConversationHistory::new();
        h.add_user_message("do it");
        h.add_message(call_with("t", "todo", "{}"));
        h.add_tool_result("t", "[ ] step\n".repeat(200));
        h.add_message(call_with("m", "memory", "{}"));
        h.add_tool_result("m", "remembered ".repeat(200));
        h.add_message(call_with("s", "read_file", "{}"));
        h.add_tool_result("s", "short");
        h.add_message(call_with("b", "bash", "{\"command\": \"cargo test\"}"));
        h.add_tool_result("b", "test output\n".repeat(800));
        h.add_user_message("now search");
        for i in 0..KEEP_TOOL_RESULTS {
            let id = format!("k{i}");
            h.add_message(call_with(&id, "grep", "{}"));
            h.add_tool_result(&id, "hit");
        }
        let budget = h.estimate_tokens();
        let done = h.clear_old_tool_results(budget).expect("cleared");
        assert_eq!(done.results, 1);
        let by_id = |id: &str| {
            h.get_conversation_messages()
                .into_iter()
                .find(|m| m.tool_call_id.as_deref() == Some(id))
                .unwrap()
                .content
        };
        assert!(by_id("t").starts_with("[ ] step"));
        assert!(by_id("m").starts_with("remembered"));
        assert_eq!(by_id("s"), "short");
        let bash = by_id("b");
        assert!(bash.starts_with(CLEARED_PREFIX), "{bash}");
        assert!(bash.contains("bash {\"command\": \"cargo test\"}"));
        assert!(bash.contains("800 lines"), "{bash}");
        // A command is not to be run again just to see its output.
        assert!(bash.contains("already ran"), "{bash}");
        assert!(!bash.contains("Run it again"));
    }

    #[test]
    fn the_turn_being_worked_on_keeps_every_result() {
        // One question with eight reads: the model may be building its
        // answer from all of them, so none is cleared, however old.
        let mut h = ConversationHistory::new();
        h.add_user_message("read all of it");
        for i in 0..8 {
            let id = format!("r{i}");
            h.add_message(call_with(&id, "read_file", "{}"));
            h.add_tool_result(&id, "z".repeat(3000));
        }
        assert_eq!(h.clear_old_tool_results(h.estimate_tokens()), None);
        // Once a new question starts, they belong to an earlier turn.
        h.add_user_message("thanks, now this");
        let done = h.clear_old_tool_results(h.estimate_tokens()).unwrap();
        assert_eq!(done.results, 8 - KEEP_TOOL_RESULTS);
    }

    #[test]
    fn a_cleared_read_is_no_repeat_so_reading_it_again_sends_the_file() {
        let mut h = reads(8, 3000);
        let first = h
            .get_conversation_messages()
            .into_iter()
            .find(|m| m.tool_call_id.as_deref() == Some("r0"))
            .unwrap()
            .content;
        assert_eq!(h.repeat_of(&["read_file"], &first).as_deref(), Some("r0"));
        let budget = h.estimate_tokens();
        h.clear_old_tool_results(budget).expect("cleared");
        assert_eq!(h.repeat_of(&["read_file"], &first), None);
    }
}
