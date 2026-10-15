//! Markdown → ratatui lines, wrapped to an exact width.
//!
//! Handles headings, emphasis / strong / strikethrough, inline code, links,
//! nested ordered / unordered / task lists, block quotes, rules, tables and
//! fenced code blocks with syntect highlighting. Output lines never exceed
//! the requested width, so callers can count lines for scrolling exactly.
//!
//! Rendering is tolerant of partial input (streaming): an unterminated code
//! fence simply renders as an open code block.

use std::sync::OnceLock;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::text::{spans_width, str_width, truncate_spans, wrap_spans, WrapMode};
use crate::theme::Theme;

fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    static TS: OnceLock<ThemeSet> = OnceLock::new();
    TS.get_or_init(ThemeSet::load_defaults)
}

fn syntect_theme_for(theme: &Theme) -> &'static syntect::highlighting::Theme {
    let ts = theme_set();
    let name = match theme.name {
        "gruvbox" => "base16-eighties.dark",
        "catppuccin" => "base16-mocha.dark",
        _ => "base16-ocean.dark",
    };
    ts.themes
        .get(name)
        .or_else(|| ts.themes.values().next())
        .expect("syntect ships default themes")
}

/// Highlight `code` as `lang`, returning one span list per source line.
/// Falls back to plain `code_fg` text for unknown languages or no-colour mode.
pub fn highlight_code(code: &str, lang: &str, theme: &Theme) -> Vec<Vec<Span<'static>>> {
    let plain = Style::default().fg(theme.code_fg);
    let code = code.replace('\t', "    ");
    let ss = syntax_set();
    let token = lang.split([',', ' ']).next().unwrap_or("").trim();
    let syntax = if token.is_empty() || !theme.mode.has_color() {
        None
    } else {
        ss.find_syntax_by_token(token)
            .or_else(|| ss.find_syntax_by_extension(token))
    };

    let Some(syntax) = syntax else {
        return code
            .lines()
            .map(|l| vec![Span::styled(l.to_string(), plain)])
            .collect();
    };

    let mut h = HighlightLines::new(syntax, syntect_theme_for(theme));
    let mut out = Vec::new();
    for line in code.lines() {
        let with_nl = format!("{line}\n");
        let spans = match h.highlight_line(&with_nl, ss) {
            Ok(ranges) => ranges
                .into_iter()
                .filter_map(|(style, text)| {
                    let text = text.trim_end_matches('\n');
                    if text.is_empty() {
                        return None;
                    }
                    let fg = theme.map_color(Color::Rgb(
                        style.foreground.r,
                        style.foreground.g,
                        style.foreground.b,
                    ));
                    let mut s = Style::default().fg(fg);
                    if style.font_style.contains(FontStyle::BOLD) {
                        s = s.add_modifier(Modifier::BOLD);
                    }
                    if style.font_style.contains(FontStyle::ITALIC) {
                        s = s.add_modifier(Modifier::ITALIC);
                    }
                    Some(Span::styled(text.to_string(), s))
                })
                .collect(),
            Err(_) => vec![Span::styled(line.to_string(), plain)],
        };
        out.push(spans);
    }
    out
}

struct TableState {
    aligns: Vec<Alignment>,
    rows: Vec<(bool, Vec<Vec<Span<'static>>>)>,
    row: Vec<Vec<Span<'static>>>,
    cell: Vec<Span<'static>>,
    in_head: bool,
}

struct Renderer<'t> {
    theme: &'t Theme,
    width: usize,
    lines: Vec<Line<'static>>,
    inline: Vec<Span<'static>>,
    strong: usize,
    emphasis: usize,
    strike: usize,
    link: Option<String>,
    link_text: String,
    heading: Option<HeadingLevel>,
    quote_depth: usize,
    /// Next number for each nested list level (`None` = bullet list).
    lists: Vec<Option<u64>>,
    /// Marker for the first line of the current list item, consumed on flush.
    item_marker: Option<Vec<Span<'static>>>,
    code: Option<(String, String)>,
    table: Option<TableState>,
    pending_blank: bool,
}

impl<'t> Renderer<'t> {
    fn new(theme: &'t Theme, width: usize) -> Self {
        Self {
            theme,
            width: width.max(10),
            lines: Vec::new(),
            inline: Vec::new(),
            strong: 0,
            emphasis: 0,
            strike: 0,
            link: None,
            link_text: String::new(),
            heading: None,
            quote_depth: 0,
            lists: Vec::new(),
            item_marker: None,
            code: None,
            table: None,
            pending_blank: false,
        }
    }

