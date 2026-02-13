use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use phazeai_core::{Agent, AgentEvent, Settings};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};
use std::io;
use tokio::sync::mpsc;

use crate::commands::{self, CommandResult};
use crate::theme::Theme;

// ── Single-prompt mode ──────────────────────────────────────────────────

pub async fn run_single_prompt(settings: &Settings, prompt: &str) -> Result<()> {
    let llm = settings.build_llm_client()?;
    let agent = Agent::new(llm).with_system_prompt(
        "You are PhazeAI, an AI coding assistant. Help the user with their request. \
         Be concise and direct.",
    );

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AgentEvent>();

    let agent_handle = tokio::spawn({
        let prompt = prompt.to_string();
        async move { agent.run_with_events(prompt, event_tx).await }
    });

    while let Some(event) = event_rx.recv().await {
        match event {
            AgentEvent::TextDelta(text) => print!("{text}"),
            AgentEvent::ToolStart { name } => eprintln!("\n[tool: {name}]"),
            AgentEvent::ToolResult {
                name,
                success,
                summary,
            } => {
                let icon = if success { "ok" } else { "err" };
                eprintln!("[{name}: {icon}] {summary}");
            }
            AgentEvent::Complete { .. } => println!(),
            AgentEvent::Error(e) => eprintln!("\nError: {e}"),
            _ => {}
        }
    }

    agent_handle.await??;
    Ok(())
}

// ── Interactive TUI ─────────────────────────────────────────────────────

struct ChatMessage {
    role: MessageRole,
    content: String,
}

#[derive(PartialEq)]
enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

struct AppState {
    input: String,
    cursor_pos: usize,
    messages: Vec<ChatMessage>,
    scroll: u16,
    is_processing: bool,
    status_text: String,
    model_name: String,
    should_quit: bool,
    show_files: bool,
    theme: Theme,
}

impl AppState {
    fn new(settings: &Settings, theme_name: &str) -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
            messages: vec![ChatMessage {
                role: MessageRole::System,
                content: "Welcome to PhazeAI. Type a message and press Enter. Ctrl+C to quit. /help for commands.".into(),
            }],
            scroll: 0,
            is_processing: false,
            status_text: "Ready".into(),
            model_name: settings.llm.model.clone(),
            should_quit: false,
            show_files: false,
            theme: Theme::by_name(theme_name),
        }
    }

    fn add_message(&mut self, role: MessageRole, content: String) {
        self.messages.push(ChatMessage { role, content });
    }

    fn auto_scroll(&mut self) {
        self.scroll = 0;
    }
}

