use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use phazeai_core::{
    collect_git_info,
    context::{ConversationMetadata, ConversationStore, SavedConversation, SavedMessage},
    tools::{ToolApprovalManager, ToolApprovalMode},
    Agent, AgentEvent, Settings, SystemPromptBuilder,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Terminal,
};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::commands::{self, CommandResult};
use crate::theme::Theme;

// ── Single-prompt mode ──────────────────────────────────────────────────

pub async fn run_single_prompt(
    settings: &Settings,
    prompt: &str,
    extra_instructions: Option<&str>,
) -> Result<()> {
    let llm = settings.build_llm_client()?;
    let system_prompt = build_system_prompt(extra_instructions);

    let mut agent = Agent::new(llm).with_system_prompt(system_prompt);

    // Try to start sidecar for semantic search
    if let Some(client) = try_start_sidecar().await {
        let client = Arc::new(client);
        agent.register_tool(Box::new(phazeai_sidecar::SemanticSearchTool::new(
            client.clone(),
        )));
        agent.register_tool(Box::new(phazeai_sidecar::BuildIndexTool::new(client)));
    }

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

#[derive(Clone)]
struct ChatMessage {
    role: MessageRole,
    content: String,
    timestamp: String,
}

#[derive(Clone, PartialEq)]
enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
    DiffAdd,
    DiffRemove,
    DiffHeader,
}

/// A single entry in the chat history — either a plain message or a tool-call card.
#[derive(Clone)]
#[allow(dead_code)]
enum ChatItem {
    Message(ChatMessage),
    ToolCard {
        name: String,
        args: String,
        output: String,
        /// None = running, Some(true) = ok, Some(false) = err
        success: Option<bool>,
        /// When true, only show the header line (for future collapsible UI)
        collapsed: bool,
    },
}

/// What we're waiting on from the user during tool approval
struct PendingApproval {
    tool_name: String,
    description: String,
}

struct AppState {
    // Input
    input: String,
    cursor_pos: usize,
    input_history: Vec<String>,
    history_pos: Option<usize>,

    // Chat
    messages: Vec<ChatItem>,
    scroll_offset: usize,
    total_content_lines: usize,

    // Processing state
    is_processing: bool,
    pending_approval: Option<PendingApproval>,

    // Status
    status_text: String,
    model_name: String,
    provider_name: String,
    iterations: usize,
    total_tokens_in: u64,
    total_tokens_out: u64,
    estimated_cost: f64,

    // Display
    should_quit: bool,
    show_files: bool,
    theme: Theme,

    // Conversation management
    conversation_id: String,
    conversation_store: ConversationStore,

    // Tool approval
    approval_manager: ToolApprovalManager,

    /// Current AI mode (chat, ask, debug, plan, edit)
    ai_mode: String,
    /// Last user message sent, used by /retry
    last_user_input: String,

    /// Handle to the running agent task — aborted on /cancel
    agent_task: Option<tokio::task::JoinHandle<()>>,

    /// Oneshot sender used to respond to the pending approval callback.
    /// When the user presses y/n/a/s, we send true/false through this channel.
    approval_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<bool>>>>,
}

impl AppState {
    fn new(settings: &Settings, theme_name: &str) -> Self {
        let provider_name = format!("{:?}", settings.llm.provider);
        let store = ConversationStore::new().unwrap_or_else(|_| ConversationStore::default());
        let conv_id = ConversationStore::generate_id();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());

        Self {
            input: String::new(),
            cursor_pos: 0,
            input_history: Vec::new(),
            history_pos: None,

            messages: vec![ChatItem::Message(ChatMessage {
                role: MessageRole::System,
                content: format!(
                    "PhazeAI v0.1.0 | {} | {} | {}\n\
                     Type a message and press Enter. Ctrl+C to quit. /help for commands.",
                    provider_name, settings.llm.model, cwd
                ),
                timestamp: now_str(),
            })],
            scroll_offset: 0,
            total_content_lines: 0,

            is_processing: false,
            pending_approval: None,

            status_text: "Ready".into(),
            model_name: settings.llm.model.clone(),
            provider_name,
            iterations: 0,
            total_tokens_in: 0,
            total_tokens_out: 0,
            estimated_cost: 0.0,

            should_quit: false,
            show_files: false,
            theme: Theme::by_name(theme_name),

            conversation_id: conv_id,
            conversation_store: store,

            approval_manager: ToolApprovalManager::default(),
            ai_mode: "chat".into(),
            last_user_input: String::new(),

            agent_task: None,
            approval_tx: Arc::new(Mutex::new(None)),
        }
    }

    fn add_message(&mut self, role: MessageRole, content: String) {
        self.messages.push(ChatItem::Message(ChatMessage {
            role,
            content,
            timestamp: now_str(),
        }));
        self.scroll_to_bottom();
    }

    fn add_tool_card(&mut self, name: String) {
        self.messages.push(ChatItem::ToolCard {
            name,
            args: String::new(),
            output: String::new(),
            success: None,
            collapsed: false,
        });
        self.scroll_to_bottom();
    }

    fn finish_tool_card(&mut self, name: &str, output: String, success: bool) {
        // Update the last ToolCard that matches this name and is still running
        for item in self.messages.iter_mut().rev() {
            if let ChatItem::ToolCard {
                name: card_name,
                output: card_output,
                success: card_success,
                ..
            } = item
            {
                if card_name == name && card_success.is_none() {
                    *card_output = output;
                    *card_success = Some(success);
                    break;
                }
            }
        }
        self.scroll_to_bottom();
    }

    fn scroll_to_bottom(&mut self) {
        // Will be resolved on next draw
        self.scroll_offset = usize::MAX;
    }

    fn push_history(&mut self, input: String) {
        if !input.is_empty() && self.input_history.last() != Some(&input) {
            self.input_history.push(input);
        }
        self.history_pos = None;
    }

    fn history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            None => self.input_history.len().saturating_sub(1),
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.history_pos = Some(pos);
        self.input = self.input_history[pos].clone();
        self.cursor_pos = self.input.len();
    }

    fn history_next(&mut self) {
        match self.history_pos {
            None => {}
            Some(pos) => {
                if pos + 1 >= self.input_history.len() {
                    self.history_pos = None;
                    self.input.clear();
                    self.cursor_pos = 0;
                } else {
                    self.history_pos = Some(pos + 1);
                    self.input = self.input_history[pos + 1].clone();
                    self.cursor_pos = self.input.len();
                }
            }
        }
    }

    fn save_conversation(&self) {
        let messages: Vec<SavedMessage> = self
            .messages
            .iter()
            .map(|item| match item {
                ChatItem::Message(m) => SavedMessage {
                    role: match m.role {
                        MessageRole::User => "user".into(),
                        MessageRole::Assistant => "assistant".into(),
                        MessageRole::System => "system".into(),
                        MessageRole::Tool => "tool".into(),
                        MessageRole::DiffAdd => "tool".into(),
                        MessageRole::DiffRemove => "tool".into(),
                        MessageRole::DiffHeader => "tool".into(),
                    },
                    content: m.content.clone(),
                    timestamp: m.timestamp.clone(),
                    tool_name: None,
                },
                ChatItem::ToolCard {
                    name,
                    output,
                    success,
                    ..
                } => {
                    let icon = match success {
                        Some(true) => "ok",
                        Some(false) => "err",
                        None => "running",
                    };
                    SavedMessage {
                        role: "tool".into(),
                        content: format!("[{name}: {icon}] {output}"),
                        timestamp: now_str(),
                        tool_name: Some(name.clone()),
                    }
                }
            })
            .collect();

        let title = self
            .messages
            .iter()
            .find_map(|item| {
                if let ChatItem::Message(m) = item {
                    if m.role == MessageRole::User {
                        let t = m.content.chars().take(80).collect::<String>();
                        return Some(if m.content.len() > 80 {
                            format!("{}...", t)
                        } else {
                            t
                        });
                    }
                }
                None
            })
            .unwrap_or_else(|| "Untitled".into());

        let cwd = std::env::current_dir()
            .ok()
            .map(|p| p.display().to_string());

        let metadata = ConversationMetadata {
            id: self.conversation_id.clone(),
            title,
            created_at: now_str(),
            updated_at: now_str(),
            message_count: messages.len(),
            model: self.model_name.clone(),
            project_dir: cwd,
        };

        let conversation = SavedConversation {
            metadata,
            messages,
            system_prompt: None,
        };

        if let Err(e) = self.conversation_store.save(&conversation) {
            tracing::warn!("Failed to save conversation: {e}");
        }
    }
}

