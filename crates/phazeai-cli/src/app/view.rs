//! Drawing. Layout, top to bottom:
//!
//! ```text
//!  ◆ phazeai  anthropic · claude-sonnet-4-5   ~/proj  git:main*        PLAN
//!  conversation (optionally with a file tree on the left)
//!  companion buddy (tall terminals only)
//! ╭──────────────────────────────────────────────────────────────────────╮
//! │ ❯ prompt…                                                             │
//! ╰──────────────────────────────────────────────────────────────────────╯
//!  enter send · shift+enter newline · / commands     ↑1.2k ↓3.4k · $0.01
//! ```

use std::time::Duration;

use phazeai_core::tools::{ToolApprovalMode, ToolPermission};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
    Frame,
};

use super::actions::fmt_tokens;
use super::chat::{needs_gap, spinner_frame, RenderCtx};
use super::keys::{approval_label, palette_matches};
use super::AppState;
use crate::commands::is_secret_input;
use crate::setup::provider_slug;
use crate::text::{spans_width, str_width, truncate_to_width, wrap_plain};
use crate::theme::Theme;
use crate::tool_view::{ApprovalPolicy, PreviewLine};

const NOTICE_TTL: Duration = Duration::from_secs(4);
const MAX_INPUT_ROWS: usize = 8;

fn home_relative(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    match dirs::home_dir() {
        Some(home) => home_relative_to(&s, &home.display().to_string()),
        None => s,
    }
}

/// `path` with a leading `home` replaced by `~`, but only when `home` is a
/// whole path component: `/home/me/x` → `~/x`, while `/home/mesa/x` is left
/// alone (it is not inside `/home/me`).
fn home_relative_to(path: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn cwd_display() -> String {
    std::env::current_dir()
        .map(|p| home_relative(&p))
        .unwrap_or_else(|_| ".".into())
}

/// Shorten a path to at most `max` cells by keeping trailing components:
/// `/home/me/src/project/crates/cli` → `…/crates/cli`.
fn short_path(path: &str, max: usize) -> String {
    if str_width(path) <= max {
        return path.to_string();
    }
    let sep = if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    };
    let mut out = String::new();
    for comp in path.rsplit(sep).filter(|c| !c.is_empty()) {
        let candidate = if out.is_empty() {
            comp.to_string()
        } else {
            format!("{comp}{sep}{out}")
        };
        if str_width(&candidate) + 2 > max {
            break;
        }
        out = candidate;
    }
    if out.is_empty() {
        truncate_to_width(path, max)
    } else {
        format!("…{sep}{out}")
    }
}

pub(super) fn draw(f: &mut Frame, state: &mut AppState) {
    let area = f.area();
    state.companion.tick();
    if !state.processing && state.pending_approval.is_none() {
        state.companion.on_idle();
    }

    let bottom_height = if state.pending_approval.is_some() {
        approval_height(state, area)
    } else {
        input_height(state, area.width)
    };
    // The buddy needs its sprite height plus a name line; only give it room
    // on tall terminals so the conversation always gets the space first.
    let buddy_h = state.companion.buddy.sprite(0).len() as u16 + 1;
    let show_companion = area.height >= 40 && area.width >= 60;
    let companion_h = if show_companion { buddy_h.max(5) } else { 0 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(companion_h),
            Constraint::Length(bottom_height),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, chunks[0], state);

    let chat_area = if state.show_files && chunks[1].width > 70 {
        let h = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(30), Constraint::Min(40)])
            .split(chunks[1]);
        draw_file_tree(f, h[0], &state.theme);
        h[1]
    } else {
        chunks[1]
    };
    draw_chat(f, chat_area, state);

    if show_companion {
        crate::companion::draw_companion(f, chunks[2], &state.companion, &state.theme);
    }

    if state.pending_approval.is_some() {
        draw_approval(f, chunks[3], state);
    } else {
        draw_input(f, chunks[3], state);
        draw_palette(f, chat_area, chunks[3], state);
    }
    draw_hint_bar(f, chunks[4], state);

    if state.session_picker.is_some() {
        draw_session_picker(f, area, state);
    }
}

