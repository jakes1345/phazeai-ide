use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::theme::Theme;

// ── Companion character ────────────────────────────────────────────────

const IDLE_MESSAGES: &[&str] = &[
    "what are we building today?",
    "i believe in you",
    "type something, i dare you",
    "another day, another diff",
    "let's ship it",
    "ready when you are",
    "got coffee? you'll need it",
    "the codebase isn't gonna fix itself",
    "i'm watching... no pressure",
    "*stretches* ok let's go",
    "tabs or spaces? just kidding",
    "git push --force? bold move",
    "semicolons are optional* (*not really)",
    "may your builds be green",
    "bugs fear us",
    "zero warnings, zero mercy",
];

const THINKING_MESSAGES: &[&str] = &[
    "hmm let me think...",
    "working on it...",
    "crunching tokens...",
    "reading your code rn...",
    "oh this is interesting...",
    "one sec...",
    "hold on, cooking...",
    "the AI is doing AI things",
    "neurons firing...",
    "beep boop beep",
    "trust the process",
    "*intense staring at code*",
];

const TOOL_MESSAGES: &[&str] = &[
    "ooh tools!",
    "running things...",
    "hope that works lol",
    "executing... fingers crossed",
    "let's see what happens",
    "doing the thing",
    "danger zone, exciting",
];

const SUCCESS_MESSAGES: &[&str] = &[
    "nice!",
    "nailed it",
    "clean.",
    "ez",
    "we're so good at this",
    "ship it!",
    "that's what i'm talking about",
    "W",
];

const ERROR_MESSAGES: &[&str] = &[
    "oof",
    "that's fine, we got this",
    "happens to the best of us",
    "try again?",
    "it's not a bug, it's a feature",
    "stack overflow time",
    "rubber duck says: read the error",
];

const APPROVAL_MESSAGES: &[&str] = &[
    "your call, boss",
    "do you trust it? i trust it",
    "to approve or not to approve",
    "check before you wreck",
    "safety first!",
];

const GREETING_MESSAGES: &[&str] = &[
    "hey! welcome back",
    "oh hi! let's build",
    "good to see you!",
    "the phaze begins",
    "ready to roll?",
];

/// Tracks companion state for contextual reactions
pub struct Companion {
    /// Current message to display
    current_msg: String,
    /// What state triggered the current message
    state: CompanionState,
    /// Counter for picking varied messages
    tick: u64,
    /// Timestamp of last state change (for timed idle transitions)
    last_change: std::time::Instant,
    /// How many messages the user has sent this session
    user_msg_count: u32,
}

#[derive(PartialEq, Clone, Copy)]
enum CompanionState {
    Greeting,
    Idle,
    Thinking,
    ToolRunning,
    Success,
    Error,
    Approval,
}

impl Default for Companion {
    fn default() -> Self {
        Self::new()
    }
}

impl Companion {
    pub fn new() -> Self {
        let tick = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let msg = pick(GREETING_MESSAGES, tick);
        Self {
            current_msg: msg.to_string(),
            state: CompanionState::Greeting,
            tick,
            last_change: std::time::Instant::now(),
            user_msg_count: 0,
        }
    }

    /// Call when user sends a message
    pub fn on_user_message(&mut self) {
        self.user_msg_count += 1;
        self.set_state(CompanionState::Thinking);
    }

    /// Call when AI starts processing
    pub fn on_thinking(&mut self) {
        self.set_state(CompanionState::Thinking);
    }

    /// Call when a tool starts running
    pub fn on_tool_start(&mut self) {
        self.set_state(CompanionState::ToolRunning);
    }

    /// Call when AI completes successfully
    pub fn on_complete(&mut self) {
        self.set_state(CompanionState::Success);
    }

    /// Call when an error occurs
    pub fn on_error(&mut self) {
        self.set_state(CompanionState::Error);
    }

    /// Call when approval is needed
    pub fn on_approval(&mut self) {
        self.set_state(CompanionState::Approval);
    }

    /// Call when processing finishes and we're back to idle
    pub fn on_idle(&mut self) {
        // Don't immediately switch to idle — let success/error messages linger
        if self.last_change.elapsed() > std::time::Duration::from_secs(8) {
            self.set_state(CompanionState::Idle);
        }
    }