pub async fn run_tui(settings: Settings, theme_name: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = AppState::new(&settings, theme_name);

    let llm = match settings.build_llm_client() {
        Ok(llm) => Some(llm),
        Err(e) => {
            state.add_message(
                MessageRole::System,
                format!("LLM not available: {e}\nSet ANTHROPIC_API_KEY or use --provider ollama"),
            );
            None
        }
    };

    let (agent_event_tx, mut agent_event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (user_input_tx, mut user_input_rx) = mpsc::unbounded_channel::<String>();

    // Agent worker task
    if let Some(llm) = llm {
        let event_tx = agent_event_tx;
        tokio::spawn(async move {
            let agent = Agent::new(llm).with_system_prompt(
                "You are PhazeAI, an AI coding assistant in a terminal. \
                 Help the user with coding tasks. Be concise. \
                 When showing code, use markdown code blocks with language tags.",
            );

            while let Some(input) = user_input_rx.recv().await {
                let local_tx = event_tx.clone();
                let (inner_tx, mut inner_rx) = mpsc::unbounded_channel();

                let agent_fut = agent.run_with_events(input, inner_tx);

                let forward = tokio::spawn(async move {
                    while let Some(ev) = inner_rx.recv().await {
                        if local_tx.send(ev).is_err() {
                            break;
                        }
                    }
                });

                if let Err(e) = agent_fut.await {
                    let _ = event_tx.send(AgentEvent::Error(e.to_string()));
                }

                forward.abort();
            }
        });
    }

    loop {
        // Draw
        terminal.draw(|f| draw_ui(f, &state))?;

        // Process agent events (non-blocking)
        while let Ok(agent_event) = agent_event_rx.try_recv() {
            handle_agent_event(&mut state, agent_event);
        }

        // Handle keyboard input with timeout
        if event::poll(std::time::Duration::from_millis(33))? {
            if let Event::Key(key) = event::read()? {
                handle_key(&mut state, key, &user_input_tx);
            }
        }

        if state.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn draw_ui(f: &mut ratatui::Frame, state: &AppState) {
    let theme = &state.theme;

    // Main vertical layout: chat + input + status
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),     // chat
            Constraint::Length(3),   // input
            Constraint::Length(1),   // status
        ])
        .split(f.area());

    // Optionally split chat area for file tree
    let chat_area = if state.show_files {
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(30), Constraint::Min(40)])
            .split(main_chunks[0]);

        // File tree panel
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());
        let files_block = Block::default()
            .borders(Borders::ALL)
            .title(" Files ")
            .border_style(Style::default().fg(theme.border));
        let files_text = Paragraph::new(format!("  {cwd}\n  (file tree placeholder)"))
            .block(files_block)
            .style(Style::default().fg(theme.muted));
        f.render_widget(files_text, h_chunks[0]);

        h_chunks[1]
    } else {
        main_chunks[0]
    };

    // Chat messages
    let mut chat_lines: Vec<Line> = Vec::new();
    for msg in &state.messages {
        let (prefix, color) = match msg.role {
            MessageRole::User => ("You > ", theme.user_color),
            MessageRole::Assistant => ("AI > ", theme.assistant_color),
            MessageRole::System => ("", theme.system_color),
            MessageRole::Tool => ("  ", theme.tool_color),
        };

        // Split content into lines to handle multi-line messages
        for (i, line) in msg.content.lines().enumerate() {
            if i == 0 {
                chat_lines.push(Line::from(vec![
                    Span::styled(
                        prefix,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(line, Style::default().fg(color)),
                ]));
            } else {
                let indent = " ".repeat(prefix.len());
                chat_lines.push(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(line, Style::default().fg(color)),
                ]));
            }
        }
        chat_lines.push(Line::raw(""));
    }

    if state.is_processing {
        chat_lines.push(Line::from(Span::styled(
            "  Thinking...",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::DIM),
        )));
    }

    let chat = Paragraph::new(Text::from(chat_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" PhazeAI ")
                .border_style(Style::default().fg(theme.border)),
        )
        .wrap(Wrap { trim: false })
        .scroll((state.scroll, 0));
    f.render_widget(chat, chat_area);

    // Input area
    let input_style = if state.is_processing {
        Style::default().fg(theme.muted)
    } else {
        Style::default().fg(theme.fg)
    };
    let input = Paragraph::new(state.input.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(if state.is_processing {
                    " Input (processing...) "
                } else {
                    " Input "
                })
                .border_style(Style::default().fg(theme.border)),
        )
        .style(input_style);
    f.render_widget(input, main_chunks[1]);

    // Set cursor position in input area
    if !state.is_processing {
        f.set_cursor_position((
            main_chunks[1].x + state.cursor_pos as u16 + 1,
            main_chunks[1].y + 1,
        ));
    }

    // Status bar
    let status_spans = vec![
        Span::styled(
            format!(" {} ", state.model_name),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::default().fg(theme.muted)),
        Span::styled(&state.status_text, Style::default().fg(theme.muted)),
        Span::styled(
            format!(
                "  {}  ",
                if state.show_files {
                    "Ctrl+E:hide files"
                } else {
                    "Ctrl+E:files"
                }
            ),
            Style::default().fg(theme.muted),
        ),
    ];
    let status = Paragraph::new(Line::from(status_spans));
    f.render_widget(status, main_chunks[2]);
}