// ── Header ──────────────────────────────────────────────────────────────

fn draw_header(f: &mut Frame, area: Rect, state: &AppState) {
    let t = &state.theme;
    let bar = Style::default().bg(t.surface);
    let badge = if t.mode.has_color() {
        Style::default()
            .fg(t.header_fg)
            .bg(t.header_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
    };

    let mut left = vec![
        Span::styled(" ◆ phazeai ", badge),
        Span::styled("  ", bar),
        Span::styled(provider_slug(&state.settings.llm.provider), bar.fg(t.muted)),
        Span::styled(" · ", bar.fg(t.dim)),
        Span::styled(
            state.settings.llm.model.clone(),
            bar.fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   ", bar),
        Span::styled(
            short_path(&cwd_display(), (area.width as usize / 3).max(12)),
            bar.fg(t.muted),
        ),
    ];
    if let Some(branch) = &state.git_branch {
        left.push(Span::styled("  git:", bar.fg(t.dim)));
        left.push(Span::styled(branch.clone(), bar.fg(t.muted)));
        if state.git_dirty > 0 {
            left.push(Span::styled("*", bar.fg(t.warning)));
        }
    }

    let mut right: Vec<Span> = Vec::new();
    if state.ai_mode != "chat" {
        right.push(Span::styled(
            format!(" {} ", state.ai_mode.to_uppercase()),
            Style::default()
                .fg(t.header_fg)
                .bg(t.warning)
                .add_modifier(Modifier::BOLD),
        ));
        right.push(Span::styled(" ", bar));
    }
    if state.approval_mode() == ToolApprovalMode::AutoApprove {
        right.push(Span::styled(
            " AUTO-APPROVE ",
            Style::default()
                .fg(t.header_fg)
                .bg(t.error)
                .add_modifier(Modifier::BOLD),
        ));
        right.push(Span::styled(" ", bar));
    }

    let width = area.width as usize;
    let right_w = spans_width(&right);
    let left = crate::text::truncate_spans(&left, width.saturating_sub(right_w));
    let pad = width.saturating_sub(spans_width(&left) + right_w);
    let mut spans = left;
    spans.push(Span::styled(" ".repeat(pad), bar));
    spans.extend(right);
    f.render_widget(Paragraph::new(Line::from(spans)).style(bar), area);
}

// ── Conversation ────────────────────────────────────────────────────────

fn welcome_lines(state: &AppState, width: usize) -> Vec<Line<'static>> {
    let t = &state.theme;
    let key = |k: &str| {
        Span::styled(
            k.to_string(),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        )
    };
    let dim = |s: &str| Span::styled(s.to_string(), Style::default().fg(t.muted));
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                "  ◆ PhazeAI",
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            dim(&format!("  v{}", env!("CARGO_PKG_VERSION"))),
        ]),
        Line::from(vec![dim(&format!(
            "  {} · {} · {}",
            state.settings.llm.provider.to_provider_id().name(),
            state.settings.llm.model,
            short_path(&cwd_display(), width.saturating_sub(40).max(12)),
        ))]),
        Line::raw(""),
        Line::from(dim("  Ask anything about your code. A few things to try:")),
        Line::raw(""),
    ];
    for (cmd, what) in [
        ("/help   ", "every command and keyboard shortcut"),
        ("/models ", "see and switch models"),
        ("/status ", "provider, API key, tokens and cost"),
        ("/mode   ", "chat · ask · plan · debug · edit"),
    ] {
        lines.push(Line::from(vec![Span::raw("    "), key(cmd), dim(what)]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(dim(
        "  Tip: press shift+enter (or end a line with \\) for a newline.",
    )));
    lines
}

fn draw_chat(f: &mut Frame, area: Rect, state: &mut AppState) {
    if area.width < 4 || area.height < 1 {
        return;
    }
    // 1 column padding on the left, 1 for the scrollbar on the right.
    let inner = Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );
    let width = inner.width as usize;
    let height = inner.height as usize;
    state.view_height = height;

    let theme = state.theme.clone();
    let ctx = RenderCtx {
        theme: &theme,
        theme_gen: state.theme_gen,
        width,
        expand_tools: state.expand_tools,
        spinner: spinner_frame(),
    };

    // Trailing status lines (not part of the transcript).
    let mut tail: Vec<Line<'static>> = Vec::new();
    if state.items.is_empty() && !state.processing {
        tail = welcome_lines(state, width);
    }
    if state.processing && state.pending_approval.is_none() {
        let elapsed = state
            .run_started
            .map(|s| s.elapsed().as_secs())
            .unwrap_or(0);
        let hint = if state.cancel_requested {
            String::new()
        } else {
            " · esc to interrupt".into()
        };
        tail.push(Line::raw(""));
        tail.push(Line::from(vec![
            Span::styled(
                format!("{} ", ctx.spinner),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}…", state.status_text),
                Style::default().fg(theme.accent),
            ),
            Span::styled(
                format!(" ({elapsed}s{hint})"),
                Style::default().fg(theme.dim),
            ),
        ]));
    }
    for q in &state.queued {
        tail.extend(wrap_plain(
            &format!("↳ queued: {}", q.lines().next().unwrap_or("")),
            width,
            Style::default().fg(theme.muted),
            "  ",
        ));
    }

    // Pass 1: heights.
    let mut total = 0usize;
    let mut heights = Vec::with_capacity(state.items.len());
    for i in 0..state.items.len() {
        let gap = {
            let (before, rest) = state.items.split_at(i);
            usize::from(needs_gap(before.last(), &rest[0]))
        };
        let h = state.items[i].lines(&ctx).len() + gap;
        heights.push((gap, h));
        total += h;
    }
    total += tail.len();

    let max_scroll = total.saturating_sub(height);
    if state.follow || state.scroll >= max_scroll {
        state.scroll = max_scroll;
        state.follow = true;
    }
    let top = state.scroll;
    let bottom = top + height;

    // Pass 2: collect only the visible window.
    let mut visible: Vec<Line<'static>> = Vec::with_capacity(height);
    let mut y = 0usize;
    for (i, (gap, h)) in heights.iter().enumerate() {
        if y + h <= top {
            y += h;
            continue;
        }
        if y >= bottom {
            break;
        }
        if *gap == 1 {
            if y >= top && y < bottom {
                visible.push(Line::raw(""));
            }
            y += 1;
        }
        for line in state.items[i].lines(&ctx) {
            if y >= top && y < bottom {
                visible.push(line.clone());
            }
            y += 1;
        }
    }
    for line in tail {
        if y >= top && y < bottom {
            visible.push(line);
        }
        y += 1;
    }

    f.render_widget(Paragraph::new(visible), inner);

    if total > height {
        let mut sb = ScrollbarState::new(max_scroll).position(state.scroll);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::default().fg(theme.border))
                .thumb_symbol("┃")
                .thumb_style(Style::default().fg(theme.muted)),
            area,
            &mut sb,
        );
        if !state.follow {
            let below = max_scroll - state.scroll;
            let label = format!(" ↓ {below} more lines · ctrl+end ");
            let w = str_width(&label) as u16;
            if area.width > w + 2 {
                let r = Rect::new(area.x + area.width - w - 2, area.y + area.height - 1, w, 1);
                f.render_widget(
                    Paragraph::new(Span::styled(
                        label,
                        Style::default().fg(theme.header_fg).bg(theme.muted),
                    )),
                    r,
                );
            }
        }
    }
}