pub async fn run_tui(
    settings: Settings,
    theme_name: &str,
    continue_last: bool,
    resume_id: Option<String>,
    extra_instructions: Option<&str>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = AppState::new(&settings, theme_name);

    // Restore conversation if requested
    let mut restore_messages: Vec<(String, String)> = Vec::new(); // (role, content) pairs

    if continue_last {
        if let Ok(recent) = state.conversation_store.list_recent(1) {
            if let Some(meta) = recent.first() {
                if let Ok(conv) = state.conversation_store.load(&meta.id) {
                    state.conversation_id = conv.metadata.id.clone();
                    for msg in &conv.messages {
                        let role = match msg.role.as_str() {
                            "user" => MessageRole::User,
                            "assistant" => MessageRole::Assistant,
                            "tool" => MessageRole::Tool,
                            _ => MessageRole::System,
                        };
                        state.messages.push(ChatItem::Message(ChatMessage {
                            role: role.clone(),
                            content: msg.content.clone(),
                            timestamp: msg.timestamp.clone(),
                        }));
                        // Collect user/assistant pairs for agent context
                        if msg.role == "user" || msg.role == "assistant" {
                            restore_messages.push((msg.role.clone(), msg.content.clone()));
                        }
                    }
                    state.add_message(
                        MessageRole::System,
                        format!("Resumed: {}", conv.metadata.title),
                    );
                    state.scroll_to_bottom();
                }
            }
        }
    } else if let Some(ref id) = resume_id {
        // Try prefix match
        if let Ok(recent) = state.conversation_store.list_recent(100) {
            if let Some(meta) = recent.iter().find(|m| m.id.starts_with(id)) {
                if let Ok(conv) = state.conversation_store.load(&meta.id) {
                    state.conversation_id = conv.metadata.id.clone();
                    for msg in &conv.messages {
                        let role = match msg.role.as_str() {
                            "user" => MessageRole::User,
                            "assistant" => MessageRole::Assistant,
                            "tool" => MessageRole::Tool,
                            _ => MessageRole::System,
                        };
                        state.messages.push(ChatItem::Message(ChatMessage {
                            role: role.clone(),
                            content: msg.content.clone(),
                            timestamp: msg.timestamp.clone(),
                        }));
                        if msg.role == "user" || msg.role == "assistant" {
                            restore_messages.push((msg.role.clone(), msg.content.clone()));
                        }
                    }
                    state.add_message(
                        MessageRole::System,
                        format!("Resumed: {}", conv.metadata.title),
                    );
                    state.scroll_to_bottom();
                } else {
                    state.add_message(
                        MessageRole::System,
                        format!("Failed to load conversation '{}'", meta.id),
                    );
                }
            } else {
                state.add_message(
                    MessageRole::System,
                    format!("No conversation matching '{id}' found."),
                );
            }
        }
    }

    let system_prompt = build_system_prompt(extra_instructions);

    let llm = match settings.build_llm_client() {
        Ok(llm) => Some(llm),
        Err(e) => {
            state.add_message(
                MessageRole::System,
                format!(
                    "LLM not available: {e}\n\
                     Set your API key or use --provider ollama for local models.\n\
                     Available providers: claude, openai, ollama, groq, together, openrouter, lmstudio"
                ),
            );
            None
        }
    };

    let (agent_event_tx, mut agent_event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (user_input_tx, mut user_input_rx) = mpsc::unbounded_channel::<String>();

    // Agent worker task
    if let Some(llm) = llm {
        let event_tx = agent_event_tx;
        let restore_msgs = restore_messages.clone();

        // Share the approval_tx with the agent callback so it can block on user input.
        let approval_tx_shared = state.approval_tx.clone();

        // Create approval callback
        let approval_manager_clone =
            Arc::new(std::sync::Mutex::new(ToolApprovalManager::default()));
        let approval_mgr = approval_manager_clone.clone();
        let approval_fn: phazeai_core::agent::ApprovalFn = Box::new(move |tool_name, params| {
            let mgr = approval_mgr.clone();
            let tx_slot = approval_tx_shared.clone();
            Box::pin(async move {
                let needs = {
                    let mgr = mgr.lock().unwrap_or_else(|e| e.into_inner());
                    mgr.needs_approval(&tool_name, &params)
                };
                if !needs {
                    return true; // Auto-approved by manager policy
                }

                // Block until the UI responds via the oneshot channel.
                let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
                {
                    let mut slot = tx_slot.lock().unwrap_or_else(|e| e.into_inner());
                    *slot = Some(tx);
                }

                // Await user response; default to deny on channel error.
                let approved = rx.await.unwrap_or(false);

                if approved {
                    let mut mgr = mgr.lock().unwrap_or_else(|e| e.into_inner());
                    mgr.record_approval(&tool_name);
                }

                approved
            })
        });

        let handle = tokio::spawn(async move {
            let mut agent = Agent::new(llm)
                .with_system_prompt(system_prompt)
                .with_approval(approval_fn);

            // Try to start the Python sidecar for semantic search
            if let Some(client) = try_start_sidecar().await {
                let client = Arc::new(client);
                agent.register_tool(Box::new(phazeai_sidecar::SemanticSearchTool::new(
                    client.clone(),
                )));
                agent.register_tool(Box::new(phazeai_sidecar::BuildIndexTool::new(client)));
            }

            // Load any restored history
            if !restore_msgs.is_empty() {
                agent.load_history(restore_msgs).await;
            }

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

        // Store the outer worker handle so /cancel can abort it.
        state.agent_task = Some(handle);
    }

    loop {
        // Draw
        terminal.draw(|f| draw_ui(f, &mut state))?;

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
            // Auto-save conversation on exit
            if state.messages.len() > 1 {
                state.save_conversation();
            }
            break;
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

/// Prepend mode-specific instructions to the user's message so the LLM
/// knows which role it should take for this turn.
fn apply_mode_prefix(mode: &str, input: &str) -> String {
    let prefix = match mode {
        "plan" => {
            "[PLANNING MODE] You are a senior software architect. \
            Your task is to produce a clear, structured, step-by-step plan. \
            Use numbered lists and headers. Do NOT write code yet — only plan.\n\n"
        }
        "debug" => {
            "[DEBUG MODE] You are an expert debugger. \
            Diagnose the root cause of the issue described below before suggesting any fix. \
            Show your reasoning step by step.\n\n"
        }
        "ask" => {
            "[READ-ONLY MODE] Answer the question below. \
            Do NOT modify any files. Do NOT write new code. \
            Only read, explain, and answer.\n\n"
        }
        "edit" => {
            "[EDIT MODE] Make precise, minimal code changes to satisfy the request. \
            Use the edit_file tool. Keep changes focused and avoid unrelated refactors.\n\n"
        }
        _ => "", // "chat" — no prefix, natural conversation
    };
    if prefix.is_empty() {
        input.to_string()
    } else {
        format!("{prefix}{input}")
    }
}

fn build_system_prompt(extra_instructions: Option<&str>) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let mut builder = SystemPromptBuilder::new()
        .with_project_root(cwd.clone())
        .with_tools(vec![
            "read_file".into(),
            "write_file".into(),
            "edit_file".into(),
            "bash".into(),
            "grep".into(),
            "glob".into(),
            "list_files".into(),
        ])
        .load_project_instructions();

    let (branch, dirty) = collect_git_info(&cwd);
    builder = builder.with_git_info(branch, dirty);

    if let Some(extra) = extra_instructions {
        builder = builder.with_additional_instructions(extra.to_string());
    }

    builder.build()
}

fn draw_ui(f: &mut ratatui::Frame, state: &mut AppState) {
    let theme = &state.theme;

    // Main vertical layout: chat + input + status
    let input_height = if state.pending_approval.is_some() {
        5
    } else {
        3
    };

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),               // chat
            Constraint::Length(input_height), // input (or approval prompt)
            Constraint::Length(1),            // status
        ])
        .split(f.area());

    // Optionally split chat area for file tree
    let chat_area = if state.show_files {
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(30), Constraint::Min(40)])
            .split(main_chunks[0]);

        draw_file_tree(f, h_chunks[0], theme);
        h_chunks[1]
    } else {
        main_chunks[0]
    };

    // Chat messages
    let chat_lines = build_chat_lines(&state.messages, state.is_processing, theme);
    let total_lines = chat_lines.len();
    state.total_content_lines = total_lines;

    // Calculate visible height (area height - 2 for borders)
    let visible_height = chat_area.height.saturating_sub(2) as usize;

    // Resolve scroll_to_bottom
    if state.scroll_offset == usize::MAX {
        state.scroll_offset = total_lines.saturating_sub(visible_height);
    }

    // Clamp scroll offset
    let max_scroll = total_lines.saturating_sub(visible_height);
    if state.scroll_offset > max_scroll {
        state.scroll_offset = max_scroll;
    }

    let chat = Paragraph::new(Text::from(chat_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" PhazeAI ")
                .border_style(Style::default().fg(theme.border)),
        )
        .wrap(Wrap { trim: false })
        .scroll((state.scroll_offset as u16, 0));
    f.render_widget(chat, chat_area);

    // Scrollbar
    if total_lines > visible_height {
        let mut scrollbar_state = ScrollbarState::new(max_scroll).position(state.scroll_offset);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("^"))
                .end_symbol(Some("v")),
            chat_area,
            &mut scrollbar_state,
        );
    }

    // Input or approval prompt
    if let Some(ref approval) = state.pending_approval {
        draw_approval_prompt(f, main_chunks[1], approval, theme);
    } else {
        draw_input(f, main_chunks[1], state, theme);
    }

    // Status bar
    draw_status_bar(f, main_chunks[2], state, theme);
}

