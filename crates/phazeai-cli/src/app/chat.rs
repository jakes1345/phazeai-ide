//! Conversation items and their rendering into wrapped, width-exact lines.

use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::markdown::render_markdown;
use crate::text::{truncate_to_width, wrap_spans, WrapMode};
use crate::theme::Theme;
use crate::tool_view::Digest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    /// Informational output from the CLI itself (command results, notices).
    System,
    /// Something went wrong.
    Error,
    /// Tool output restored from a saved conversation.
    Tool,
    /// A unified diff (colour-coded per line).
    Diff,
}

impl MessageRole {
    pub fn as_saved(self) -> &'static str {
        match self {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System | MessageRole::Error => "system",
            MessageRole::Tool | MessageRole::Diff => "tool",
        }
    }

    pub fn from_saved(role: &str) -> Self {
        match role {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "tool" => MessageRole::Tool,
            _ => MessageRole::System,
        }
    }
}

/// Cached rendering of a message for a given width / content / theme.
#[derive(Clone, Default)]
struct RenderCache {
    key: Option<(usize, usize, u64)>,
    lines: Vec<Line<'static>>,
}

#[derive(Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: String,
    cache: RenderCache,
}

impl ChatMessage {
    pub fn new(role: MessageRole, content: String, timestamp: String) -> Self {
        Self {
            role,
            content,
            timestamp,
            cache: RenderCache::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    /// Waiting for the user to approve.
    Pending,
    Running,
    Ok,
    Failed,
}

#[derive(Clone)]
pub struct ToolCard {
    pub name: String,
    /// Short description of the arguments (command, path, pattern…).
    pub label: String,
    pub status: ToolStatus,
    pub digest: Option<Digest>,
    pub started: Option<Instant>,
    pub elapsed: Option<Duration>,
    rendered: Vec<Line<'static>>,
}

impl ToolCard {
    pub fn new(name: String, label: String, status: ToolStatus) -> Self {
        Self {
            name,
            label,
            status,
            digest: None,
            started: None,
            elapsed: None,
            rendered: Vec::new(),
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.status, ToolStatus::Ok | ToolStatus::Failed)
    }
}

#[derive(Clone)]
pub enum ChatItem {
    Message(ChatMessage),
    Tool(ToolCard),
}

impl ChatItem {
    pub fn message(role: MessageRole, content: String) -> Self {
        ChatItem::Message(ChatMessage::new(role, content, now_str()))
    }

    pub fn role(&self) -> Option<MessageRole> {
        match self {
            ChatItem::Message(m) => Some(m.role),
            ChatItem::Tool(_) => None,
        }
    }
}

pub fn now_str() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

/// Render options shared by every item for one frame.
pub struct RenderCtx<'a> {
    pub theme: &'a Theme,
    pub theme_gen: u64,
    pub width: usize,
    pub expand_tools: bool,
    pub spinner: &'static str,
}

/// Spinner frames (braille), advanced by wall-clock time.
pub fn spinner_frame() -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    FRAMES[(ms / 80) as usize % FRAMES.len()]
}

/// Whether a blank line goes between `prev` and `next`.
pub fn needs_gap(prev: Option<&ChatItem>, next: &ChatItem) -> bool {
    match (prev, next) {
        (None, _) => false,
        // Consecutive tool cards stack tightly.
        (Some(ChatItem::Tool(_)), ChatItem::Tool(_)) => false,
        // Consecutive diff / system lines from one command stay together.
        (Some(ChatItem::Message(a)), ChatItem::Message(b))
            if a.role == MessageRole::Diff && b.role == MessageRole::Diff =>
        {
            false
        }
        _ => true,
    }
}

impl ChatItem {
    /// Ensure the item is rendered for this frame and return its lines.
    pub fn lines(&mut self, ctx: &RenderCtx<'_>) -> &[Line<'static>] {
        match self {
            ChatItem::Message(m) => {
                let key = (ctx.width, m.content.len(), ctx.theme_gen);
                if m.cache.key != Some(key) {
                    m.cache.lines = render_message(m, ctx.width, ctx.theme);
                    m.cache.key = Some(key);
                }
                &m.cache.lines
            }
            ChatItem::Tool(card) => {
                card.rendered = render_tool_card(card, ctx);
                &card.rendered
            }
        }
    }
}