// ── Input ───────────────────────────────────────────────────────────────

fn display_buffer(state: &AppState) -> crate::input::InputBuffer {
    if is_secret_input(state.input.text()) {
        state.input.masked("/key ".len())
    } else {
        state.input.clone()
    }
}

fn input_height(state: &AppState, width: u16) -> u16 {
    // Borders, horizontal padding and the "❯ " prompt.
    let inner = (width as usize).saturating_sub(6);
    let (rows, _, _) = display_buffer(state).layout(inner);
    rows.len().clamp(1, MAX_INPUT_ROWS) as u16 + 2
}

fn draw_input(f: &mut Frame, area: Rect, state: &mut AppState) {
    let t = state.theme.clone();
    let border = if state.processing {
        t.border
    } else {
        t.input_active
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(ratatui::widgets::Padding::horizontal(1))
        .border_style(Style::default().fg(border));
    if state.ai_mode != "chat" {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {} mode ", state.ai_mode),
                Style::default().fg(t.warning),
            ))
            .alignment(Alignment::Right),
        );
    }
    if is_secret_input(state.input.text()) {
        block = block.title(Span::styled(
            " key is hidden and won't be saved to history ",
            Style::default().fg(t.muted),
        ));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 3 || inner.height < 1 {
        return;
    }

    let text_w = (inner.width as usize).saturating_sub(2);
    let prompt_style = Style::default()
        .fg(if state.processing { t.muted } else { t.accent })
        .add_modifier(Modifier::BOLD);

    if state.input.is_empty() {
        let placeholder = if state.processing {
            "Type to queue a follow-up…"
        } else {
            "Ask anything, or type / for commands"
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("❯ ", prompt_style),
                Span::styled(
                    truncate_to_width(placeholder, text_w),
                    Style::default().fg(t.dim),
                ),
            ])),
            inner,
        );
        f.set_cursor_position((inner.x + 2, inner.y));
        return;
    }

    let buf = display_buffer(state);
    let (rows, cur_row, cur_col) = buf.layout(text_w);
    let visible = inner.height as usize;
    if cur_row < state.input_scroll {
        state.input_scroll = cur_row;
    } else if cur_row >= state.input_scroll + visible {
        state.input_scroll = cur_row + 1 - visible;
    }
    let slash = state.input.text().starts_with('/');
    let text_style = if slash {
        Style::default().fg(t.accent)
    } else {
        Style::default().fg(t.fg)
    };
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(state.input_scroll)
        .take(visible)
        .map(|(i, row)| {
            let lead = if i == 0 {
                Span::styled("❯ ", prompt_style)
            } else {
                Span::raw("  ")
            };
            Line::from(vec![lead, Span::styled(row.clone(), text_style)])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);

    let x = inner.x + 2 + cur_col as u16;
    let y = inner.y + (cur_row - state.input_scroll) as u16;
    f.set_cursor_position((x.min(inner.x + inner.width - 1), y));
}