fn draw_file_tree(f: &mut ratatui::Frame, area: Rect, theme: &Theme) {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());

    let mut lines: Vec<Line> = Vec::new();

    // Header: show the working directory path.
    lines.push(Line::from(Span::styled(
        format!(" {cwd}"),
        Style::default().fg(theme.accent),
    )));
    lines.push(Line::raw(""));

    if let Ok(entries) = std::fs::read_dir(".") {
        let mut entries: Vec<_> = entries.flatten().collect();
        // Directories first, then files; both sorted alphabetically.
        entries.sort_by_key(|e| {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (!is_dir, e.file_name())
        });

        let mut count = 0usize;
        for entry in &entries {
            if count >= 50 {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip hidden entries and common noise dirs.
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let icon = if is_dir { "▸ " } else { "  " };
            let color = if is_dir { theme.accent } else { theme.fg };
            lines.push(Line::from(Span::styled(
                format!(" {icon}{name}"),
                Style::default().fg(color),
            )));
            count += 1;

            // Show immediate children of directories (depth 1).
            if is_dir {
                if let Ok(sub) = std::fs::read_dir(entry.path()) {
                    let mut sub: Vec<_> = sub.flatten().collect();
                    sub.sort_by_key(|e| e.file_name());
                    let mut sub_count = 0usize;
                    for sub_entry in &sub {
                        if sub_count >= 20 || count >= 50 {
                            break;
                        }
                        let sub_name = sub_entry.file_name().to_string_lossy().to_string();
                        if sub_name.starts_with('.') {
                            continue;
                        }
                        let sub_is_dir =
                            sub_entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        let sub_icon = if sub_is_dir { "▸ " } else { "  " };
                        let sub_color = if sub_is_dir { theme.accent } else { theme.muted };
                        lines.push(Line::from(Span::styled(
                            format!("   {sub_icon}{sub_name}"),
                            Style::default().fg(sub_color),
                        )));
                        sub_count += 1;
                        count += 1;
                    }
                }
            }
        }
    }

    let files_block = Block::default()
        .borders(Borders::ALL)
        .title(" Files ")
        .border_style(Style::default().fg(theme.border));

    let paragraph = Paragraph::new(lines)
        .block(files_block)
        .style(Style::default().fg(theme.fg));

    f.render_widget(paragraph, area);
}

fn render_message_lines<'a>(msg: &'a ChatMessage, theme: &'a Theme) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::new();

    let (prefix, color) = match msg.role {
        MessageRole::User => ("You > ", theme.user_color),
        MessageRole::Assistant => ("AI > ", theme.assistant_color),
        MessageRole::System => ("", theme.system_color),
        MessageRole::Tool => ("  ", theme.tool_color),
        MessageRole::DiffAdd => ("  ", theme.success),
        MessageRole::DiffRemove => ("  ", theme.error),
        MessageRole::DiffHeader => ("  ", theme.accent),
    };

    // For assistant messages, do code-block detection.
    // For all others, render plainly.
    let do_code_detection = msg.role == MessageRole::Assistant;

    let mut in_code_block = false;
    for (i, raw_line) in msg.content.lines().enumerate() {
        let is_fence = raw_line.starts_with("```");

        if do_code_detection {
            if is_fence {
                // Toggle code block state; render the fence line distinctly
                let was_in = in_code_block;
                in_code_block = !in_code_block;
                let lang = if !was_in {
                    raw_line.trim_start_matches('`').trim()
                } else {
                    ""
                };
                let label = if !lang.is_empty() {
                    format!("  ```{lang}")
                } else {
                    "  ```".to_string()
                };
                if i == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(
                            prefix,
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(label, Style::default().fg(theme.muted)),
                    ]));
                } else {
                    lines.push(Line::from(Span::styled(
                        label,
                        Style::default().fg(theme.muted),
                    )));
                }
                continue;
            }

            if in_code_block {
                let indent = " ".repeat(prefix.len());
                lines.push(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(raw_line, Style::default().fg(theme.code_fg)),
                ]));
                continue;
            }
        }

        // Regular line rendering
        if i == 0 {
            lines.push(Line::from(vec![
                Span::styled(
                    prefix,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(raw_line, Style::default().fg(color)),
            ]));
        } else {
            let indent = " ".repeat(prefix.len());
            lines.push(Line::from(vec![
                Span::raw(indent),
                Span::styled(raw_line, Style::default().fg(color)),
            ]));
        }
    }

    lines
}

fn render_tool_card_lines<'a>(
    name: &'a str,
    _args: &'a str,
    output: &'a str,
    success: Option<bool>,
    theme: &'a Theme,
) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::new();

    // Choose border color based on state
    let (border_color, status_icon, status_text) = match success {
        None => (theme.warning, "⟳", "running..."),
        Some(true) => (theme.success, "✓", "completed"),
        Some(false) => (theme.error, "✗", "failed"),
    };

    // Top border with tool name in title
    let title_str = format!("─ {name} ");
    let top_line = format!("  ┌{title_str}");
    // Pad to a reasonable width; we'll just render the border open-ended
    lines.push(Line::from(vec![
        Span::styled(top_line, Style::default().fg(border_color)),
        Span::styled(
            "─".repeat(40usize.saturating_sub(title_str.len() + 4)),
            Style::default().fg(border_color),
        ),
        Span::styled("┐", Style::default().fg(border_color)),
    ]));

    // Output lines (up to 8 lines to avoid flooding)
    let content_lines: Vec<&str> = if output.is_empty() {
        vec![]
    } else {
        output.lines().take(8).collect()
    };

    for content_line in &content_lines {
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(border_color)),
            Span::styled(*content_line, Style::default().fg(theme.fg)),
        ]));
    }

    if content_lines.is_empty() && success.is_none() {
        // Show "running" placeholder
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(border_color)),
            Span::styled(
                "  waiting for output...",
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            ),
        ]));
    }

    // Status line
    lines.push(Line::from(vec![
        Span::styled("  │ ", Style::default().fg(border_color)),
        Span::styled(
            format!("{status_icon} {status_text}"),
            Style::default().fg(border_color),
        ),
    ]));

    // Bottom border
    lines.push(Line::from(Span::styled(
        "  └".to_string() + &"─".repeat(44),
        Style::default().fg(border_color),
    )));

    lines.push(Line::raw(""));
    lines
}