    /// Advance tick (call each frame for varied messages in long states)
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        // Auto-transition to idle after lingering on success/error
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
            self.current_msg = pick(IDLE_MESSAGES, self.tick).to_string();
        }
    }

    fn set_state(&mut self, state: CompanionState) {
        if self.state == state {
            return;
        }
        self.state = state;
        self.last_change = std::time::Instant::now();
        self.current_msg = match state {
            CompanionState::Greeting => pick(GREETING_MESSAGES, self.tick),
            CompanionState::Idle => pick(IDLE_MESSAGES, self.tick),
            CompanionState::Thinking => pick(THINKING_MESSAGES, self.tick),
            CompanionState::ToolRunning => pick(TOOL_MESSAGES, self.tick),
            CompanionState::Success => pick(SUCCESS_MESSAGES, self.tick),
            CompanionState::Error => pick(ERROR_MESSAGES, self.tick),
            CompanionState::Approval => pick(APPROVAL_MESSAGES, self.tick),
        }
        .to_string();
    }

    pub fn message(&self) -> &str {
        &self.current_msg
    }
}

/// Pick a message pseudo-randomly from a pool based on tick
fn pick(pool: &'static [&'static str], tick: u64) -> &'static str {
    pool[(tick as usize) % pool.len()]
}

// ── Rendering ──────────────────────────────────────────────────────────

/// The companion character as ASCII art (small, fits in 3 lines)
const PHAZE_FACE: [&str; 3] = [
    " ◉‿◉ ", // normal face
    " ◉_◉ ", // thinking face
    " ◉△◉ ", // surprised face
];

/// Draw the companion widget: character + speech bubble next to the input area.
/// Returns the Rect consumed so the caller can shrink the input area.
///
/// Layout:  [ face | speech bubble ............... ]
///          Takes 2 rows of height, rendered ABOVE the input box.
pub fn draw_companion(f: &mut ratatui::Frame, area: Rect, companion: &Companion, theme: &Theme) {
    if area.width < 20 || area.height < 2 {
        return; // too small to render
    }

    let face_idx = match companion.state {
        CompanionState::Thinking | CompanionState::ToolRunning => 1,
        CompanionState::Error | CompanionState::Approval => 2,
        _ => 0,
    };

    let face_color = match companion.state {
        CompanionState::Success => theme.success,
        CompanionState::Error => theme.error,
        CompanionState::Thinking | CompanionState::ToolRunning => theme.accent,
        CompanionState::Approval => theme.warning,
        _ => theme.muted,
    };

    let msg = companion.message();

    // Build the two-line widget:
    // Line 1:  ◉‿◉  ╭─ message ─╮
    // Line 2:       ╰────────────╯
    let face = PHAZE_FACE[face_idx];
    let max_bubble = (area.width as usize).saturating_sub(8); // face + padding
    let display_msg: String = if msg.len() > max_bubble {
        format!("{}…", &msg[..max_bubble.saturating_sub(1)])
    } else {
        msg.to_string()
    };

    let bubble_inner_w = display_msg.len();
    let top_border = format!("╭{}╮", "─".repeat(bubble_inner_w + 2));
    let bot_border = format!("╰{}╯", "─".repeat(bubble_inner_w + 2));

    let lines = vec![
        Line::from(vec![
            Span::styled(
                face.to_string(),
                Style::default().fg(face_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(top_border, Style::default().fg(theme.dim)),
        ]),
        Line::from(vec![
            Span::styled(" ".repeat(face.len()), Style::default()),
            Span::styled("│ ", Style::default().fg(theme.dim)),
            Span::styled(display_msg, Style::default().fg(theme.fg)),
            Span::styled(" │", Style::default().fg(theme.dim)),
        ]),
        Line::from(vec![
            Span::styled(" ".repeat(face.len()), Style::default()),
            Span::styled(bot_border, Style::default().fg(theme.dim)),
        ]),
    ];

    let p = Paragraph::new(lines);
    f.render_widget(p, area);
}