fn draw_palette(f: &mut Frame, chat: Rect, input: Rect, state: &mut AppState) {
    let matches = palette_matches(state);
    if matches.is_empty() {
        return;
    }
    let t = &state.theme;
    if state.palette_index >= matches.len() {
        state.palette_index = 0;
    }
    let rows = matches.len().min(8);
    let height = rows as u16 + 2;
    if chat.height < height {
        return;
    }
    let width = input.width.min(90);
    let rect = Rect::new(input.x, input.y.saturating_sub(height), width, height);

    // Keep the selection in view.
    let start = state.palette_index.saturating_sub(rows - 1);
    let name_w = matches
        .iter()
        .map(|c| str_width(c.name) + 1 + str_width(c.args))
        .max()
        .unwrap_or(10)
        .min(36);
    let inner_w = width.saturating_sub(2) as usize;
    let lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, c)| {
            let selected = i == state.palette_index;
            let base = if selected {
                if t.mode.has_color() {
                    Style::default().bg(t.surface)
                } else {
                    Style::default().add_modifier(Modifier::REVERSED)
                }
            } else {
                Style::default()
            };
            let synopsis = if c.args.is_empty() {
                c.name.to_string()
            } else {
                format!("{} {}", c.name, c.args)
            };
            let synopsis = truncate_to_width(&synopsis, name_w);
            let pad = name_w.saturating_sub(str_width(&synopsis)) + 2;
            let help_w = inner_w.saturating_sub(name_w + 4);
            let mut spans = vec![
                Span::styled(if selected { "› " } else { "  " }, base.fg(t.accent)),
                Span::styled(
                    synopsis,
                    base.fg(if selected { t.accent } else { t.fg })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ".repeat(pad), base),
                Span::styled(truncate_to_width(c.help, help_w), base.fg(t.muted)),
            ];
            let used = spans_width(&spans);
            if used < inner_w {
                spans.push(Span::styled(" ".repeat(inner_w - used), base));
            }
            Line::from(spans)
        })
        .collect();

    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(t.border_focused))
                .title_bottom(
                    Line::from(Span::styled(
                        " ↑↓ select · tab complete · enter run · esc close ",
                        Style::default().fg(t.dim),
                    ))
                    .alignment(Alignment::Right),
                ),
        ),
        rect,
    );
}