fn build_chat_lines<'a>(
    messages: &'a [ChatItem],
    is_processing: bool,
    theme: &'a Theme,
) -> Vec<Line<'a>> {
    let mut chat_lines: Vec<Line> = Vec::new();

    for item in messages {
        match item {
            ChatItem::Message(msg) => {
                chat_lines.extend(render_message_lines(msg, theme));
                chat_lines.push(Line::raw(""));
            }
            ChatItem::ToolCard {
                name,
                args,
                output,
                success,
                collapsed: _,
            } => {
                chat_lines.extend(render_tool_card_lines(name, args, output, *success, theme));
            }
        }
    }

    if is_processing {
        chat_lines.push(Line::from(Span::styled(
            "  Thinking...",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::DIM),
        )));
    }

    chat_lines
}

fn draw_approval_prompt(
    f: &mut ratatui::Frame,
    area: Rect,
    approval: &PendingApproval,
    theme: &Theme,
) {
    let text = vec![
        Line::from(vec![
            Span::styled(
                format!(" Tool: {} ", approval.tool_name),
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(&approval.description, Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![Span::styled(
            " [y] Allow  [n] Deny  [a] Allow all  [s] Allow session ",
            Style::default().fg(theme.accent),
        )]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Approve? ")
        .border_style(Style::default().fg(theme.warning));

    f.render_widget(Paragraph::new(text).block(block), area);
}

fn draw_input(f: &mut ratatui::Frame, area: Rect, state: &AppState, theme: &Theme) {
    let input_style = if state.is_processing {
        Style::default().fg(theme.muted)
    } else {
        Style::default().fg(theme.fg)
    };

    let title = if state.is_processing {
        " Input (processing...) "
    } else if state.input.starts_with('/') {
        " Command "
    } else {
        " Input  Shift+Enter for newline "
    };

    let input = Paragraph::new(state.input.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(if state.input.starts_with('/') {
                    theme.accent
                } else {
                    theme.border
                })),
        )
        .style(input_style);
    f.render_widget(input, area);

    // Set cursor position in input area
    if !state.is_processing && state.pending_approval.is_none() {
        let cursor_x = area.x + state.cursor_pos as u16 + 1;
        // Clamp cursor to area width
        let max_x = area.x + area.width.saturating_sub(2);
        f.set_cursor_position((cursor_x.min(max_x), area.y + 1));
    }
}

fn draw_status_bar(f: &mut ratatui::Frame, area: Rect, state: &AppState, theme: &Theme) {
    let cwd = std::env::current_dir()
        .map(|p| {
            let s = p.display().to_string();
            // Shorten home dir
            if let Ok(home) = std::env::var("HOME") {
                if s.starts_with(&home) {
                    return format!("~{}", &s[home.len()..]);
                }
            }
            s
        })
        .unwrap_or_else(|_| ".".into());

    let tokens_str = if state.total_tokens_in > 0 || state.total_tokens_out > 0 {
        format!(
            "{}in/{}out ",
            format_tokens(state.total_tokens_in),
            format_tokens(state.total_tokens_out),
        )
    } else {
        String::new()
    };

    let cost_str = if state.estimated_cost > 0.0 {
        format!("${:.4} ", state.estimated_cost)
    } else {
        String::new()
    };

    let approval_str = match state.approval_manager.mode() {
        ToolApprovalMode::AutoApprove => "auto",
        ToolApprovalMode::AlwaysAsk => "ask",
        ToolApprovalMode::AskOnce => "ask-once",
    };

    let status_spans = vec![
        Span::styled(
            format!(" {} ", state.provider_name),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("| {} ", state.model_name),
            Style::default().fg(theme.accent),
        ),
        Span::styled(
            format!("| {} ", state.ai_mode),
            Style::default().fg(theme.accent),
        ),
        Span::styled(
            format!("| {} ", approval_str),
            Style::default().fg(theme.muted),
        ),
        Span::styled(&tokens_str, Style::default().fg(theme.muted)),
        Span::styled(&cost_str, Style::default().fg(theme.warning)),
        Span::styled("| ", Style::default().fg(theme.muted)),
        Span::styled(&state.status_text, Style::default().fg(theme.muted)),
        // Right-align cwd and keybindings
        Span::styled(format!("  {} ", cwd), Style::default().fg(theme.muted)),
    ];
    let status = Paragraph::new(Line::from(status_spans));
    f.render_widget(status, area);
}

fn handle_agent_event(state: &mut AppState, event: AgentEvent) {
    match event {
        AgentEvent::Thinking { iteration } => {
            state.iterations = iteration;
            state.status_text = format!("Thinking... (step {iteration})");
        }
        AgentEvent::ToolApprovalRequest { name, params } => {
            // Handle tool approval request if needed
            let desc = format!("{}: {}", name, params);
            state.pending_approval = Some(PendingApproval {
                tool_name: name,
                description: desc,
            });
        }
        AgentEvent::TextDelta(text) => {
            // Append to existing assistant message or create new one
            if let Some(ChatItem::Message(m)) = state.messages.last_mut() {
                if m.role == MessageRole::Assistant {
                    m.content.push_str(&text);
                    state.scroll_to_bottom();
                    return;
                }
            }
            state.add_message(MessageRole::Assistant, text);
        }
        AgentEvent::ToolStart { name } => {
            // Check if tool needs approval
            let needs_approval = state
                .approval_manager
                .needs_approval(&name, &serde_json::Value::Null);

            if needs_approval {
                let desc = state
                    .approval_manager
                    .format_approval_prompt(&name, &serde_json::Value::Null);
                state.pending_approval = Some(PendingApproval {
                    tool_name: name.clone(),
                    description: desc,
                });
            }

            // Per-tool status text
            state.status_text = match name.as_str() {
                "bash" => "Running bash...".into(),
                "read_file" => "Reading file...".into(),
                "write_file" => "Writing file...".into(),
                "edit_file" => "Editing file...".into(),
                "grep" => "Searching...".into(),
                "glob" | "list_files" => "Listing files...".into(),
                _ => format!("Running {name}..."),
            };

            state.add_tool_card(name);
        }
        AgentEvent::ToolResult {
            name,
            success,
            summary,
        } => {
            state.finish_tool_card(&name, summary, success);
            state.pending_approval = None;
        }
        AgentEvent::Complete { iterations } => {
            state.is_processing = false;
            state.status_text = format!("Done ({iterations} steps)");
            state.scroll_to_bottom();
            // Auto-save periodically
            if state.messages.len().is_multiple_of(10) {
                state.save_conversation();
            }
        }
        AgentEvent::Error(e) => {
            state.is_processing = false;
            state.add_message(MessageRole::System, format!("Error: {e}"));
            state.status_text = "Error".into();
        }
        AgentEvent::BrowserFetchStart { .. }
        | AgentEvent::BrowserFetchComplete { .. }
        | AgentEvent::BrowserFetchError { .. } => {}
    }
}

fn handle_key(state: &mut AppState, key: KeyEvent, user_input_tx: &mpsc::UnboundedSender<String>) {
    // Handle approval prompt first
    if state.pending_approval.is_some() {
        handle_approval_key(state, key);
        return;
    }

    match (key.modifiers, key.code) {
        // Quit
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            if state.is_processing {
                // First Ctrl+C aborts the running agent task.
                if let Some(handle) = state.agent_task.take() {
                    handle.abort();
                }
                // Unblock any pending approval.
                let sender = state
                    .approval_tx
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                if let Some(tx) = sender {
                    let _ = tx.send(false);
                }
                state.pending_approval = None;
                state.is_processing = false;
                state.status_text = "Cancelled".into();
                state.add_message(MessageRole::System, "Request cancelled.".into());
            } else {
                state.should_quit = true;
            }
        }

        // Toggle file tree
        (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
            state.show_files = !state.show_files;
        }

        // Clear chat
        (KeyModifiers::CONTROL, KeyCode::Char('l')) => {
            state.messages.clear();
            state.add_message(MessageRole::System, "Chat cleared.".into());
            state.scroll_offset = 0;
        }

        // New conversation
        (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
            if state.messages.len() > 1 {
                state.save_conversation();
            }
            state.messages.clear();
            state.conversation_id = ConversationStore::generate_id();
            state.add_message(MessageRole::System, "New conversation started.".into());
            state.scroll_offset = 0;
            state.iterations = 0;
            state.total_tokens_in = 0;
            state.total_tokens_out = 0;
            state.estimated_cost = 0.0;
        }

        // Save conversation
        (KeyModifiers::CONTROL, KeyCode::Char('s')) => {
            state.save_conversation();
            state.add_message(MessageRole::System, "Conversation saved.".into());
        }

        // Submit input
        (_, KeyCode::Enter) => {
            if state.input.is_empty() || state.is_processing {
                return;
            }

            let input = state.input.clone();
            state.input.clear();
            state.cursor_pos = 0;
            state.push_history(input.clone());

            // Handle slash commands
            if input.starts_with('/') {
                handle_command_result(state, commands::handle_command(&input), user_input_tx);
                return;
            }

            state.add_message(MessageRole::User, input.clone());
            state.is_processing = true;
            state.status_text = "Sending...".into();
            state.last_user_input = input.clone();
            let agent_input = apply_mode_prefix(&state.ai_mode, &input);
            let _ = user_input_tx.send(agent_input);
        }

        // Input editing
        (_, KeyCode::Backspace) => {
            if state.cursor_pos > 0 && !state.is_processing {
                state.input.remove(state.cursor_pos - 1);
                state.cursor_pos -= 1;
            }
        }
        (_, KeyCode::Delete) => {
            if state.cursor_pos < state.input.len() && !state.is_processing {
                state.input.remove(state.cursor_pos);
            }
        }
        (_, KeyCode::Left) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Word jump left
                state.cursor_pos = word_boundary_left(&state.input, state.cursor_pos);
            } else {
                state.cursor_pos = state.cursor_pos.saturating_sub(1);
            }
        }
        (_, KeyCode::Right) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Word jump right
                state.cursor_pos = word_boundary_right(&state.input, state.cursor_pos);
            } else if state.cursor_pos < state.input.len() {
                state.cursor_pos += 1;
            }
        }
        (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
            state.cursor_pos = 0;
        }
        (_, KeyCode::Home) => {
            state.cursor_pos = 0;
        }
        (_, KeyCode::End) => {
            state.cursor_pos = state.input.len();
        }

        // Scroll (Shift+arrows, must come before bare arrows)
        (KeyModifiers::SHIFT, KeyCode::Up) => {
            state.scroll_offset = state.scroll_offset.saturating_sub(1);
        }
        (KeyModifiers::SHIFT, KeyCode::Down) => {
            state.scroll_offset = state.scroll_offset.saturating_add(1);
        }
        (_, KeyCode::PageUp) => {
            state.scroll_offset = state.scroll_offset.saturating_sub(20);
        }
        (_, KeyCode::PageDown) => {
            state.scroll_offset = state.scroll_offset.saturating_add(20);
        }

        // History navigation
        (_, KeyCode::Up) => {
            if !state.is_processing {
                state.history_prev();
            }
        }
        (_, KeyCode::Down) => {
            if !state.is_processing {
                state.history_next();
            }
        }

        // Kill line (Ctrl+U)
        (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
            state.input.drain(..state.cursor_pos);
            state.cursor_pos = 0;
        }

        // Kill to end of line (Ctrl+K)
        (KeyModifiers::CONTROL, KeyCode::Char('k')) => {
            state.input.truncate(state.cursor_pos);
        }

        // Delete word backward (Ctrl+W)
        (KeyModifiers::CONTROL, KeyCode::Char('w')) => {
            let new_pos = word_boundary_left(&state.input, state.cursor_pos);
            state.input.drain(new_pos..state.cursor_pos);
            state.cursor_pos = new_pos;
        }

        // Clipboard paste (Ctrl+V)
        (KeyModifiers::CONTROL, KeyCode::Char('v')) => {
            if !state.is_processing {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if let Ok(text) = cb.get_text() {
                        let text = text.replace('\r', ""); // strip CR
                        state.input.insert_str(state.cursor_pos, &text);
                        state.cursor_pos += text.len();
                    }
                }
            }
        }

        // Tab completion for commands
        (_, KeyCode::Tab) => {
            if state.input.starts_with('/') {
                if let Some(completion) = complete_command(&state.input) {
                    state.input = completion;
                    state.cursor_pos = state.input.len();
                }
            }
        }

        // Regular character input
        (_, KeyCode::Char(c)) => {
            if !state.is_processing {
                state.input.insert(state.cursor_pos, c);
                state.cursor_pos += 1;
            }
        }

        _ => {}
    }
}