fn wrap_each_line(
    text: &str,
    width: usize,
    style: Style,
    first: &[Span<'static>],
    rest: &[Span<'static>],
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let rest_w = crate::text::spans_width(rest);
    for (i, raw) in text.split('\n').enumerate() {
        let lead = if i == 0 { first } else { rest };
        // Continuation lines align with the line's text column, so aligned
        // "name    description" listings stay readable when they wrap.
        let mut cont = rest.to_vec();
        let hang = hanging_column(raw);
        let indent = raw.len() - raw.trim_start_matches(' ').len();
        // Too deep for a narrow screen: fall back to a small extra indent.
        let hang = if rest_w + hang <= width / 2 {
            hang
        } else {
            (indent + 4).min(width / 4)
        };
        if hang > 0 {
            cont.push(Span::raw(" ".repeat(hang)));
        }
        out.extend(wrap_spans(
            &[Span::styled(raw.to_string(), style)],
            width,
            lead,
            &cont,
            WrapMode::Word,
        ));
    }
    out
}

/// Column where a line's wrapped continuation should start: after the last
/// run of 2+ spaces (a column gap) if there is one, else after the leading
/// indentation.
fn hanging_column(line: &str) -> usize {
    let indent = line.len() - line.trim_start().len();
    let body = &line[indent..];
    let gap_end = body
        .match_indices("  ")
        .last()
        .map(|(i, _)| {
            let after = &body[i..];
            i + (after.len() - after.trim_start().len())
        })
        .filter(|end| *end < body.len());
    match gap_end {
        Some(end) => crate::text::str_width(&line[..indent + end]),
        None => crate::text::str_width(&line[..indent]),
    }
}

fn render_message(m: &ChatMessage, width: usize, t: &Theme) -> Vec<Line<'static>> {
    let indent = vec![Span::raw("  ")];
    let content = crate::text::sanitize(&m.content);
    match m.role {
        MessageRole::User => {
            let marker = vec![Span::styled(
                "❯ ",
                Style::default()
                    .fg(t.user_color)
                    .add_modifier(Modifier::BOLD),
            )];
            let style = Style::default().fg(t.fg).add_modifier(Modifier::BOLD);
            wrap_each_line(&content, width, style, &marker, &indent)
        }
        MessageRole::Assistant => {
            let body = render_markdown(&content, width.saturating_sub(2), t);
            let marker = Span::styled("● ", Style::default().fg(t.assistant_color));
            if body.is_empty() {
                return vec![Line::from(marker)];
            }
            body.into_iter()
                .enumerate()
                .map(|(i, line)| {
                    let mut spans = vec![if i == 0 {
                        marker.clone()
                    } else {
                        Span::raw("  ")
                    }];
                    spans.extend(line.spans);
                    Line::from(spans)
                })
                .collect()
        }
        MessageRole::System | MessageRole::Tool => {
            let style = Style::default().fg(t.system_color);
            wrap_each_line(&content, width, style, &indent, &indent)
        }
        MessageRole::Error => {
            let marker = vec![Span::styled(
                "✗ ",
                Style::default().fg(t.error).add_modifier(Modifier::BOLD),
            )];
            let style = Style::default().fg(t.error);
            wrap_each_line(&content, width, style, &marker, &indent)
        }
        MessageRole::Diff => content
            .split('\n')
            .map(|raw| {
                let style = if raw.starts_with("+++") || raw.starts_with("---") {
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
                } else if raw.starts_with("@@") {
                    Style::default().fg(t.accent)
                } else if raw.starts_with('+') {
                    Style::default().fg(t.success)
                } else if raw.starts_with('-') {
                    Style::default().fg(t.error)
                } else if raw.starts_with("diff ") || raw.starts_with("index ") {
                    Style::default().fg(t.muted).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.muted)
                };
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(truncate_to_width(raw, width.saturating_sub(2)), style),
                ])
            })
            .collect(),
    }
}

fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{secs:.0}s")
    } else {
        format!("{}m{:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

fn render_tool_card(card: &ToolCard, ctx: &RenderCtx<'_>) -> Vec<Line<'static>> {
    let t = ctx.theme;
    let (glyph, color) = match card.status {
        ToolStatus::Pending => ("?", t.warning),
        ToolStatus::Running => (ctx.spinner, t.accent),
        ToolStatus::Ok => ("✓", t.success),
        ToolStatus::Failed => ("✗", t.error),
    };

    let mut header = vec![
        Span::raw("  "),
        Span::styled(
            format!("{glyph} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            card.name.clone(),
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
    ];
    let mut meta: Vec<String> = Vec::new();
    match card.status {
        ToolStatus::Pending => meta.push("awaiting approval".into()),
        ToolStatus::Running => {
            if let Some(start) = card.started {
                meta.push(format_elapsed(start.elapsed()));
            }
        }
        ToolStatus::Ok | ToolStatus::Failed => {
            if let Some(d) = &card.digest {
                if !d.headline.is_empty() && card.status == ToolStatus::Ok {
                    meta.push(d.headline.clone());
                }
            }
            if let Some(e) = card.elapsed {
                meta.push(format_elapsed(e));
            }
        }
    }
    let meta = if meta.is_empty() {
        String::new()
    } else {
        format!("  {}", meta.join(" · "))
    };
    let used = 4 + crate::text::str_width(&card.name) + crate::text::str_width(&meta);
    if !card.label.is_empty() {
        let room = ctx.width.saturating_sub(used + 1);
        if room > 3 {
            header.push(Span::styled(
                format!(" {}", truncate_to_width(&card.label, room)),
                Style::default().fg(t.muted),
            ));
        }
    }
    header.push(Span::styled(meta, Style::default().fg(t.dim)));
    let mut lines = vec![Line::from(header)];

    // Body: failure text, or the digest's detail lines.
    let Some(digest) = &card.digest else {
        return lines;
    };
    let mut body: Vec<String> = Vec::new();
    if card.status == ToolStatus::Failed && !digest.headline.is_empty() {
        body.push(digest.headline.clone());
    }
    body.extend(digest.body.iter().cloned());
    if body.is_empty() {
        return lines;
    }

    let limit = if ctx.expand_tools { 40 } else { 3 };
    let body_style = if card.status == ToolStatus::Failed {
        Style::default().fg(t.error)
    } else {
        Style::default().fg(t.muted)
    };
    let avail = ctx.width.saturating_sub(6);
    for (i, line) in body.iter().take(limit).enumerate() {
        let lead = if i == 0 { "    ⎿ " } else { "      " };
        lines.push(Line::from(vec![
            Span::styled(lead, Style::default().fg(t.dim)),
            Span::styled(truncate_to_width(line, avail), body_style),
        ]));
    }
    if body.len() > limit {
        let hint = if ctx.expand_tools {
            format!("… +{} more lines", body.len() - limit)
        } else {
            format!("… +{} lines (ctrl+t to expand)", body.len() - limit)
        };
        lines.push(Line::from(vec![
            Span::raw("      "),
            Span::styled(hint, Style::default().fg(t.dim)),
        ]));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(theme: &Theme) -> RenderCtx<'_> {
        RenderCtx {
            theme,
            theme_gen: 0,
            width: 40,
            expand_tools: false,
            spinner: "⠋",
        }
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn user_message_keeps_indentation_and_wraps() {
        let theme = Theme::dark();
        let mut item = ChatItem::message(
            MessageRole::User,
            "first line\n  indented line that is long enough to wrap".into(),
        );
        let out = texts(item.lines(&ctx(&theme)));
        assert_eq!(out[0], "❯ first line");
        assert!(out[1].starts_with("    indented"));
        for l in &out {
            assert!(crate::text::str_width(l) <= 40);
        }
    }

    #[test]
    fn assistant_message_gets_marker() {
        let theme = Theme::dark();
        let mut item = ChatItem::message(MessageRole::Assistant, "Hello **world**".into());
        let out = texts(item.lines(&ctx(&theme)));
        assert_eq!(out, vec!["● Hello world"]);
    }

    #[test]
    fn cache_invalidates_on_content_change() {
        let theme = Theme::dark();
        let mut item = ChatItem::message(MessageRole::Assistant, "a".into());
        assert_eq!(texts(item.lines(&ctx(&theme))), vec!["● a"]);
        if let ChatItem::Message(m) = &mut item {
            m.content.push_str("bc");
        }
        assert_eq!(texts(item.lines(&ctx(&theme))), vec!["● abc"]);
    }

    #[test]
    fn tool_card_collapses_body() {
        let theme = Theme::dark();
        let mut card = ToolCard::new("bash".into(), "cargo test".into(), ToolStatus::Ok);
        card.digest = Some(Digest {
            headline: "exit 0".into(),
            body: (1..=5).map(|i| format!("line {i}")).collect(),
        });
        card.elapsed = Some(Duration::from_millis(1500));
        let mut item = ChatItem::Tool(card);
        let out = texts(item.lines(&ctx(&theme)));
        assert_eq!(out[0], "  ✓ bash cargo test  exit 0 · 1.5s");
        assert_eq!(out[1], "    ⎿ line 1");
        assert_eq!(out.len(), 5);
        assert!(out[4].contains("+2 lines"));
    }

    #[test]
    fn hanging_indent_follows_columns() {
        assert_eq!(hanging_column("  /help      Show help"), 13);
        assert_eq!(hanging_column("    indented text"), 4);
        assert_eq!(hanging_column("plain"), 0);
        assert_eq!(hanging_column("trailing gap  "), 0);
    }

    #[test]
    fn gaps_between_items() {
        let tool = ChatItem::Tool(ToolCard::new("a".into(), String::new(), ToolStatus::Ok));
        let msg = ChatItem::message(MessageRole::Assistant, "x".into());
        assert!(!needs_gap(None, &msg));
        assert!(!needs_gap(Some(&tool), &tool));
        assert!(needs_gap(Some(&msg), &tool));
    }
}