    fn quote_prefix(&self) -> Vec<Span<'static>> {
        (0..self.quote_depth)
            .map(|_| Span::styled("▎ ", Style::default().fg(self.theme.muted)))
            .collect()
    }

    /// Indentation for content inside the current list nesting (excluding the
    /// marker of the innermost item).
    fn list_indent(&self) -> usize {
        // Each enclosing level contributes the width of its marker column.
        self.lists.len().saturating_sub(1) * 3
    }

    fn start_block(&mut self) {
        if self.pending_blank && !self.lines.is_empty() {
            let prefix = self.quote_prefix();
            self.lines.push(Line::from(prefix));
        }
        self.pending_blank = false;
    }

    fn inline_style(&self) -> Style {
        let t = self.theme;
        let mut s = Style::default().fg(t.fg);
        if let Some(level) = self.heading {
            s = match level {
                HeadingLevel::H1 => Style::default()
                    .fg(t.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                HeadingLevel::H2 => Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                _ => Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
            };
        }
        if self.quote_depth > 0 {
            s = s.fg(t.muted).add_modifier(Modifier::ITALIC);
        }
        if self.strong > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.emphasis > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.link.is_some() {
            s = s.fg(t.accent).add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    fn code_style(&self) -> Style {
        let t = self.theme;
        if t.mode.has_color() {
            Style::default().fg(t.code_fg).bg(t.code_bg)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    fn push_inline(&mut self, text: String, style: Style) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push(Span::styled(text, style));
        } else {
            self.inline.push(Span::styled(text, style));
        }
    }

    /// Wrap and emit the accumulated inline content as one block.
    fn flush_inline(&mut self) {
        if self.inline.is_empty() && self.item_marker.is_none() {
            return;
        }
        let quote = self.quote_prefix();
        let mut first = quote.clone();
        let mut rest = quote;
        if self.list_indent() > 0 {
            let indent = Span::raw(" ".repeat(self.list_indent()));
            first.push(indent.clone());
            rest.push(indent);
        }
        if let Some(marker) = self.item_marker.take() {
            let w = spans_width(&marker);
            first.extend(marker);
            rest.push(Span::raw(" ".repeat(w)));
        } else if !self.lists.is_empty() {
            // Continuation paragraph inside a list item: align with item text.
            let pad = Span::raw("   ");
            first.push(pad.clone());
            rest.push(pad);
        }
        let spans = std::mem::take(&mut self.inline);
        self.lines.extend(wrap_spans(
            &spans,
            self.width,
            &first,
            &rest,
            WrapMode::Word,
        ));
    }

    fn render_code_block(&mut self, lang: &str, code: &str) {
        let t = self.theme;
        let border = Style::default().fg(t.dim);
        let quote = self.quote_prefix();
        let indent = " ".repeat(self.list_indent() + if self.lists.is_empty() { 0 } else { 3 });
        let mut lead = quote;
        lead.push(Span::raw(indent));
        let lead_w = spans_width(&lead);
        let inner = self.width.saturating_sub(lead_w);

        let label = if lang.is_empty() {
            String::new()
        } else {
            format!(" {lang} ")
        };
        let mut top = lead.clone();
        top.push(Span::styled("╭─", border));
        if !label.is_empty() {
            top.push(Span::styled(
                label.clone(),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ));
        }
        // Frame width: the full line, capped so wide terminals don't get a
        // screen-wide rule.
        let frame_w = inner.min(56);
        let used = 2 + str_width(&label);
        top.push(Span::styled(
            "─".repeat(frame_w.saturating_sub(used)),
            border,
        ));
        self.lines.push(Line::from(top));

        let mut gutter = lead.clone();
        gutter.push(Span::styled("│ ", border));
        let code = code.strip_suffix('\n').unwrap_or(code);
        let highlighted = if code.is_empty() {
            vec![Vec::new()]
        } else {
            highlight_code(code, lang, t)
        };
        for spans in highlighted {
            self.lines.extend(wrap_spans(
                &spans,
                self.width,
                &gutter,
                &gutter,
                WrapMode::Char,
            ));
        }

        let mut bottom = lead;
        bottom.push(Span::styled(
            format!("╰{}", "─".repeat(frame_w.saturating_sub(1))),
            border,
        ));
        self.lines.push(Line::from(bottom));
    }

    fn render_table(&mut self, table: TableState) {
        let t = self.theme;
        let cols = table.rows.iter().map(|(_, r)| r.len()).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let mut widths = vec![1usize; cols];
        for (_, row) in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(spans_width(cell));
            }
        }
        let quote = self.quote_prefix();
        let avail = self
            .width
            .saturating_sub(spans_width(&quote))
            .saturating_sub(3 * (cols - 1));
        // Shrink the widest column until everything fits.
        while widths.iter().sum::<usize>() > avail {
            let (idx, w) = widths
                .iter()
                .enumerate()
                .max_by_key(|(_, w)| **w)
                .map(|(i, w)| (i, *w))
                .unwrap_or((0, 0));
            if w <= 3 {
                break;
            }
            widths[idx] -= 1;
        }

        let sep = Style::default().fg(t.dim);
        for (ri, (is_head, row)) in table.rows.iter().enumerate() {
            let mut spans = quote.clone();
            for (ci, w) in widths.iter().enumerate() {
                if ci > 0 {
                    spans.push(Span::styled(" │ ", sep));
                }
                let empty = Vec::new();
                let cell = row.get(ci).unwrap_or(&empty);
                let mut cell = truncate_spans(cell, *w);
                if *is_head {
                    for s in &mut cell {
                        s.style = s.style.add_modifier(Modifier::BOLD);
                    }
                }
                let pad = w.saturating_sub(spans_width(&cell));
                let (left, right) = match table.aligns.get(ci) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                if left > 0 {
                    spans.push(Span::raw(" ".repeat(left)));
                }
                spans.extend(cell);
                // Don't pad the last column with trailing spaces.
                if right > 0 && ci + 1 < cols {
                    spans.push(Span::raw(" ".repeat(right)));
                }
            }
            self.lines.push(Line::from(spans));
            if *is_head && ri + 1 < table.rows.len() {
                let rule = widths
                    .iter()
                    .map(|w| "─".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                let mut l = quote.clone();
                l.push(Span::styled(rule, sep));
                self.lines.push(Line::from(l));
            }
        }
    }

    fn bullet_for_depth(&self, depth: usize) -> &'static str {
        match depth % 3 {
            1 => "•",
            2 => "◦",
            _ => "▪",
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if let Some((_, buf)) = self.code.as_mut() {
                    buf.push_str(&text);
                } else {
                    if self.link.is_some() {
                        self.link_text.push_str(&text);
                    }
                    let style = self.inline_style();
                    self.push_inline(text.into_string(), style);
                }
            }
            Event::Code(code) => {
                if self.link.is_some() {
                    self.link_text.push_str(&code);
                }
                let style = self.code_style();
                self.push_inline(code.into_string(), style);
            }
            Event::InlineMath(m) | Event::DisplayMath(m) => {
                let style = self.code_style();
                self.push_inline(m.into_string(), style);
            }
            Event::Html(html) => {
                // Block HTML arrives line by line; keep the line structure.
                let style = Style::default().fg(self.theme.muted);
                let text = format!("{}\n", html.trim_end_matches('\n'));
                self.push_inline(text, style);
            }
            Event::InlineHtml(html) => {
                let style = Style::default().fg(self.theme.muted);
                self.push_inline(html.into_string(), style);
            }
            Event::SoftBreak => {
                let style = self.inline_style();
                self.push_inline(" ".into(), style);
            }
            Event::HardBreak => {
                let style = self.inline_style();
                self.push_inline("\n".into(), style);
            }
            Event::Rule => {
                self.flush_inline();
                self.start_block();
                let mut l = self.quote_prefix();
                let w = self.width.saturating_sub(spans_width(&l));
                l.push(Span::styled(
                    "─".repeat(w),
                    Style::default().fg(self.theme.dim),
                ));
                self.lines.push(Line::from(l));
                self.pending_blank = true;
            }
            Event::TaskListMarker(checked) => {
                let (text, color) = if checked {
                    ("[x] ", self.theme.success)
                } else {
                    ("[ ] ", self.theme.muted)
                };
                let mut marker = self.item_marker.take().unwrap_or_default();
                // Replace the bullet with the checkbox but keep the indent.
                marker.pop();
                marker.push(Span::styled(text, Style::default().fg(color)));
                self.item_marker = Some(marker);
            }
            Event::FootnoteReference(name) => {
                let style = Style::default().fg(self.theme.muted);
                self.push_inline(format!("[^{name}]"), style);
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.table.is_none() {
                    self.start_block();
                }
            }
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.start_block();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.start_block();
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                self.start_block();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => {
                self.flush_inline();
                self.start_block();
            }
            Tag::List(start) => {
                self.flush_inline();
                if self.lists.is_empty() {
                    self.start_block();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush_inline();
                let depth = self.lists.len();
                let marker_text = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}.");
                        *n += 1;
                        s
                    }
                    _ => self.bullet_for_depth(depth).to_string(),
                };
                let color = if marker_text.ends_with('.') {
                    self.theme.muted
                } else {
                    self.theme.accent
                };
                self.item_marker = Some(vec![Span::styled(
                    format!("{marker_text:<2} "),
                    Style::default().fg(color),
                )]);
            }
            Tag::Table(aligns) => {
                self.flush_inline();
                self.start_block();
                self.table = Some(TableState {
                    aligns,
                    rows: Vec::new(),
                    row: Vec::new(),
                    cell: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = true;
                }
            }
            Tag::TableRow | Tag::TableCell => {}
            Tag::Emphasis => self.emphasis += 1,
            Tag::Strong => self.strong += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
                self.link_text.clear();
            }
            Tag::Image { dest_url, .. } => {
                let style = Style::default().fg(self.theme.muted);
                self.push_inline(format!("[image: {dest_url}] "), style);
            }
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => {
                self.flush_inline();
                self.start_block();
            }
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if self.table.is_none() {
                    self.flush_inline();
                    self.pending_blank = true;
                }
            }
            TagEnd::Heading(_) => {
                self.flush_inline();
                self.heading = None;
                self.pending_blank = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.pending_blank = true;
            }
            TagEnd::CodeBlock => {
                if let Some((lang, code)) = self.code.take() {
                    self.render_code_block(&lang, &code);
                }
                self.pending_blank = true;
            }
            TagEnd::HtmlBlock => {
                self.flush_inline();
                self.pending_blank = true;
            }
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.pending_blank = true;
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                // Tight lists never set pending_blank inside items; loose
                // lists do via their paragraphs, which is what we want.
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(table);
                }
                self.pending_blank = true;
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    // pulldown-cmark emits head cells directly under TableHead.
                    if !t.cell.is_empty() {
                        let cell = std::mem::take(&mut t.cell);
                        t.row.push(cell);
                    }
                    let row = std::mem::take(&mut t.row);
                    let head = t.in_head;
                    t.rows.push((head, row));
                    t.in_head = false;
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let cell = std::mem::take(&mut t.cell);
                    t.row.push(cell);
                }
            }
            TagEnd::Emphasis => self.emphasis = self.emphasis.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some(url) = self.link.take() {
                    let shown = self.link_text.trim();
                    if !url.is_empty() && !url.starts_with('#') && shown != url {
                        let style = Style::default().fg(self.theme.muted);
                        self.push_inline(format!(" ({url})"), style);
                    }
                }
            }
            TagEnd::Image => {}
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => {
                self.flush_inline();
                self.pending_blank = true;
            }
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        // Streaming: an unterminated fence is still shown as code.
        if let Some((lang, code)) = self.code.take() {
            self.render_code_block(&lang, &code);
        }
        if let Some(table) = self.table.take() {
            self.render_table(table);
        }
        self.flush_inline();
        self.lines
    }
}