// ── Approval prompt ─────────────────────────────────────────────────────

fn approval_height(state: &AppState, area: Rect) -> u16 {
    let preview = state
        .pending_approval
        .as_ref()
        .map(|p| p.request.preview.len())
        .unwrap_or(0);
    let max = (area.height as usize * 3 / 5).max(7);
    (preview + 4).clamp(5, max) as u16
}

fn preview_line(line: &PreviewLine, width: usize, t: &Theme) -> Line<'static> {
    let (prefix, text, style) = match line {
        PreviewLine::Command(s) => (
            "$ ",
            s,
            Style::default().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        PreviewLine::Add(s) => ("+ ", s, Style::default().fg(t.success)),
        PreviewLine::Remove(s) => ("- ", s, Style::default().fg(t.error)),
        PreviewLine::Context(s) => ("  ", s, Style::default().fg(t.muted)),
        PreviewLine::Hunk(s) => ("", s, Style::default().fg(t.accent)),
        PreviewLine::Note(s) => (
            "",
            s,
            Style::default().fg(t.muted).add_modifier(Modifier::ITALIC),
        ),
    };
    let body = truncate_to_width(
        &crate::text::sanitize(text),
        width.saturating_sub(prefix.len()),
    );
    Line::from(vec![
        Span::styled(prefix.to_string(), style),
        Span::styled(body, style),
    ])
}