fn handle_agent_event(state: &mut AppState, event: AgentEvent) {
    match event {
        AgentEvent::Thinking { iteration } => {
            state.status_text = format!("Thinking... (iteration {iteration})");
        }
        AgentEvent::TextDelta(text) => {
            // Append to existing assistant message or create new one
            if let Some(last) = state.messages.last_mut() {
                if last.role == MessageRole::Assistant {
                    last.content.push_str(&text);
                    state.auto_scroll();
                    return;
                }
            }
            state.add_message(MessageRole::Assistant, text);
            state.auto_scroll();
        }
        AgentEvent::ToolStart { name } => {
            state.add_message(MessageRole::Tool, format!("[{name}]"));
            state.status_text = format!("Running {name}...");
        }
        AgentEvent::ToolResult {
            name,
            success,
            summary,
        } => {
            let icon = if success { "ok" } else { "ERR" };
            state.add_message(MessageRole::Tool, format!("[{name}: {icon}] {summary}"));
        }
        AgentEvent::Complete { iterations } => {
            state.is_processing = false;
            state.status_text = format!("Done ({iterations} iterations)");
            state.auto_scroll();
        }
        AgentEvent::Error(e) => {
            state.is_processing = false;
            state.add_message(MessageRole::System, format!("Error: {e}"));
            state.status_text = "Error".into();
        }
    }
}

fn handle_key(
    state: &mut AppState,
    key: event::KeyEvent,
    user_input_tx: &mpsc::UnboundedSender<String>,
) {
    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            state.should_quit = true;
        }
        (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
            state.show_files = !state.show_files;
        }
        (_, KeyCode::Enter) => {
            if state.input.is_empty() || state.is_processing {
                return;
            }

            let input = state.input.clone();
            state.input.clear();
            state.cursor_pos = 0;

            // Handle slash commands
            if input.starts_with('/') {
                match commands::handle_command(&input) {
                    CommandResult::Message(msg) => {
                        state.add_message(MessageRole::System, msg);
                    }
                    CommandResult::Clear => {
                        state.messages.clear();
                        state.add_message(MessageRole::System, "Chat cleared.".into());
                    }
                    CommandResult::Quit => {
                        state.should_quit = true;
                    }
                    CommandResult::ModelChanged(model) => {
                        state.model_name = model.clone();
                        state.add_message(
                            MessageRole::System,
                            format!("Model changed to: {model}"),
                        );
                    }
                    CommandResult::ThemeChanged(name) => {
                        state.theme = Theme::by_name(&name);
                        state.add_message(
                            MessageRole::System,
                            format!("Theme changed to: {}", state.theme.name),
                        );
                    }
                    CommandResult::ToggleFiles => {
                        state.show_files = !state.show_files;
                    }
                    CommandResult::NotACommand => {
                        // Shouldn't happen for /-prefixed input
                    }
                }
                return;
            }

            state.add_message(MessageRole::User, input.clone());
            state.is_processing = true;
            state.status_text = "Sending...".into();
            state.auto_scroll();
            let _ = user_input_tx.send(input);
        }
        (_, KeyCode::Backspace) => {
            if state.cursor_pos > 0 {
                state.input.remove(state.cursor_pos - 1);
                state.cursor_pos -= 1;
            }
        }
        (_, KeyCode::Delete) => {
            if state.cursor_pos < state.input.len() {
                state.input.remove(state.cursor_pos);
            }
        }
        (_, KeyCode::Left) => {
            state.cursor_pos = state.cursor_pos.saturating_sub(1);
        }
        (_, KeyCode::Right) => {
            if state.cursor_pos < state.input.len() {
                state.cursor_pos += 1;
            }
        }
        (_, KeyCode::Home) => {
            state.cursor_pos = 0;
        }
        (_, KeyCode::End) => {
            state.cursor_pos = state.input.len();
        }
        (_, KeyCode::Char(c)) => {
            if !state.is_processing {
                state.input.insert(state.cursor_pos, c);
                state.cursor_pos += 1;
            }
        }
        (_, KeyCode::PageUp) => {
            state.scroll = state.scroll.saturating_add(10);
        }
        (_, KeyCode::PageDown) => {
            state.scroll = state.scroll.saturating_sub(10);
        }
        (_, KeyCode::Up) => {
            state.scroll = state.scroll.saturating_add(1);
        }
        (_, KeyCode::Down) => {
            state.scroll = state.scroll.saturating_sub(1);
        }
        _ => {}
    }
}