fn handle_approval_key(state: &mut AppState, key: KeyEvent) {
    /// Helper: drain the oneshot sender from state and send the user's decision.
    fn send_approval(state: &mut AppState, approved: bool) {
        let sender = state
            .approval_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(tx) = sender {
            // Ignore send errors (agent may have been aborted).
            let _ = tx.send(approved);
        }
    }

    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            send_approval(state, true);
            state.pending_approval = None;
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            send_approval(state, false);
            state.add_message(MessageRole::System, "Tool execution denied.".into());
            state.pending_approval = None;
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            // Auto-approve this and all future tools.
            state
                .approval_manager
                .set_mode(ToolApprovalMode::AutoApprove);
            send_approval(state, true);
            state.pending_approval = None;
            state.add_message(
                MessageRole::System,
                "All tools auto-approved for this session.".into(),
            );
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            // Allow this one tool for the session (record then approve).
            if let Some(ref approval) = state.pending_approval {
                state.approval_manager.record_approval(&approval.tool_name);
            }
            send_approval(state, true);
            state.pending_approval = None;
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            send_approval(state, false);
            state.pending_approval = None;
            state.is_processing = false;
            state.status_text = "Cancelled".into();
        }
        _ => {}
    }
}

fn handle_command_result(
    state: &mut AppState,
    result: CommandResult,
    user_input_tx: &mpsc::UnboundedSender<String>,
) {
    match result {
        CommandResult::Message(msg) => {
            state.add_message(MessageRole::System, msg);
        }
        CommandResult::Clear => {
            state.messages.clear();
            state.add_message(MessageRole::System, "Chat cleared.".into());
            state.scroll_offset = 0;
        }
        CommandResult::Quit => {
            state.should_quit = true;
        }
        CommandResult::ModelChanged(model) => {
            state.model_name = model.clone();
            state.add_message(MessageRole::System, format!("Model changed to: {model}"));
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
        CommandResult::ProviderChanged(provider) => {
            state.provider_name = provider.clone();
            state.add_message(
                MessageRole::System,
                format!("Provider changed to: {provider}. Restart to take effect."),
            );
        }
        CommandResult::Compact => {
            let msg_count = state.messages.len();

            // Only compact if we have enough messages to make it worthwhile
            if msg_count <= 8 {
                state.add_message(
                    MessageRole::System,
                    "Conversation is too short to compact (need more than 8 messages).".into(),
                );
                return;
            }

            // Find the first user message (for context)
            let first_user_idx = state.messages.iter().position(
                |item| matches!(item, ChatItem::Message(m) if m.role == MessageRole::User),
            );

            if first_user_idx.is_none() {
                state.add_message(
                    MessageRole::System,
                    "No user messages found to compact.".into(),
                );
                return;
            }

            let first_user_idx = first_user_idx.unwrap();

            // Keep:
            // - Everything before the first user message (system welcome, etc.)
            // - The first user message
            // - The last 6 messages
            // Replace everything in between with a summary message

            let keep_recent = 6;
            let split_point = msg_count.saturating_sub(keep_recent);

            // Only compact if there's something to compact between first user and last 6
            if split_point <= first_user_idx + 1 {
                state.add_message(
                    MessageRole::System,
                    "Conversation is too short to compact effectively.".into(),
                );
                return;
            }

            // Calculate how many messages we're compacting
            let compacted_count = split_point - (first_user_idx + 1);

            if compacted_count == 0 {
                state.add_message(MessageRole::System, "Nothing to compact.".into());
                return;
            }

            // Build a structured summary from the compacted messages
            let compacted_items = &state.messages[(first_user_idx + 1)..split_point];
            let summary = build_compaction_summary(compacted_items);

            // Build new message list
            let mut new_messages: Vec<ChatItem> = Vec::new();

            // Keep everything up to and including the first user message
            for i in 0..=first_user_idx {
                new_messages.push(state.messages[i].clone());
            }

            // Add the compaction summary as a system message
            new_messages.push(ChatItem::Message(ChatMessage {
                role: MessageRole::System,
                content: summary,
                timestamp: now_str(),
            }));

            // Keep the last 6 messages
            for i in split_point..msg_count {
                new_messages.push(state.messages[i].clone());
            }

            // Replace the messages
            state.messages = new_messages;

            state.add_message(
                MessageRole::System,
                format!("Conversation compacted: {} messages reduced to {}. Token savings will apply on next request.",
                    msg_count, state.messages.len() - 1), // -1 to exclude this message we just added
            );
        }
        CommandResult::SaveConversation => {
            state.save_conversation();
            state.add_message(MessageRole::System, "Conversation saved.".into());
        }
        CommandResult::LoadConversation(id) => match state.conversation_store.load(&id) {
            Ok(conv) => {
                state.messages.clear();
                state.conversation_id = conv.metadata.id;
                for msg in conv.messages {
                    let role = match msg.role.as_str() {
                        "user" => MessageRole::User,
                        "assistant" => MessageRole::Assistant,
                        "tool" => MessageRole::Tool,
                        _ => MessageRole::System,
                    };
                    state.messages.push(ChatItem::Message(ChatMessage {
                        role,
                        content: msg.content,
                        timestamp: msg.timestamp,
                    }));
                }
                state.add_message(
                    MessageRole::System,
                    format!("Loaded conversation: {}", conv.metadata.title),
                );
                state.scroll_to_bottom();
            }
            Err(e) => {
                state.add_message(
                    MessageRole::System,
                    format!("Failed to load conversation: {e}"),
                );
            }
        },
        CommandResult::ListConversations => match state.conversation_store.list_recent(20) {
            Ok(convs) => {
                if convs.is_empty() {
                    state.add_message(MessageRole::System, "No saved conversations.".into());
                } else {
                    let mut list = String::from("Recent conversations:\n");
                    for c in &convs {
                        list.push_str(&format!(
                            "  {} | {} | {} msgs | {}\n",
                            &c.id[..8.min(c.id.len())],
                            c.title,
                            c.message_count,
                            c.updated_at,
                        ));
                    }
                    list.push_str("\nUse /load <id> to resume a conversation.");
                    state.add_message(MessageRole::System, list);
                }
            }
            Err(e) => {
                state.add_message(
                    MessageRole::System,
                    format!("Failed to list conversations: {e}"),
                );
            }
        },
        CommandResult::NewConversation => {
            if state.messages.len() > 1 {
                state.save_conversation();
            }
            state.messages.clear();
            state.conversation_id = ConversationStore::generate_id();
            state.add_message(MessageRole::System, "New conversation started.".into());
            state.scroll_offset = 0;
        }
        CommandResult::SetApprovalMode(mode) => {
            let new_mode = match mode.as_str() {
                "auto" => ToolApprovalMode::AutoApprove,
                "ask" => ToolApprovalMode::AlwaysAsk,
                "ask-once" | "askonce" => ToolApprovalMode::AskOnce,
                _ => {
                    state.add_message(
                        MessageRole::System,
                        "Invalid mode. Use: auto, ask, or ask-once".into(),
                    );
                    return;
                }
            };
            state.approval_manager.set_mode(new_mode);
            state.add_message(
                MessageRole::System,
                format!("Tool approval mode set to: {mode}"),
            );
        }
        CommandResult::ShowStatus => {
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".into());
            let status = format!(
                "Status:\n\
                 Provider: {}\n\
                 Model: {}\n\
                 Working dir: {}\n\
                 Tokens: {}in / {}out\n\
                 Cost: ${:.4}\n\
                 Messages: {}\n\
                 Iterations: {}",
                state.provider_name,
                state.model_name,
                cwd,
                state.total_tokens_in,
                state.total_tokens_out,
                state.estimated_cost,
                state.messages.len(),
                state.iterations,
            );
            state.add_message(MessageRole::System, status);
        }
        CommandResult::ShowDiff => {
            let diff = std::process::Command::new("git")
                .args(["diff"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_else(|| "Not a git repository".into());

            if diff.trim().is_empty() || diff == "Not a git repository" {
                state.add_message(MessageRole::System, diff);
                return;
            }

            // Header
            state.add_message(MessageRole::System, "Git diff:".into());

            let all_lines: Vec<&str> = diff.lines().collect();
            let display_lines = &all_lines[..all_lines.len().min(100)];

            for line in display_lines {
                let role = if line.starts_with("+++") || line.starts_with("---") {
                    MessageRole::DiffHeader
                } else if line.starts_with('+') {
                    MessageRole::DiffAdd
                } else if line.starts_with('-') {
                    MessageRole::DiffRemove
                } else if line.starts_with("@@") {
                    MessageRole::DiffHeader
                } else {
                    MessageRole::Tool
                };
                state.add_message(role, line.to_string());
            }

            if all_lines.len() > 100 {
                state.add_message(
                    MessageRole::System,
                    format!("... (truncated at 100 lines, {} total)", all_lines.len()),
                );
            }
        }
        CommandResult::ShowGitStatus => {
            // Get branch name
            let branch = std::process::Command::new("git")
                .args(["branch", "--show-current"])
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "unknown".into());

            // Get status
            let status_output = std::process::Command::new("git")
                .args(["status", "--porcelain"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_else(|| "".into());

            let mut formatted = format!("On branch: {}\n\n", branch);

            if status_output.is_empty() {
                formatted.push_str("Working tree clean");
            } else {
                // Parse and format with state labels
                for line in status_output.lines() {
                    if line.len() < 3 {
                        continue;
                    }
                    let status_code = &line[..2];
                    let path = line[3..].trim();

                    let (label, _color) = match status_code {
                        "M " | " M" | "MM" => ("M", "modified"),
                        "A " | " A" | "AM" => ("A", "added"),
                        "D " | " D" => ("D", "deleted"),
                        "R " | " R" => ("R", "renamed"),
                        "??" => ("?", "untracked"),
                        "UU" | "AA" | "DD" => ("U", "conflicted"),
                        _ => ("M", "modified"),
                    };

                    formatted.push_str(&format!("  {} {}\n", label, path));
                }
            }

            state.add_message(MessageRole::System, formatted);
        }
        CommandResult::ShowLog => {
            let log = std::process::Command::new("git")
                .args(["log", "--oneline", "-20"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_else(|| "Not a git repository".into());
            state.add_message(
                MessageRole::System,
                format!("Git log (last 20 commits):\n{log}"),
            );
        }
        CommandResult::SearchFiles(pattern) => {
            let result = std::process::Command::new("find")
                .args([".", "-name", &pattern, "-not", "-path", "./.git/*"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_else(|| "Search failed".into());
            let lines: Vec<&str> = result.lines().take(50).collect();
            state.add_message(
                MessageRole::System,
                format!("Files matching '{}':\n{}", pattern, lines.join("\n")),
            );
        }
        CommandResult::NotACommand => {}
        CommandResult::ListModels => {
            // Show static models for the active provider
            let provider = match state.provider_name.to_lowercase().as_str() {
                s if s.contains("claude") || s.contains("anthropic") => {
                    phazeai_core::ProviderId::Claude
                }
                s if s.contains("openai") => phazeai_core::ProviderId::OpenAI,
                s if s.contains("ollama") => phazeai_core::ProviderId::Ollama,
                s if s.contains("groq") => phazeai_core::ProviderId::Groq,
                s if s.contains("together") => phazeai_core::ProviderId::Together,
                s if s.contains("openrouter") => phazeai_core::ProviderId::OpenRouter,
                s if s.contains("lm studio") || s.contains("lmstudio") => {
                    phazeai_core::ProviderId::LmStudio
                }
                _ => phazeai_core::ProviderId::Claude,
            };

            let models = phazeai_core::ProviderRegistry::known_models(&provider);

            if models.is_empty() {
                state.add_message(MessageRole::System,
                    format!("No static model list for {}. Local models must be discovered.\nTip: Use /discover to scan for local models.", provider));
            } else {
                let mut msg = format!("Available models for {}:\n", provider);
                for m in &models {
                    let active = if m.id == state.model_name {
                        " (active)"
                    } else {
                        ""
                    };
                    msg.push_str(&format!(
                        "  {} - {} | ctx:{} | tools:{}{}\n",
                        m.id,
                        m.name,
                        m.context_window,
                        if m.supports_tools { "yes" } else { "no" },
                        active
                    ));
                }
                msg.push_str("\nUse /model <id> to switch models.");
                state.add_message(MessageRole::System, msg);
            }
        }
        CommandResult::DiscoverModels => {
            state.add_message(MessageRole::System, "Scanning for local models...".into());

            // Check Ollama via CLI
            let mut msg = String::from("Local model discovery:\n");

            let ollama_result = std::process::Command::new("ollama").arg("list").output();

            match ollama_result {
                Ok(output) if output.status.success() => {
                    let list = String::from_utf8_lossy(&output.stdout);
                    msg.push_str(&format!("\n  Ollama models:\n{}\n", list));
                }
                _ => {
                    msg.push_str("\n  Ollama: not running or not installed\n");
                }
            }

            // Check LM Studio (port 1234)
            let lm_check = std::net::TcpStream::connect_timeout(
                &"127.0.0.1:1234".parse().unwrap(),
                std::time::Duration::from_millis(500),
            );
            if lm_check.is_ok() {
                msg.push_str("  LM Studio: running on port 1234\n");
            } else {
                msg.push_str("  LM Studio: not detected\n");
            }

            msg.push_str("\nUse /provider ollama and /model <name> to use a local model.");
            state.add_message(MessageRole::System, msg);
        }
        CommandResult::ShowContext => {
            use phazeai_core::context::ProjectType;

            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".into());

            let mut context_info = format!("Project context:\n  Working dir: {}\n", cwd);

            // Check for instruction files
            let root = std::env::current_dir().unwrap_or_default();
            let candidates = [
                root.join("CLAUDE.md"),
                root.join(".phazeai/instructions.md"),
                root.join(".ai/instructions.md"),
            ];

            let mut found_instructions = false;
            for path in &candidates {
                if path.exists() {
                    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                    context_info.push_str(&format!(
                        "  Instructions: {} ({} bytes)\n",
                        path.display(),
                        size
                    ));
                    found_instructions = true;
                    break;
                }
            }

            if !found_instructions {
                // Check parent directories
                let mut current = root.parent();
                while let Some(dir) = current {
                    let parent_claude = dir.join("CLAUDE.md");
                    if parent_claude.exists() {
                        let size = std::fs::metadata(&parent_claude)
                            .map(|m| m.len())
                            .unwrap_or(0);
                        context_info.push_str(&format!(
                            "  Parent instructions: {} ({} bytes)\n",
                            parent_claude.display(),
                            size
                        ));
                        break;
                    }
                    current = dir.parent();
                }
            }

            // Check global instructions
            if let Some(home) = dirs::home_dir() {
                let global_instructions = home.join(".phazeai").join("instructions.md");
                if global_instructions.exists() {
                    let size = std::fs::metadata(&global_instructions)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    context_info.push_str(&format!(
                        "  Global instructions: {} ({} bytes)\n",
                        global_instructions.display(),
                        size
                    ));
                }
            }

            // Check project type
            let project_type = ProjectType::detect(&root);
            context_info.push_str(&format!("  Project type: {}\n", project_type.name()));

            // Check git
            let (branch, dirty) = phazeai_core::collect_git_info(&root);
            if let Some(b) = branch {
                context_info.push_str(&format!("  Git branch: {}\n", b));
                if !dirty.is_empty() {
                    context_info.push_str(&format!("  Modified files: {}\n", dirty.len()));
                }
            }

            state.add_message(MessageRole::System, context_info);
        }
        CommandResult::SetMode(mode) => {
            state.ai_mode = mode.clone();
            let description = match mode.as_str() {
                "plan" => "Planning mode: creates structured plans and checklists",
                "debug" => "Debug mode: focuses on diagnosing and fixing issues",
                "ask" => "Ask mode: read-only, answers questions about your code",
                "edit" => "Edit mode: makes targeted code changes",
                "chat" => "Chat mode: general conversation and assistance",
                _ => "Mode changed",
            };
            state.add_message(MessageRole::System, format!("Mode: {mode} — {description}"));
        }
        CommandResult::AddFile(path_str) => {
            let path = std::path::Path::new(&path_str);
            match std::fs::read_to_string(path) {
                Ok(contents) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or(path_str.clone());
                    let char_count = contents.chars().count();
                    let context_msg = format!(
                        "[File context added: {name} ({char_count} chars)]\n\n```\n{}\n```",
                        contents
                    );
                    state.add_message(
                        MessageRole::System,
                        format!("Added file: {path_str} ({char_count} chars)"),
                    );
                    // Send file contents as a context message to the agent
                    if !state.is_processing {
                        let _ = user_input_tx.send(format!(
                            "I'm providing the contents of `{name}` for context. You don't need to respond yet, just acknowledge briefly.\n\n{context_msg}"
                        ));
                        state.is_processing = true;
                        state.status_text = "Loading file context...".into();
                    }
                }
                Err(e) => {
                    state.add_message(
                        MessageRole::System,
                        format!("Failed to read file '{path_str}': {e}"),
                    );
                }
            }
        }
        CommandResult::Retry => {
            let last = state.last_user_input.clone();
            if last.is_empty() {
                state.add_message(MessageRole::System, "Nothing to retry.".into());
            } else if state.is_processing {
                state.add_message(
                    MessageRole::System,
                    "Agent is still running. Wait for it to finish.".into(),
                );
            } else {
                state.add_message(MessageRole::User, format!("[retry] {last}"));
                state.is_processing = true;
                state.status_text = "Retrying...".into();
                let agent_input = apply_mode_prefix(&state.ai_mode, &last);
                let _ = user_input_tx.send(agent_input);
            }
        }
        CommandResult::Cancel => {
            // Abort the running agent task immediately.
            if let Some(handle) = state.agent_task.take() {
                handle.abort();
            }
            // Also unblock any pending approval by sending false.
            let sender = state
                .approval_tx
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(tx) = sender {
                let _ = tx.send(false);
            }
            state.pending_approval = None;
            state.is_processing = false;
            state.status_text = "Cancelled".into();
            state.add_message(
                MessageRole::System,
                "Agent task aborted.".into(),
            );
        }
        CommandResult::Grep(pattern) => {
            let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let output = std::process::Command::new("rg")
                .args([
                    "--color=never",
                    "--line-number",
                    "--with-filename",
                    &pattern,
                ])
                .current_dir(&root)
                .output()
                .or_else(|_| {
                    // Fallback to grep if rg not available
                    std::process::Command::new("grep")
                        .args(["-rn", &pattern, "."])
                        .current_dir(&root)
                        .output()
                });
            match output {
                Ok(out) => {
                    let text = String::from_utf8_lossy(&out.stdout);
                    if text.is_empty() {
                        state.add_message(
                            MessageRole::System,
                            format!("No matches for '{pattern}'"),
                        );
                    } else {
                        let lines: Vec<&str> = text.lines().take(50).collect();
                        let result = lines.join("\n");
                        let truncated = if text.lines().count() > 50 {
                            "\n... (showing first 50 matches)"
                        } else {
                            ""
                        };
                        state.add_message(
                            MessageRole::System,
                            format!("Grep results for '{pattern}':\n{result}{truncated}"),
                        );
                    }
                }
                Err(e) => {
                    state.add_message(MessageRole::System, format!("Search failed: {e}"));
                }
            }
        }
    }
}

// ── Helper functions ────────────────────────────────────────────────────

fn word_boundary_left(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let bytes = s.as_bytes();
    let mut i = pos - 1;
    // Skip whitespace
    while i > 0 && bytes[i] == b' ' {
        i -= 1;
    }
    // Skip word characters
    while i > 0 && bytes[i] != b' ' {
        i -= 1;
    }
    if bytes[i] == b' ' && i > 0 {
        i + 1
    } else {
        i
    }
}

fn word_boundary_right(s: &str, pos: usize) -> usize {
    let len = s.len();
    if pos >= len {
        return len;
    }
    let bytes = s.as_bytes();
    let mut i = pos;
    // Skip current word
    while i < len && bytes[i] != b' ' {
        i += 1;
    }
    // Skip whitespace
    while i < len && bytes[i] == b' ' {
        i += 1;
    }
    i
}

fn complete_command(input: &str) -> Option<String> {
    let commands = [
        "/help",
        "/exit",
        "/quit",
        "/clear",
        "/model",
        "/models",
        "/mode",
        "/provider",
        "/theme",
        "/files",
        "/tree",
        "/compact",
        "/save",
        "/load",
        "/conversations",
        "/history",
        "/new",
        "/config",
        "/status",
        "/approve",
        "/diff",
        "/git",
        "/log",
        "/search",
        "/pwd",
        "/cd",
        "/cost",
        "/version",
        "/context",
        "/discover",
        "/plan",
        "/debug",
        "/ask",
        "/edit",
        "/chat",
        "/add",
        "/retry",
        "/cancel",
        "/grep",
        "/yolo",
    ];

    let matches: Vec<&&str> = commands.iter().filter(|c| c.starts_with(input)).collect();

    if matches.len() == 1 {
        Some(format!("{} ", matches[0]))
    } else {
        None
    }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Build a structured summary from compacted messages.
/// Extracts user requests and key assistant responses to preserve context.
fn build_compaction_summary(items: &[ChatItem]) -> String {
    let mut summary = String::from("[Conversation Summary]\n");

    let mut user_requests: Vec<String> = Vec::new();
    let mut assistant_actions: Vec<String> = Vec::new();
    let mut tool_results: Vec<String> = Vec::new();

    for item in items {
        match item {
            ChatItem::Message(msg) => match msg.role {
                MessageRole::User => {
                    // Extract the first line or first 120 chars as the request summary
                    let text = msg.content.trim();
                    let brief = if let Some(first_line) = text.lines().next() {
                        if first_line.len() > 120 {
                            format!("{}...", &first_line[..117])
                        } else {
                            first_line.to_string()
                        }
                    } else {
                        continue;
                    };
                    user_requests.push(brief);
                }
                MessageRole::Assistant => {
                    // Extract the first meaningful line (skip empty lines)
                    let text = msg.content.trim();
                    let brief = text
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .to_string();
                    if !brief.is_empty() {
                        let truncated = if brief.len() > 150 {
                            format!("{}...", &brief[..147])
                        } else {
                            brief
                        };
                        assistant_actions.push(truncated);
                    }
                }
                MessageRole::Tool
                | MessageRole::DiffAdd
                | MessageRole::DiffRemove
                | MessageRole::DiffHeader => {
                    // Capture tool execution summaries (first line only)
                    let text = msg.content.trim();
                    if let Some(first_line) = text.lines().next() {
                        let truncated = if first_line.len() > 100 {
                            format!("{}...", &first_line[..97])
                        } else {
                            first_line.to_string()
                        };
                        tool_results.push(truncated);
                    }
                }
                MessageRole::System => {
                    // Skip system messages in summary
                }
            },
            ChatItem::ToolCard { name, output, .. } => {
                // Summarize tool cards as tool results
                let brief = if output.is_empty() {
                    format!("[tool: {name}]")
                } else {
                    let first_line = output.lines().next().unwrap_or("");
                    if first_line.len() > 100 {
                        format!("[{name}] {}...", &first_line[..97])
                    } else {
                        format!("[{name}] {first_line}")
                    }
                };
                tool_results.push(brief);
            }
        }
    }

    if !user_requests.is_empty() {
        summary.push_str("User requested:\n");
        for (i, req) in user_requests.iter().enumerate().take(10) {
            summary.push_str(&format!("  {}. {}\n", i + 1, req));
        }
        if user_requests.len() > 10 {
            summary.push_str(&format!(
                "  ... and {} more requests\n",
                user_requests.len() - 10
            ));
        }
    }

    if !assistant_actions.is_empty() {
        summary.push_str("Assistant actions:\n");
        for action in assistant_actions.iter().take(8) {
            summary.push_str(&format!("  - {}\n", action));
        }
        if assistant_actions.len() > 8 {
            summary.push_str(&format!(
                "  ... and {} more actions\n",
                assistant_actions.len() - 8
            ));
        }
    }

    if !tool_results.is_empty() {
        summary.push_str("Tool outputs:\n");
        for result in tool_results.iter().take(5) {
            summary.push_str(&format!("  - {}\n", result));
        }
        if tool_results.len() > 5 {
            summary.push_str(&format!(
                "  ... and {} more results\n",
                tool_results.len() - 5
            ));
        }
    }

    summary
}

fn now_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    format!("{:02}:{:02}", h, m)
}

/// Attempt to start the Python sidecar for semantic search.
/// Returns None if Python is unavailable or the sidecar script doesn't exist.
async fn try_start_sidecar() -> Option<phazeai_sidecar::SidecarClient> {
    // Look for python3 or python
    let python = if phazeai_sidecar::SidecarManager::check_python("python3").await {
        "python3"
    } else if phazeai_sidecar::SidecarManager::check_python("python").await {
        "python"
    } else {
        return None;
    };

    // Look for the sidecar script: project-local first, then config directory
    let cwd = std::env::current_dir().ok()?;
    let local_path = cwd.join("sidecar").join("server.py");
    let config_path =
        dirs::config_dir().map(|d| d.join("phazeai").join("sidecar").join("server.py"));

    let script_path = if local_path.exists() {
        local_path
    } else if config_path.as_ref().is_some_and(|p| p.exists()) {
        config_path.unwrap()
    } else {
        return None;
    };

    let mut manager = phazeai_sidecar::SidecarManager::new(python, &script_path);
    if manager.start().await.is_err() {
        return None;
    }

    let process = manager.take_process()?;
    phazeai_sidecar::SidecarClient::from_process(process).ok()
}