fn draw_approval(f: &mut Frame, area: Rect, state: &mut AppState) {
    let t = state.theme.clone();
    let Some(pending) = state.pending_approval.as_mut() else {
        return;
    };
    let req = &pending.request;
    let (risk_label, risk_color) = match req.risk {
        ToolPermission::ReadOnly => ("read-only", t.success),
        ToolPermission::Write => ("writes files", t.warning),
        ToolPermission::Execute => ("runs commands", t.warning),
        ToolPermission::Destructive => ("DESTRUCTIVE", t.error),
    };
    let target = match req.name.as_str() {
        "bash" => String::new(),
        _ => {
            let l = crate::tool_view::call_label(&req.name, &req.params);
            if l.is_empty() {
                l
            } else {
                format!(" {l}")
            }
        }
    };
    let title = Line::from(vec![
        Span::styled(" Allow ", Style::default().fg(t.fg)),
        Span::styled(
            req.name.clone(),
            Style::default().fg(t.warning).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            truncate_to_width(&target, (area.width as usize).saturating_sub(30)),
            Style::default().fg(t.fg),
        ),
        Span::styled("? ", Style::default().fg(t.fg)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(risk_color))
        .title(title)
        .title(
            Line::from(Span::styled(
                format!(" {risk_label} "),
                Style::default().fg(risk_color).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Right),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 2 {
        return;
    }

    let width = inner.width.saturating_sub(1) as usize;
    let body_rows = inner.height.saturating_sub(2) as usize;
    let total = req.preview.len();
    let max_scroll = total.saturating_sub(body_rows);
    pending.scroll = pending.scroll.min(max_scroll);
    let mut lines: Vec<Line> = req
        .preview
        .iter()
        .skip(pending.scroll)
        .take(body_rows)
        .map(|l| preview_line(l, width, &t))
        .collect();
    while lines.len() < body_rows {
        lines.push(Line::raw(""));
    }

    let key = |k: &str, c| {
        Span::styled(
            format!(" {k} "),
            Style::default()
                .fg(t.header_fg)
                .bg(c)
                .add_modifier(Modifier::BOLD),
        )
    };
    let label = |s: String| Span::styled(s, Style::default().fg(t.fg));
    let always = ApprovalPolicy::describe(&req.name, &req.params);
    let build = |always_label: String| {
        vec![
            key("y", t.success),
            label(" yes  ".into()),
            key("a", t.accent),
            label(always_label),
            key("n", t.error),
            label(" no  ".into()),
            key("esc", t.muted),
            label(" stop".into()),
        ]
    };
    let mut footer = build(format!(" always allow {always}  "));
    // On narrow terminals keep every choice visible rather than the detail.
    if spans_width(&footer) > width {
        footer = build(" always  ".into());
    }
    if max_scroll > 0 {
        footer.push(Span::styled(
            format!(
                "   ↑↓ scroll {}/{}",
                pending.scroll + body_rows.min(total),
                total
            ),
            Style::default().fg(t.dim),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(crate::text::truncate_spans(
        &footer,
        inner.width as usize,
    )));
    f.render_widget(
        Paragraph::new(lines),
        Rect::new(
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(1),
            inner.height,
        ),
    );
}

// ── Hint / status bar ───────────────────────────────────────────────────

fn draw_hint_bar(f: &mut Frame, area: Rect, state: &AppState) {
    let t = &state.theme;
    let k = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default().fg(t.muted).add_modifier(Modifier::BOLD),
        )
    };
    let d = |s: &str| Span::styled(s.to_string(), Style::default().fg(t.dim));

    let mut left: Vec<Span> = vec![Span::raw(" ")];
    let armed = state
        .quit_armed_at
        .is_some_and(|at| at.elapsed() < Duration::from_secs(2));
    let notice = state
        .notice
        .as_ref()
        .filter(|(_, at)| at.elapsed() < NOTICE_TTL)
        .map(|(n, _)| n.clone());

    if armed {
        left.push(Span::styled(
            "Press Ctrl+C again to exit",
            Style::default().fg(t.warning).add_modifier(Modifier::BOLD),
        ));
    } else if let Some(n) = notice {
        left.push(Span::styled(n, Style::default().fg(t.accent)));
    } else if state.pending_approval.is_some() {
        left.extend([
            k("y"),
            d(" yes  "),
            k("a"),
            d(" always  "),
            k("n"),
            d(" no  "),
            k("esc"),
            d(" stop"),
        ]);
    } else if state.processing {
        left.extend([
            k("esc"),
            d(" interrupt  "),
            k("enter"),
            d(" queue message  "),
            k("pgup/pgdn"),
            d(" scroll"),
        ]);
    } else {
        left.extend([
            k("enter"),
            d(" send  "),
            k("shift+enter"),
            d(" newline  "),
            k("↑"),
            d(" history  "),
            k("/"),
            d(" commands  "),
            k("ctrl+t"),
            d(" tool details  "),
            k("ctrl+c"),
            d(" quit"),
        ]);
    }

    let sep = || Span::styled(" · ", Style::default().fg(t.dim));
    let mut right: Vec<Span> = Vec::new();
    if state.tokens_in + state.tokens_out > 0 {
        right.push(Span::styled(
            format!(
                "↑{} ↓{}",
                fmt_tokens(state.tokens_in),
                fmt_tokens(state.tokens_out)
            ),
            Style::default().fg(t.muted),
        ));
    }
    if state.cost > 0.0 {
        if !right.is_empty() {
            right.push(sep());
        }
        right.push(Span::styled(
            format!("${:.4}", state.cost),
            Style::default().fg(t.muted),
        ));
    }
    if let Some(ctx) = state.context_tokens.filter(|c| *c > 0) {
        if !right.is_empty() {
            right.push(sep());
        }
        let pct = ctx * 100 / state.context_budget.max(1);
        let color = if pct >= 90 {
            t.error
        } else if pct >= 70 {
            t.warning
        } else {
            t.muted
        };
        right.push(Span::styled(
            format!("ctx {pct}%"),
            Style::default().fg(color),
        ));
    }
    if !right.is_empty() {
        right.push(sep());
    }
    right.push(Span::styled(
        approval_label(&state.approval_mode()),
        Style::default().fg(t.dim),
    ));
    right.push(Span::raw(" "));

    let width = area.width as usize;
    let right_w = spans_width(&right);
    let left = crate::text::truncate_spans(&left, width.saturating_sub(right_w + 1));
    let pad = width.saturating_sub(spans_width(&left) + right_w);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(right);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ── Overlays ────────────────────────────────────────────────────────────

fn draw_session_picker(f: &mut Frame, area: Rect, state: &AppState) {
    let Some(picker) = state.session_picker.as_ref() else {
        return;
    };
    let t = &state.theme;
    let rect = picker_rect(area, picker.list.len());
    let rows = rect.height.saturating_sub(2) as usize;
    let start = picker.index.saturating_sub(rows.saturating_sub(1));
    let inner_w = rect.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = picker
        .list
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, m)| {
            let sel = i == picker.index;
            let meta = format!(
                "  {} · {} msgs · {}",
                &m.id[..8.min(m.id.len())],
                m.message_count,
                m.updated_at
            );
            let title_w = inner_w.saturating_sub(str_width(&meta) + 3);
            let title = truncate_to_width(&m.title, title_w);
            let base = if sel {
                if t.mode.has_color() {
                    Style::default().bg(t.surface)
                } else {
                    Style::default().add_modifier(Modifier::REVERSED)
                }
            } else {
                Style::default()
            };
            let pad = title_w.saturating_sub(str_width(&title));
            Line::from(vec![
                Span::styled(if sel { " › " } else { "   " }, base.fg(t.accent)),
                Span::styled(
                    title,
                    if sel {
                        base.fg(t.accent).add_modifier(Modifier::BOLD)
                    } else {
                        base.fg(t.fg)
                    },
                ),
                Span::styled(" ".repeat(pad), base),
                Span::styled(meta, base.fg(t.muted)),
            ])
        })
        .collect();
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(t.border_focused))
                .title(Span::styled(
                    " Saved sessions ",
                    Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
                ))
                .title_bottom(
                    Line::from(Span::styled(
                        " ↑↓ select · enter load · esc close ",
                        Style::default().fg(t.dim),
                    ))
                    .alignment(Alignment::Center),
                ),
        ),
        rect,
    );
}

