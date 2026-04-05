use phazeai_core::companion::{self as buddy, Companion as BuddyData};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::theme::Theme;

// ── Companion state machine (CLI-specific rendering + transitions) ────

#[derive(PartialEq, Clone, Copy)]
pub enum CompanionState {
    Greeting,
    Idle,
    Thinking,
    ToolRunning,
    Success,
    Error,
    Approval,
}

pub struct Companion {
    /// The deterministic buddy data (species, rarity, eye, hat, name, stats)
    pub buddy: BuddyData,
    /// Current displayed message
    current_msg: String,
    /// Current state
    state: CompanionState,
    /// Tick counter for animation and message rotation
    tick: u64,
    /// Last state change timestamp
    last_change: std::time::Instant,
    /// User message count this session
    pub user_msg_count: u32,
}

impl Default for Companion {
    fn default() -> Self {
        Self::new()
    }
}

impl Companion {
    pub fn new() -> Self {
        let seed = buddy::user_seed();
        let buddy_data = buddy::generate(&seed);
        let tick = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let msg = buddy::pick_message(buddy::GREETING_MESSAGES, tick);

        Self {
            buddy: buddy_data,
            current_msg: msg.to_string(),
            state: CompanionState::Greeting,
            tick,
            last_change: std::time::Instant::now(),
            user_msg_count: 0,
        }
    }

    pub fn on_user_message(&mut self) {
        self.user_msg_count += 1;
        self.set_state(CompanionState::Thinking);
    }

    pub fn on_thinking(&mut self) {
        self.set_state(CompanionState::Thinking);
    }

    pub fn on_tool_start(&mut self) {
        self.set_state(CompanionState::ToolRunning);
    }

    pub fn on_complete(&mut self) {
        self.set_state(CompanionState::Success);
    }

    pub fn on_error(&mut self) {
        self.set_state(CompanionState::Error);
    }

    pub fn on_approval(&mut self) {
        self.set_state(CompanionState::Approval);
    }

    pub fn on_idle(&mut self) {
        if self.last_change.elapsed() > std::time::Duration::from_secs(8) {
            self.set_state(CompanionState::Idle);
        }
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        // Auto-transition to idle after lingering
        if matches!(
            self.state,
            CompanionState::Success | CompanionState::Error | CompanionState::Greeting
        ) && self.last_change.elapsed() > std::time::Duration::from_secs(12)
        {
            self.set_state(CompanionState::Idle);
        }

        // Rotate idle messages every ~30 seconds
        if self.state == CompanionState::Idle
            && self.last_change.elapsed() > std::time::Duration::from_secs(30)
        {
            self.last_change = std::time::Instant::now();
            self.current_msg = buddy::pick_message(buddy::IDLE_MESSAGES, self.tick).to_string();
        }
    }

    fn set_state(&mut self, state: CompanionState) {
        if self.state == state {
            return;
        }
        self.state = state;
        self.last_change = std::time::Instant::now();
        let pool = match state {
            CompanionState::Greeting => buddy::GREETING_MESSAGES,
            CompanionState::Idle => buddy::IDLE_MESSAGES,
            CompanionState::Thinking => buddy::THINKING_MESSAGES,
            CompanionState::ToolRunning => buddy::TOOL_MESSAGES,
            CompanionState::Success => buddy::SUCCESS_MESSAGES,
            CompanionState::Error => buddy::ERROR_MESSAGES,
            CompanionState::Approval => buddy::APPROVAL_MESSAGES,
        };
        self.current_msg = buddy::pick_message(pool, self.tick).to_string();
    }

    pub fn message(&self) -> &str {
        &self.current_msg
    }

    /// Current animation frame index (cycles every ~500ms based on tick)
    fn frame(&self) -> usize {
        (self.tick as usize / 4) % 3
    }
}

// ── Rendering ────────────────────────────────────────────────────────────────

/// Draw the companion: animated ASCII sprite + speech bubble + name/rarity badge.
pub fn draw_companion(f: &mut ratatui::Frame, area: Rect, companion: &Companion, theme: &Theme) {
    if area.width < 30 || area.height < 5 {
        return; // too small
    }

    let buddy = &companion.buddy;
    let sprite_lines = buddy.sprite(companion.frame());
    let sprite_width = sprite_lines.iter().map(|l| l.len()).max().unwrap_or(12);

    let face_color = match companion.state {
        CompanionState::Success => theme.success,
        CompanionState::Error => theme.error,
        CompanionState::Thinking | CompanionState::ToolRunning => theme.accent,
        CompanionState::Approval => theme.warning,
        _ => theme.muted,
    };

    let rarity_color = match buddy.rarity {
        buddy::Rarity::Common => theme.muted,
        buddy::Rarity::Uncommon => theme.success,
        buddy::Rarity::Rare => theme.accent,
        buddy::Rarity::Epic => theme.warning,
        buddy::Rarity::Legendary => theme.error,
    };

    let msg = companion.message();
    let bubble_max = (area.width as usize).saturating_sub(sprite_width + 6);
    let display_msg: String = if msg.len() > bubble_max {
        format!("{}…", &msg[..bubble_max.saturating_sub(1)])
    } else {
        msg.to_string()
    };

    let bubble_w = display_msg.len();
    let top_border = format!("╭{}╮", "─".repeat(bubble_w + 2));
    let bot_border = format!("╰{}╯", "─".repeat(bubble_w + 2));

    // Build name badge: "Quacky ★★★ (duck)"
    let badge = format!(
        "{} {} ({})",
        buddy.name,
        buddy.rarity.stars(),
        buddy.species.name()
    );

    let mut lines: Vec<Line> = Vec::new();

    // Line 0: name badge
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:width$}", "", width = sprite_width + 1),
            Style::default(),
        ),
        Span::styled(
            badge,
            Style::default()
                .fg(rarity_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Sprite lines with speech bubble alongside
    for (i, sprite_line) in sprite_lines.iter().enumerate() {
        let padded_sprite = format!("{:width$}", sprite_line, width = sprite_width);
        let mut spans = vec![Span::styled(
            format!("{padded_sprite} "),
            Style::default().fg(face_color),
        )];

        match i {
            0 => {
                spans.push(Span::styled(
                    top_border.clone(),
                    Style::default().fg(theme.dim),
                ));
            }
            1 => {
                spans.push(Span::styled("│ ", Style::default().fg(theme.dim)));
                spans.push(Span::styled(
                    display_msg.clone(),
                    Style::default().fg(theme.fg),
                ));
                spans.push(Span::styled(" │", Style::default().fg(theme.dim)));
            }
            2 => {
                spans.push(Span::styled(
                    bot_border.clone(),
                    Style::default().fg(theme.dim),
                ));
            }
            _ => {}
        }

        lines.push(Line::from(spans));
    }

    let p = Paragraph::new(lines);
    f.render_widget(p, area);
}