/// Render markdown `content` into lines no wider than `width` cells.
pub fn render_markdown(content: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let opts = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut r = Renderer::new(theme, width);
    for event in Parser::new_ext(content, opts) {
        r.handle(event);
    }
    r.finish()
}

/// Extract the last fenced code block from markdown (used by `/copy code`).
pub fn last_code_block(content: &str) -> Option<String> {
    let mut last = None;
    let mut current: Option<String> = None;
    for event in Parser::new_ext(content, Options::empty()) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => current = Some(String::new()),
            Event::Text(t) => {
                if let Some(buf) = current.as_mut() {
                    buf.push_str(&t);
                }
            }
            Event::End(TagEnd::CodeBlock) => last = current.take(),
            _ => {}
        }
    }
    last.or(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::dark()
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn render(md: &str, width: usize) -> Vec<String> {
        texts(&render_markdown(md, width, &theme()))
    }

    #[test]
    fn paragraphs_are_separated_and_soft_breaks_join() {
        let out = render("first line\nstill first\n\nsecond", 80);
        assert_eq!(out, vec!["first line still first", "", "second"]);
    }

    #[test]
    fn bold_italic_and_code_are_styled() {
        let lines = render_markdown("a **b** *c* `d`", 80, &theme());
        let spans = &lines[0].spans;
        let find = |t: &str| spans.iter().find(|s| s.content == t).unwrap().style;
        assert!(find("b").add_modifier.contains(Modifier::BOLD));
        assert!(find("c").add_modifier.contains(Modifier::ITALIC));
        assert_eq!(find("d").bg, Some(theme().code_bg));
    }

    #[test]
    fn headings_are_bold() {
        let lines = render_markdown("# Title\n\nbody", 80, &theme());
        assert_eq!(texts(&lines), vec!["Title", "", "body"]);
        assert!(lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
    }

    #[test]
    fn lists_get_markers_and_hanging_indent() {
        let out = render("- one\n- two words that wrap around\n\n1. a\n2. b", 16);
        assert_eq!(out[0], "•  one");
        assert_eq!(out[1], "•  two words");
        assert_eq!(out[2], "   that wrap");
        assert_eq!(out[3], "   around");
        assert_eq!(out[4], "");
        assert_eq!(out[5], "1. a");
        assert_eq!(out[6], "2. b");
    }

    #[test]
    fn nested_lists_indent() {
        let out = render("- outer\n  - inner", 40);
        assert_eq!(out, vec!["•  outer", "   ◦  inner"]);
    }

    #[test]
    fn task_lists() {
        let out = render("- [x] done\n- [ ] todo", 40);
        assert_eq!(out, vec!["[x] done", "[ ] todo"]);
    }

    #[test]
    fn code_blocks_are_boxed_and_never_exceed_width() {
        let md = "```rust\nfn main() { println!(\"hello world, this is long\"); }\n```";
        let lines = render_markdown(md, 30, &theme());
        let out = texts(&lines);
        assert!(out[0].starts_with("╭─ rust "));
        assert!(out[1].starts_with("│ fn main()"));
        assert!(out.last().unwrap().starts_with("╰"));
        for l in &out {
            assert!(str_width(l) <= 30, "line too wide: {l:?}");
        }
    }

    #[test]
    fn unterminated_fence_still_renders_as_code() {
        let out = render("text\n\n```py\nprint(1)", 40);
        assert_eq!(out[0], "text");
        assert!(out.iter().any(|l| l.starts_with("│ print(1)")));
    }

    #[test]
    fn links_show_url() {
        let out = render("see [docs](https://example.com)", 80);
        assert_eq!(out, vec!["see docs (https://example.com)"]);
        let out = render("<https://example.com>", 80);
        assert_eq!(out, vec!["https://example.com"]);
    }

    #[test]
    fn blockquotes_have_bar() {
        let out = render("> quoted", 40);
        assert_eq!(out, vec!["▎ quoted"]);
    }

    #[test]
    fn tables_align_columns() {
        let out = render("| a | bb |\n|---|---:|\n| ccc | d |", 40);
        assert_eq!(out[0], "a   │ bb");
        assert_eq!(out[1], "────┼───");
        assert_eq!(out[2], "ccc │  d");
    }

    #[test]
    fn wide_tables_are_shrunk() {
        let md =
            "| col | other |\n|---|---|\n| aaaaaaaaaaaaaaaaaaaaaa | bbbbbbbbbbbbbbbbbbbbbbbb |";
        for l in render(md, 24) {
            assert!(str_width(&l) <= 24, "{l:?}");
        }
    }

    #[test]
    fn every_line_fits_width() {
        let md = "# Heading that is quite long indeed\n\nSome paragraph text with `inline code` and **bold** words that go on.\n\n- list item that also wraps across lines\n  1. nested numbered item text\n\n> a quote that wraps across the width\n\n---";
        for w in [12, 20, 33] {
            for l in render(md, w) {
                assert!(str_width(&l) <= w, "width {w}: {l:?}");
            }
        }
    }

    #[test]
    fn extracts_last_code_block() {
        let md = "```\nfirst\n```\ntext\n```rs\nsecond\n```";
        assert_eq!(last_code_block(md).as_deref(), Some("second\n"));
        assert_eq!(last_code_block("no code"), None);
    }

    #[test]
    fn highlighting_produces_spans_per_line() {
        let lines = highlight_code("let a = 1;\nlet b = 2;", "rust", &theme());
        assert_eq!(lines.len(), 2);
        let joined: String = lines[0].iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "let a = 1;");
    }
}