/// Where the session picker goes: centred, 30–90 columns wide and tall enough
/// for `items` rows, but never larger than `area`. Drawing outside the frame
/// panics in ratatui, so a tiny terminal gets a tiny picker instead.
fn picker_rect(area: Rect, items: usize) -> Rect {
    let w = area.width.saturating_sub(4).clamp(30, 90).min(area.width);
    let wanted = u16::try_from(items).unwrap_or(u16::MAX).saturating_add(2);
    let h = wanted
        .clamp(5, area.height.saturating_sub(4).max(5))
        .min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn draw_file_tree(f: &mut Frame, area: Rect, theme: &Theme) {
    let cwd = std::env::current_dir().ok();
    let name = cwd
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".into());
    let width = area.width.saturating_sub(2) as usize;
    let rows = area.height.saturating_sub(2) as usize;

    let hidden = |n: &str| n.starts_with('.') || n == "target" || n == "node_modules";
    let mut lines: Vec<Line> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(".") {
        let mut entries: Vec<_> = entries
            .flatten()
            .filter(|e| !hidden(&e.file_name().to_string_lossy()))
            .collect();
        entries.sort_by_key(|e| {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (!is_dir, e.file_name())
        });
        let total = entries.len();
        for (i, entry) in entries.iter().enumerate() {
            if lines.len() >= rows {
                break;
            }
            let fname = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let last = i + 1 == total;
            let conn = if last { "└─ " } else { "├─ " };
            let label = if is_dir { format!("{fname}/") } else { fname };
            lines.push(Line::from(vec![
                Span::styled(conn, Style::default().fg(theme.dim)),
                Span::styled(
                    truncate_to_width(&label, width.saturating_sub(3)),
                    Style::default().fg(if is_dir { theme.accent } else { theme.fg }),
                ),
            ]));
            if is_dir {
                if let Ok(sub) = std::fs::read_dir(entry.path()) {
                    let mut sub: Vec<_> = sub
                        .flatten()
                        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                        .collect();
                    sub.sort_by_key(|e| e.file_name());
                    let shown = sub.len().min(8);
                    let bar = if last { "   " } else { "│  " };
                    for (j, s) in sub.iter().take(shown).enumerate() {
                        if lines.len() >= rows {
                            break;
                        }
                        let sname = s.file_name().to_string_lossy().to_string();
                        let sdir = s.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        let conn = if j + 1 == shown && sub.len() <= shown {
                            "└─ "
                        } else {
                            "├─ "
                        };
                        let label = if sdir { format!("{sname}/") } else { sname };
                        lines.push(Line::from(vec![
                            Span::styled(format!("{bar}{conn}"), Style::default().fg(theme.dim)),
                            Span::styled(
                                truncate_to_width(&label, width.saturating_sub(6)),
                                Style::default().fg(if sdir { theme.accent } else { theme.muted }),
                            ),
                        ]));
                    }
                    if sub.len() > shown && lines.len() < rows {
                        lines.push(Line::from(Span::styled(
                            format!("{bar}└─ +{} more", sub.len() - shown),
                            Style::default().fg(theme.dim),
                        )));
                    }
                }
            }
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.border))
                .title(Span::styled(
                    format!(" {name} "),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::{home_relative_to, picker_rect, short_path};
    use ratatui::layout::Rect;

    #[test]
    fn picker_rect_never_leaves_the_frame() {
        for (w, h) in [
            (0, 0),
            (1, 1),
            (10, 3),
            (29, 4),
            (30, 5),
            (80, 24),
            (200, 60),
        ] {
            let area = Rect::new(0, 0, w, h);
            for items in [0, 3, 50] {
                let r = picker_rect(area, items);
                assert_eq!(r, r.intersection(area), "{w}x{h} items={items}: {r:?}");
            }
        }
        // Roomy terminals: centred, 90 wide, list height plus the border.
        let r = picker_rect(Rect::new(2, 1, 100, 40), 3);
        assert_eq!((r.x, r.y, r.width, r.height), (7, 18, 90, 5));
        let r = picker_rect(Rect::new(0, 0, 100, 40), 100);
        assert_eq!(r.height, 36);
    }

    #[test]
    fn home_prefix_must_be_a_whole_component() {
        assert_eq!(
            home_relative_to("/home/me/project", "/home/me"),
            "~/project"
        );
        assert_eq!(home_relative_to("/home/me", "/home/me"), "~");
        assert_eq!(
            home_relative_to("/home/mesa/project", "/home/me"),
            "/home/mesa/project"
        );
        assert_eq!(home_relative_to("/opt/x", "/home/me"), "/opt/x");
        assert_eq!(home_relative_to("/x", "/"), "/x");
        assert_eq!(
            home_relative_to(r"C:\Users\me\code", r"C:\Users\me\"),
            r"~\code"
        );
        assert_eq!(
            home_relative_to(r"C:\Users\mesa\code", r"C:\Users\me"),
            r"C:\Users\mesa\code"
        );
    }

    #[test]
    fn short_path_keeps_trailing_components() {
        assert_eq!(short_path("~/src/proj", 20), "~/src/proj");
        assert_eq!(
            short_path("/home/me/src/project/crates/cli", 16),
            "…/crates/cli"
        );
        assert_eq!(short_path("C:\\Users\\me\\code\\app", 10), "…\\code\\app");
        assert_eq!(short_path("/averyveryverylongname", 8), "/averyv…");
    }
}
