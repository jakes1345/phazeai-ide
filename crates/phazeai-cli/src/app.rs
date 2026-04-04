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
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
    Terminal,
};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use pulldown_cmark::{Event as MdEvent, Options as MdOptions, Parser as MdParser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::commands::{self, CommandResult};
use crate::companion::Companion;
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

enum WorkerCommand {
    UserMessage(String),
    ClearHistory,
    SwapModel { settings: Box<Settings> },
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

    // Session Picker UI
    show_session_picker: bool,
    session_picker_list: Vec<ConversationMetadata>,
    session_picker_index: usize,

    // Tool approval
    approval_manager: ToolApprovalManager,

    /// Current AI mode (chat, ask, debug, plan, edit)
    ai_mode: String,
    /// Last user message sent, used by /retry
    last_user_input: String,

    agent_task: Option<tokio::task::JoinHandle<()>>,
    cancel_token: Arc<AtomicBool>,

    approval_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<bool>>>>,

    // Companion buddy
    companion: Companion,
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
                    "PhazeAI v{}  ·  {}  ·  {}\n\
                     {}\n\n\
                     ╭──────────────────────────────────────╮\n\
                     │  Enter    Send       Ctrl+C  Quit    │\n\
                     │  Ctrl+B   Files      Ctrl+N  New     │\n\
                     │  Ctrl+O   Sessions   Ctrl+E  Editor  │\n\
                     │  /help    Commands   /mode   Modes   │\n\
                     ╰──────────────────────────────────────╯",
                    env!("CARGO_PKG_VERSION"),
                    provider_name,
                    settings.llm.model,
                    cwd
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

            show_session_picker: false,
            session_picker_list: Vec::new(),
            session_picker_index: 0,

            approval_manager: ToolApprovalManager::default(),
            ai_mode: "chat".into(),
            last_user_input: String::new(),

            agent_task: None,
            cancel_token: Arc::new(AtomicBool::new(false)),
            approval_tx: Arc::new(Mutex::new(None)),

            companion: Companion::new(),
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
    let (user_input_tx, mut user_input_rx) = mpsc::unbounded_channel::<WorkerCommand>();

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

        let cancel_token = state.cancel_token.clone();
        let handle = tokio::spawn(async move {
            let mut agent = Agent::new(llm)
                .with_system_prompt(system_prompt)
                .with_approval(approval_fn)
                .with_cancel_token(cancel_token.clone());

            // Connect to MCP servers
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let mcp_configs = phazeai_core::mcp::McpManager::load_config(&cwd);
            if !mcp_configs.is_empty() {
                let mut mcp_manager = phazeai_core::mcp::McpManager::new();
                mcp_manager.connect_all(&mcp_configs);
                agent.register_mcp_tools(std::sync::Arc::new(std::sync::Mutex::new(mcp_manager)));
            }

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

            while let Some(cmd) = user_input_rx.recv().await {
                let input = match cmd {
                    WorkerCommand::ClearHistory => {
                        agent.clear_conversation().await;
                        continue;
                    }
                    WorkerCommand::SwapModel {
                        settings: new_settings,
                    } => {
                        if let Ok(new_llm) = new_settings.build_llm_client() {
                            agent.swap_llm(new_llm);
                        }
                        continue;
                    }
                    WorkerCommand::UserMessage(msg) => msg,
                };
                cancel_token.store(false, Ordering::Relaxed);

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
                    let err_str = e.to_string();
                    if err_str != "Cancelled" {
                        let _ = event_tx.send(AgentEvent::Error(err_str));
                    }
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
            "memory".into(),
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

    // Main vertical layout: header + chat + companion + input + status
    let input_height = if state.pending_approval.is_some() {
        5
    } else {
        3
    };

    // Tick the companion each frame
    state.companion.tick();
    if !state.is_processing && state.pending_approval.is_none() {
        state.companion.on_idle();
    }

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // header bar
            Constraint::Min(5),               // chat
            Constraint::Length(3),            // companion buddy
            Constraint::Length(input_height), // input (or approval prompt)
            Constraint::Length(1),            // status
        ])
        .split(f.area());

    // Header bar
    draw_header_bar(f, main_chunks[0], state, theme);

    // Optionally split chat area for file tree
    let chat_area = if state.show_files {
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(40)])
            .split(main_chunks[1]);

        draw_file_tree(f, h_chunks[0], theme);
        h_chunks[1]
    } else {
        main_chunks[1]
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
                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                .border_type(BorderType::Rounded)
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
                .begin_symbol(Some("▲"))
                .end_symbol(Some("▼"))
                .track_symbol(Some("│"))
                .thumb_symbol("█"),
            chat_area,
            &mut scrollbar_state,
        );
    }

    // Companion buddy
    crate::companion::draw_companion(f, main_chunks[2], &state.companion, theme);

    // Input or approval prompt
    if let Some(ref approval) = state.pending_approval {
        draw_approval_prompt(f, main_chunks[3], approval, theme);
    } else {
        draw_input(f, main_chunks[3], state, theme);
    }

    // Status bar
    draw_status_bar(f, main_chunks[4], state, theme);

    // Session picker overlay
    if state.show_session_picker {
        draw_session_picker(f, f.area(), state);
    }
}

fn draw_header_bar(f: &mut ratatui::Frame, area: Rect, state: &AppState, theme: &Theme) {
    let mode_label = state.ai_mode.to_uppercase();
    let width = area.width as usize;

    // Build header spans
    let mut spans = vec![
        Span::styled(
            " PhazeAI ",
            Style::default()
                .fg(theme.header_fg)
                .bg(theme.header_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", state.provider_name),
            Style::default().fg(theme.accent).bg(theme.surface),
        ),
        Span::styled(
            format!(" {} ", state.model_name),
            Style::default().fg(theme.fg).bg(theme.surface),
        ),
        Span::styled(" ", Style::default().bg(theme.surface)),
        Span::styled(
            format!(" {} ", mode_label),
            Style::default()
                .fg(theme.header_fg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    // Calculate used width (approximate)
    let used: usize =
        10 + state.provider_name.len() + 2 + state.model_name.len() + 2 + 1 + mode_label.len() + 2;
    let remaining = width.saturating_sub(used);
    if remaining > 0 {
        spans.push(Span::styled(
            " ".repeat(remaining),
            Style::default().bg(theme.surface),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_file_tree(f: &mut ratatui::Frame, area: Rect, theme: &Theme) {
    let cwd = std::env::current_dir()
        .map(|p| {
            let s = p.display().to_string();
            // Show just the last path component
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or(s)
        })
        .unwrap_or_else(|_| ".".into());

    let mut lines: Vec<Line> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(".") {
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (!is_dir, e.file_name())
        });

        let filtered: Vec<_> = entries
            .iter()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                !name.starts_with('.') && name != "target" && name != "node_modules"
            })
            .collect();

        let total = filtered.len();
        let mut count = 0usize;

        for (i, entry) in filtered.iter().enumerate() {
            if count >= 50 {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let is_last = i == total - 1;
            let connector = if is_last { "└─" } else { "├─" };
            let color = if is_dir { theme.accent } else { theme.fg };

            let dir_marker = if is_dir { "/ " } else { "  " };
            lines.push(Line::from(vec![
                Span::styled(format!(" {connector} "), Style::default().fg(theme.dim)),
                Span::styled(name, Style::default().fg(color)),
                Span::styled(dir_marker, Style::default().fg(theme.dim)),
            ]));
            count += 1;

            // Show immediate children of directories
            if is_dir {
                if let Ok(sub) = std::fs::read_dir(entry.path()) {
                    let mut sub: Vec<_> = sub.flatten().collect();
                    sub.sort_by_key(|e| e.file_name());
                    let sub_filtered: Vec<_> = sub
                        .iter()
                        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                        .collect();
                    let sub_total = sub_filtered.len().min(12);
                    let prefix = if is_last { "   " } else { " │ " };

                    for (j, sub_entry) in sub_filtered.iter().take(12).enumerate() {
                        if count >= 50 {
                            break;
                        }
                        let sub_name = sub_entry.file_name().to_string_lossy().to_string();
                        let sub_is_dir = sub_entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        let sub_last = j == sub_total - 1;
                        let sub_conn = if sub_last { "└─" } else { "├─" };
                        let sub_color = if sub_is_dir {
                            theme.accent
                        } else {
                            theme.muted
                        };
                        let sub_dir_marker = if sub_is_dir { "/ " } else { "  " };

                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("{prefix}{sub_conn} "),
                                Style::default().fg(theme.dim),
                            ),
                            Span::styled(sub_name, Style::default().fg(sub_color)),
                            Span::styled(sub_dir_marker, Style::default().fg(theme.dim)),
                        ]));
                        count += 1;
                    }
                    if sub_filtered.len() > 12 {
                        lines.push(Line::from(Span::styled(
                            format!("{prefix}  +{} more", sub_filtered.len() - 12),
                            Style::default().fg(theme.dim),
                        )));
                    }
                }
            }
        }
    }

    let files_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(format!(" {} ", cwd))
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.border))
        .padding(Padding::new(0, 0, 0, 0));

    let paragraph = Paragraph::new(lines)
        .block(files_block)
        .style(Style::default().fg(theme.fg));

    f.render_widget(paragraph, area);
}

fn render_message_lines(msg: &ChatMessage, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    let (role_label, role_color, role_icon) = match msg.role {
        MessageRole::User => (" You ", theme.user_color, "›"),
        MessageRole::Assistant => (" AI ", theme.assistant_color, "›"),
        MessageRole::System => (" System ", theme.system_color, "·"),
        MessageRole::Tool => (" Tool ", theme.tool_color, "·"),
        MessageRole::DiffAdd => ("", theme.success, "+"),
        MessageRole::DiffRemove => ("", theme.error, "-"),
        MessageRole::DiffHeader => ("", theme.accent, ""),
    };

    if !role_label.is_empty() {
        let header_spans = vec![
            Span::styled(format!(" {role_icon}"), Style::default().fg(role_color)),
            Span::styled(
                role_label.to_string(),
                Style::default().fg(role_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}", msg.timestamp),
                Style::default().fg(theme.dim),
            ),
        ];
        lines.push(Line::from(header_spans));
    }

    if msg.role == MessageRole::Assistant {
        lines.extend(render_markdown_lines(&msg.content, theme));
    } else {
        for raw_line in msg.content.lines() {
            let indent = match msg.role {
                MessageRole::DiffAdd | MessageRole::DiffRemove | MessageRole::DiffHeader => {
                    "    ".to_string()
                }
                _ => "   ".to_string(),
            };
            lines.push(Line::from(Span::styled(
                format!("{indent}{raw_line}"),
                Style::default().fg(role_color),
            )));
        }
    }

    lines
}

fn render_markdown_lines(content: &str, theme: &Theme) -> Vec<Line<'static>> {
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let syntect_theme_name = match theme.name {
        "tokyo-night" | "dracula" | "one-dark" => "base16-ocean.dark",
        "gruvbox" => "base16-eighties.dark",
        "nord" => "base16-ocean.dark",
        "catppuccin" => "base16-mocha.dark",
        _ => "base16-ocean.dark",
    };
    let highlight_theme = ts
        .themes
        .get(syntect_theme_name)
        .unwrap_or_else(|| &ts.themes["base16-ocean.dark"]);

    let opts = MdOptions::ENABLE_STRIKETHROUGH | MdOptions::ENABLE_TABLES;
    let parser = MdParser::new_ext(content, opts);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut bold = false;
    let mut italic = false;
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();
    let mut in_heading = false;

    let mut list_depth: usize = 0;
    let mut ordered_index: Option<u64> = None;

    for event in parser {
        match event {
            MdEvent::Start(Tag::Heading { level, .. }) => {
                flush_line(&mut lines, &mut current_spans);
                in_heading = true;
                let marker = match level {
                    pulldown_cmark::HeadingLevel::H1 => "━━ ",
                    pulldown_cmark::HeadingLevel::H2 => "── ",
                    pulldown_cmark::HeadingLevel::H3 => "─ ",
                    _ => "· ",
                };
                current_spans.push(Span::styled(
                    format!("   {marker}"),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            MdEvent::End(TagEnd::Heading(_)) => {
                in_heading = false;
                flush_line(&mut lines, &mut current_spans);
            }
            MdEvent::Start(Tag::Strong) => bold = true,
            MdEvent::End(TagEnd::Strong) => bold = false,
            MdEvent::Start(Tag::Emphasis) => italic = true,
            MdEvent::End(TagEnd::Emphasis) => italic = false,
            MdEvent::Start(Tag::CodeBlock(kind)) => {
                flush_line(&mut lines, &mut current_spans);
                in_code_block = true;
                code_buffer.clear();
                code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                let lang_display = if code_lang.is_empty() {
                    "code".to_string()
                } else {
                    code_lang.clone()
                };
                lines.push(Line::from(vec![
                    Span::styled("   ╭─".to_string(), Style::default().fg(theme.dim)),
                    Span::styled(
                        format!(" {} ", lang_display),
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("─".repeat(40), Style::default().fg(theme.dim)),
                ]));
            }
            MdEvent::End(TagEnd::CodeBlock) => {
                let syntax = ss
                    .find_syntax_by_token(&code_lang)
                    .unwrap_or_else(|| ss.find_syntax_plain_text());
                let mut h = HighlightLines::new(syntax, highlight_theme);

                let code_snapshot = code_buffer.clone();
                let code_lines_vec: Vec<&str> = code_snapshot.lines().collect();
                let line_num_width = code_lines_vec.len().to_string().len();

                for (i, code_line) in code_lines_vec.iter().enumerate() {
                    let line_num = format!("{:>width$}", i + 1, width = line_num_width);
                    let mut spans: Vec<Span<'static>> = vec![
                        Span::styled("   │".to_string(), Style::default().fg(theme.dim)),
                        Span::styled(format!("{line_num} "), Style::default().fg(theme.dim)),
                    ];
                    if let Ok(ranges) = h.highlight_line(code_line, &ss) {
                        for (style, text) in ranges {
                            if let Ok(span) = syntect_tui::into_span((style, text)) {
                                spans.push(Span::styled(span.content.to_string(), span.style));
                            }
                        }
                    } else {
                        spans.push(Span::styled(
                            code_line.to_string(),
                            Style::default().fg(theme.code_fg),
                        ));
                    }
                    lines.push(Line::from(spans));
                }
                lines.push(Line::from(Span::styled(
                    format!("   ╰─{}", "─".repeat(40)),
                    Style::default().fg(theme.dim),
                )));
                in_code_block = false;
                code_buffer.clear();
            }
            MdEvent::Start(Tag::List(start)) => {
                flush_line(&mut lines, &mut current_spans);
                list_depth += 1;
                ordered_index = start;
            }
            MdEvent::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    ordered_index = None;
                }
            }
            MdEvent::Start(Tag::Item) => {
                flush_line(&mut lines, &mut current_spans);
                let indent = "  ".repeat(list_depth);
                let bullet = if let Some(ref mut idx) = ordered_index {
                    let s = format!("{}. ", idx);
                    *idx += 1;
                    s
                } else {
                    "• ".to_string()
                };
                current_spans.push(Span::styled(
                    format!("   {indent}{bullet}"),
                    Style::default().fg(theme.fg),
                ));
            }
            MdEvent::End(TagEnd::Item) => {
                flush_line(&mut lines, &mut current_spans);
            }
            MdEvent::Start(Tag::Paragraph) => {
                if !in_heading && list_depth == 0 {
                    flush_line(&mut lines, &mut current_spans);
                    if current_spans.is_empty() {
                        current_spans.push(Span::raw("   ".to_string()));
                    }
                }
            }
            MdEvent::End(TagEnd::Paragraph) => {
                flush_line(&mut lines, &mut current_spans);
            }
            MdEvent::Code(code) => {
                current_spans.push(Span::styled(
                    format!(" {code} "),
                    Style::default().fg(theme.code_fg).bg(theme.code_bg),
                ));
            }
            MdEvent::Text(text) => {
                if in_code_block {
                    code_buffer.push_str(&text);
                } else {
                    let style = if in_heading {
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD)
                    } else if bold && italic {
                        Style::default()
                            .fg(theme.fg)
                            .add_modifier(Modifier::BOLD | Modifier::ITALIC)
                    } else if bold {
                        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
                    } else if italic {
                        Style::default().fg(theme.fg).add_modifier(Modifier::ITALIC)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    if current_spans.is_empty() && list_depth == 0 && !in_heading {
                        current_spans.push(Span::raw("   ".to_string()));
                    }
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }
            MdEvent::SoftBreak | MdEvent::HardBreak => {
                flush_line(&mut lines, &mut current_spans);
            }
            MdEvent::Rule => {
                flush_line(&mut lines, &mut current_spans);
                lines.push(Line::from(Span::styled(
                    format!("   {}", "─".repeat(50)),
                    Style::default().fg(theme.dim),
                )));
            }
            _ => {}
        }
    }

    flush_line(&mut lines, &mut current_spans);
    lines
}

fn flush_line(lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

fn render_tool_card_lines(
    name: &str,
    args: &str,
    output: &str,
    success: Option<bool>,
    collapsed: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    let (status_color, status_icon, status_text) = match success {
        None => (theme.warning, "◌", "running"),
        Some(true) => (theme.success, "●", "done"),
        Some(false) => (theme.error, "●", "failed"),
    };

    let collapse_icon = if collapsed { "▸" } else { "▾" };

    lines.push(Line::from(vec![
        Span::styled(
            format!("   {collapse_icon} "),
            Style::default().fg(theme.dim),
        ),
        Span::styled(format!("{status_icon} "), Style::default().fg(status_color)),
        Span::styled(
            name.to_string(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {status_text}"), Style::default().fg(theme.dim)),
    ]));

    if !collapsed {
        // Show tool arguments (truncated)
        if !args.is_empty() {
            let display_args = if args.len() > 100 {
                format!("{}…", &args[..99])
            } else {
                args.to_string()
            };
            lines.push(Line::from(vec![
                Span::styled("     │ ".to_string(), Style::default().fg(theme.dim)),
                Span::styled(display_args, Style::default().fg(theme.muted)),
            ]));
        }

        let content_lines: Vec<&str> = if output.is_empty() {
            vec![]
        } else {
            output.lines().take(10).collect()
        };

        for content_line in &content_lines {
            lines.push(Line::from(vec![
                Span::styled("     │ ".to_string(), Style::default().fg(theme.dim)),
                Span::styled(content_line.to_string(), Style::default().fg(theme.fg)),
            ]));
        }

        let total_output_lines = output.lines().count();
        if total_output_lines > 10 {
            lines.push(Line::from(Span::styled(
                format!("     │ +{} more lines", total_output_lines - 10),
                Style::default().fg(theme.dim),
            )));
        }

        if content_lines.is_empty() && success.is_none() {
            let spinners = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let tick = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 80) as usize;
            let frame = spinners[tick % spinners.len()];
            lines.push(Line::from(Span::styled(
                format!("     │ {frame} executing..."),
                Style::default().fg(theme.muted),
            )));
        }
    }

    lines
}

fn build_chat_lines(
    messages: &[ChatItem],
    is_processing: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut chat_lines: Vec<Line<'static>> = Vec::new();

    for (idx, item) in messages.iter().enumerate() {
        match item {
            ChatItem::Message(msg) => {
                // Add a subtle separator between conversation turns (not before first, not for system)
                if idx > 0 && matches!(msg.role, MessageRole::User | MessageRole::Assistant) {
                    chat_lines.push(Line::raw(""));
                }
                chat_lines.extend(render_message_lines(msg, theme));
                chat_lines.push(Line::raw(""));
            }
            ChatItem::ToolCard {
                name,
                args,
                output,
                success,
                collapsed,
            } => {
                chat_lines.extend(render_tool_card_lines(
                    name, args, output, *success, *collapsed, theme,
                ));
            }
        }
    }

    if is_processing {
        let spinners = ['⣾', '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷'];
        let tick = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            / 100) as usize;
        let frame = spinners[tick % spinners.len()];
        chat_lines.push(Line::from(vec![
            Span::styled(format!("  {frame} "), Style::default().fg(theme.accent)),
            Span::styled(
                "Thinking...".to_string(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    chat_lines
}

fn draw_approval_prompt(
    f: &mut ratatui::Frame,
    area: Rect,
    approval: &PendingApproval,
    theme: &Theme,
) {
    let truncated_desc = if approval.description.len() > 80 {
        format!("{}…", &approval.description[..79])
    } else {
        approval.description.clone()
    };

    let text = vec![
        Line::from(vec![
            Span::styled(
                " ⚠ ",
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} ", approval.tool_name),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(truncated_desc, Style::default().fg(theme.muted)),
        ]),
        Line::from(vec![
            Span::styled("   ", Style::default()),
            Span::styled(
                " y ",
                Style::default().fg(theme.header_fg).bg(theme.success),
            ),
            Span::styled(" Allow  ", Style::default().fg(theme.fg)),
            Span::styled(" n ", Style::default().fg(theme.header_fg).bg(theme.error)),
            Span::styled(" Deny  ", Style::default().fg(theme.fg)),
            Span::styled(" a ", Style::default().fg(theme.header_fg).bg(theme.accent)),
            Span::styled(" All  ", Style::default().fg(theme.fg)),
            Span::styled(" s ", Style::default().fg(theme.header_fg).bg(theme.accent)),
            Span::styled(" Session", Style::default().fg(theme.fg)),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Approve Tool ")
        .title_style(
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )
        .border_style(Style::default().fg(theme.warning));

    f.render_widget(Paragraph::new(text).block(block), area);
}

fn draw_input(f: &mut ratatui::Frame, area: Rect, state: &AppState, theme: &Theme) {
    let input_style = if state.is_processing {
        Style::default().fg(theme.muted)
    } else {
        Style::default().fg(theme.fg)
    };

    let (border_color, title_style) = if state.is_processing {
        (theme.dim, Style::default().fg(theme.muted))
    } else if state.input.starts_with('/') {
        (
            theme.accent,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (theme.input_active, Style::default().fg(theme.input_active))
    };

    let title = if state.is_processing {
        " processing... ".to_string()
    } else if state.input.starts_with('/') {
        " / command ".to_string()
    } else {
        " message ".to_string()
    };

    let input = Paragraph::new(state.input.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(title)
                .title_style(title_style)
                .border_style(Style::default().fg(border_color)),
        )
        .style(input_style);
    f.render_widget(input, area);

    // Set cursor position in input area
    if !state.is_processing && state.pending_approval.is_none() {
        let cursor_x = area.x + state.cursor_pos as u16 + 1;
        let max_x = area.x + area.width.saturating_sub(2);
        f.set_cursor_position((cursor_x.min(max_x), area.y + 1));
    }
}

fn draw_status_bar(f: &mut ratatui::Frame, area: Rect, state: &AppState, theme: &Theme) {
    let cwd = std::env::current_dir()
        .map(|p| {
            let s = p.display().to_string();
            if let Ok(home) = std::env::var("HOME") {
                if s.starts_with(&home) {
                    return format!("~{}", &s[home.len()..]);
                }
            }
            s
        })
        .unwrap_or_else(|_| ".".into());

    let approval_label = match state.approval_manager.mode() {
        ToolApprovalMode::AutoApprove => "auto",
        ToolApprovalMode::AlwaysAsk => "ask",
        ToolApprovalMode::AskOnce => "once",
    };

    let sep = Span::styled(" · ", Style::default().fg(theme.dim));

    let mut spans = vec![Span::styled(" ", Style::default().bg(theme.surface))];

    // Status/activity indicator
    if state.is_processing {
        let spinners = ['⣾', '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷'];
        let tick = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            / 100) as usize;
        let frame = spinners[tick % spinners.len()];
        spans.push(Span::styled(
            format!("{frame} {}", state.status_text),
            Style::default().fg(theme.accent),
        ));
    } else {
        spans.push(Span::styled(
            state.status_text.clone(),
            Style::default().fg(theme.muted),
        ));
    }

    // Token usage
    if state.total_tokens_in > 0 || state.total_tokens_out > 0 {
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!(
                "↑{} ↓{}",
                format_tokens(state.total_tokens_in),
                format_tokens(state.total_tokens_out),
            ),
            Style::default().fg(theme.muted),
        ));
    }

    // Cost
    if state.estimated_cost > 0.0 {
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!("${:.4}", state.estimated_cost),
            Style::default().fg(theme.warning),
        ));
    }

    // Approval mode
    spans.push(sep.clone());
    spans.push(Span::styled(
        approval_label.to_string(),
        Style::default().fg(theme.dim),
    ));

    // CWD (right-aligned via padding)
    spans.push(sep);
    spans.push(Span::styled(cwd, Style::default().fg(theme.dim)));

    let status = Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.surface));
    f.render_widget(status, area);
}

fn draw_session_picker(f: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let theme = &state.theme;
    let popup_width = area.width.clamp(40, 72);
    let popup_height = area.height.clamp(6, 22);
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    f.render_widget(ratatui::widgets::Clear, popup_area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, meta) in state.session_picker_list.iter().enumerate() {
        let is_selected = i == state.session_picker_index;
        let indicator = if is_selected { " › " } else { "   " };
        let id_short = &meta.id[..8.min(meta.id.len())];
        let title: String = meta.title.chars().take(36).collect();

        let (title_style, meta_style) = if is_selected {
            (
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme.muted),
            )
        } else {
            (
                Style::default().fg(theme.fg),
                Style::default().fg(theme.dim),
            )
        };

        lines.push(Line::from(vec![
            Span::styled(indicator, Style::default().fg(theme.accent)),
            Span::styled(title, title_style),
            Span::styled(
                format!("  {id_short}  {} msgs", meta.message_count),
                meta_style,
            ),
        ]));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "   No saved conversations.",
            Style::default().fg(theme.muted),
        )));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Sessions ")
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .title_bottom(Line::from(" Enter load · Esc close ").alignment(Alignment::Center))
        .border_style(Style::default().fg(theme.border_focused));

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, popup_area);
}

fn handle_agent_event(state: &mut AppState, event: AgentEvent) {
    match event {
        AgentEvent::Thinking { iteration } => {
            state.iterations = iteration;
            state.status_text = format!("Thinking... (step {iteration})");
            state.companion.on_thinking();
        }
        AgentEvent::ToolApprovalRequest { name, params } => {
            let desc = format!("{}: {}", name, params);
            state.pending_approval = Some(PendingApproval {
                tool_name: name,
                description: desc,
            });
            state.companion.on_approval();
        }
        AgentEvent::TextDelta(text) => {
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
            state.companion.on_tool_start();
        }
        AgentEvent::ToolResult {
            name,
            success,
            summary,
        } => {
            state.finish_tool_card(&name, summary, success);
            state.pending_approval = None;
        }
        AgentEvent::TokenUsage {
            input_tokens,
            output_tokens,
        } => {
            state.total_tokens_in += input_tokens;
            state.total_tokens_out += output_tokens;
            state.estimated_cost += estimate_cost(
                &state.provider_name,
                &state.model_name,
                input_tokens,
                output_tokens,
            );
        }
        AgentEvent::Complete { iterations } => {
            state.is_processing = false;
            state.status_text = format!("Done ({iterations} steps)");
            state.scroll_to_bottom();
            state.companion.on_complete();
            if state.messages.len().is_multiple_of(10) {
                state.save_conversation();
            }
        }
        AgentEvent::Error(e) => {
            state.is_processing = false;
            state.add_message(MessageRole::System, format!("Error: {e}"));
            state.status_text = "Error".into();
            state.companion.on_error();
        }
        AgentEvent::BrowserFetchStart { .. }
        | AgentEvent::BrowserFetchComplete { .. }
        | AgentEvent::BrowserFetchError { .. } => {}
    }
}

fn handle_key(
    state: &mut AppState,
    key: KeyEvent,
    user_input_tx: &mpsc::UnboundedSender<WorkerCommand>,
) {
    // Handle approval prompt first
    if state.pending_approval.is_some() {
        handle_approval_key(state, key);
        return;
    }

    // Handle session picker UI
    if state.show_session_picker {
        match key.code {
            KeyCode::Esc => state.show_session_picker = false,
            KeyCode::Up => {
                state.session_picker_index = state.session_picker_index.saturating_sub(1);
            }
            KeyCode::Down => {
                if state.session_picker_index + 1 < state.session_picker_list.len() {
                    state.session_picker_index += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(meta) = state
                    .session_picker_list
                    .get(state.session_picker_index)
                    .cloned()
                {
                    state.show_session_picker = false;
                    handle_command_result(
                        state,
                        CommandResult::LoadConversation(meta.id),
                        user_input_tx,
                    );
                } else {
                    state.show_session_picker = false;
                }
            }
            _ => {}
        }
        return;
    }

    match (key.modifiers, key.code) {
        // Quit
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            if state.is_processing {
                state.cancel_token.store(true, Ordering::Relaxed);
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
        (KeyModifiers::CONTROL, KeyCode::Char('b')) => {
            state.show_files = !state.show_files;
        }

        // Show session picker
        (KeyModifiers::CONTROL, KeyCode::Char('o')) => {
            match state.conversation_store.list_recent(20) {
                Ok(convs) => {
                    if !convs.is_empty() {
                        state.session_picker_list = convs;
                        state.session_picker_index = 0;
                        state.show_session_picker = true;
                    } else {
                        state.add_message(MessageRole::System, "No saved conversations.".into());
                    }
                }
                Err(e) => {
                    state.add_message(
                        MessageRole::System,
                        format!("Failed to list conversations: {e}"),
                    );
                }
            }
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
            let _ = user_input_tx.send(WorkerCommand::ClearHistory);
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

        // Multi-line input
        (KeyModifiers::SHIFT, KeyCode::Enter) => {
            if !state.is_processing {
                state.input.insert(state.cursor_pos, '\n');
                state.cursor_pos += 1;
            }
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
            state.companion.on_user_message();
            let agent_input = apply_mode_prefix(&state.ai_mode, &input);
            let _ = user_input_tx.send(WorkerCommand::UserMessage(agent_input));
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

        // External Editor (Ctrl+E)
        (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
            if !state.is_processing {
                let temp_dir = std::env::temp_dir();
                let temp_file = temp_dir.join(format!("phazeai_input_{}.txt", std::process::id()));
                let _ = std::fs::write(&temp_file, &state.input);

                let _ = disable_raw_mode();
                let _ = execute!(std::io::stdout(), LeaveAlternateScreen);

                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".into());
                let _ = std::process::Command::new(&editor).arg(&temp_file).status();

                let _ = enable_raw_mode();
                let _ = execute!(std::io::stdout(), EnterAlternateScreen);
                let _ = execute!(
                    std::io::stdout(),
                    crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
                );

                if let Ok(content) = std::fs::read_to_string(&temp_file) {
                    state.input = content.trim_end().to_string();
                    state.cursor_pos = state.input.len();
                }
                let _ = std::fs::remove_file(temp_file);
            }
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

        (KeyModifiers::CONTROL, KeyCode::Char('t')) => {
            for item in state.messages.iter_mut().rev() {
                if let ChatItem::ToolCard { collapsed, .. } = item {
                    *collapsed = !*collapsed;
                    break;
                }
            }
        }

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
    user_input_tx: &mpsc::UnboundedSender<WorkerCommand>,
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
            let mut new_settings = Settings::load();
            new_settings.llm.model = model.clone();
            let _ = user_input_tx.send(WorkerCommand::SwapModel {
                settings: Box::new(new_settings),
            });
            state.model_name = model.clone();
            state.add_message(MessageRole::System, format!("Model switched to: {model}"));
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
            let mut new_settings = Settings::load();
            let provider_enum = match provider.to_lowercase().as_str() {
                "claude" | "anthropic" => phazeai_core::config::LlmProvider::Claude,
                "openai" | "gpt" => phazeai_core::config::LlmProvider::OpenAI,
                "ollama" | "local" => phazeai_core::config::LlmProvider::Ollama,
                "groq" => phazeai_core::config::LlmProvider::Groq,
                "together" => phazeai_core::config::LlmProvider::Together,
                "openrouter" | "or" => phazeai_core::config::LlmProvider::OpenRouter,
                "lmstudio" | "lm-studio" => phazeai_core::config::LlmProvider::LmStudio,
                "gemini" => phazeai_core::config::LlmProvider::Gemini,
                _ => {
                    state.add_message(MessageRole::System, format!("Unknown provider: {provider}"));
                    return;
                }
            };
            new_settings.llm.provider = provider_enum;
            let _ = user_input_tx.send(WorkerCommand::SwapModel {
                settings: Box::new(new_settings),
            });
            state.provider_name = provider.clone();
            state.add_message(
                MessageRole::System,
                format!("Provider switched to: {provider}"),
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
            let Some(first_user_idx) = state.messages.iter().position(
                |item| matches!(item, ChatItem::Message(m) if m.role == MessageRole::User),
            ) else {
                state.add_message(
                    MessageRole::System,
                    "No user messages found to compact.".into(),
                );
                return;
            };

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

            let _ = user_input_tx.send(WorkerCommand::ClearHistory);

            let mut replay = String::new();
            for item in &state.messages {
                if let ChatItem::Message(m) = item {
                    match m.role {
                        MessageRole::User => {
                            replay.push_str(&format!("[Previous user message]: {}\n", m.content));
                        }
                        MessageRole::Assistant => {
                            replay.push_str(&format!(
                                "[Previous assistant response]: {}\n",
                                m.content.chars().take(300).collect::<String>()
                            ));
                        }
                        _ => {}
                    }
                }
            }
            if !replay.is_empty() && !state.is_processing {
                let _ = user_input_tx.send(WorkerCommand::UserMessage(format!(
                    "Here is a summary of our conversation so far for context. Do not respond, just acknowledge with 'OK'.\n\n{replay}"
                )));
                state.is_processing = true;
                state.status_text = "Replaying compact context...".into();
            }

            state.add_message(
                MessageRole::System,
                format!(
                    "Conversation compacted: {} messages reduced to {}.",
                    msg_count,
                    state.messages.len()
                ),
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
            state.total_tokens_in = 0;
            state.total_tokens_out = 0;
            state.estimated_cost = 0.0;
            state.iterations = 0;
            let _ = user_input_tx.send(WorkerCommand::ClearHistory);
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

                    let label = match status_code {
                        "M " | " M" | "MM" => "M",
                        "A " | " A" | "AM" => "A",
                        "D " | " D" => "D",
                        "R " | " R" => "R",
                        "??" => "?",
                        "UU" | "AA" | "DD" => "U",
                        _ => "M",
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
            let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let mut results = Vec::new();
            if let Ok(glob_matcher) = globset::GlobBuilder::new(&pattern)
                .literal_separator(false)
                .build()
                .map(|g| g.compile_matcher())
            {
                let walker = ignore::WalkBuilder::new(&root)
                    .hidden(true)
                    .git_ignore(true)
                    .build();
                for entry in walker.flatten() {
                    if results.len() >= 50 {
                        break;
                    }
                    if let Ok(rel) = entry.path().strip_prefix(&root) {
                        if glob_matcher.is_match(rel) {
                            results.push(rel.display().to_string());
                        }
                    }
                }
            }
            if results.is_empty() {
                state.add_message(
                    MessageRole::System,
                    format!("No files matching '{pattern}'"),
                );
            } else {
                let count = results.len();
                state.add_message(
                    MessageRole::System,
                    format!(
                        "Files matching '{}' ({} results):\n{}",
                        pattern,
                        count,
                        results.join("\n")
                    ),
                );
            }
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

            // Check LM Studio
            let lm_port = phazeai_core::constants::endpoints::LMSTUDIO_PORT;
            let lm_addr = format!("127.0.0.1:{}", lm_port);
            let lm_check = lm_addr
                .parse::<std::net::SocketAddr>()
                .ok()
                .and_then(|addr| {
                    std::net::TcpStream::connect_timeout(
                        &addr,
                        std::time::Duration::from_millis(500),
                    )
                    .ok()
                });
            if lm_check.is_some() {
                msg.push_str(&format!("  LM Studio: running on port {}\n", lm_port));
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
                        let _ = user_input_tx.send(WorkerCommand::UserMessage(format!(
                            "I'm providing the contents of `{name}` for context. You don't need to respond yet, just acknowledge briefly.\n\n{context_msg}"
                        )));
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
                let _ = user_input_tx.send(WorkerCommand::UserMessage(agent_input));
            }
        }
        CommandResult::Cancel => {
            state.cancel_token.store(true, Ordering::Relaxed);
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
        CommandResult::RunSkill { name, args } => {
            let mut found_path = None;
            let pwd = std::env::current_dir().unwrap_or_default();

            let mut cands = vec![pwd
                .join(".phazeai")
                .join("commands")
                .join(format!("{name}.md"))];
            if let Some(h) = dirs::home_dir() {
                cands.push(
                    h.join(".config")
                        .join("phazeai")
                        .join("commands")
                        .join(format!("{name}.md")),
                );
                cands.push(
                    h.join(".phazeai")
                        .join("commands")
                        .join(format!("{name}.md")),
                );
            }

            for c in cands {
                if c.exists() {
                    found_path = Some(c);
                    break;
                }
            }

            match found_path {
                Some(path) => {
                    if let Ok(mut text) = std::fs::read_to_string(&path) {
                        text = text.replace("$ARGS", &args);
                        state.add_message(MessageRole::User, format!("Running skill: {name}"));
                        state.is_processing = true;
                        state.status_text = format!("Running skill {name}...");
                        state.last_user_input = text.clone();
                        let agent_input = apply_mode_prefix(&state.ai_mode, &text);
                        let _ = user_input_tx.send(WorkerCommand::UserMessage(agent_input));
                    } else {
                        state.add_message(
                            MessageRole::System,
                            format!("Could not read skill file: {}", path.display()),
                        );
                    }
                }
                None => {
                    state.add_message(
                        MessageRole::System,
                        format!("Skill '{name}' not found. Searched in .phazeai/commands/ & ~/.config/phazeai/commands/")
                    );
                }
            }
        }
        CommandResult::InstallGithubApp => {
            let workflow_dir = std::path::Path::new(".github/workflows");
            if !workflow_dir.exists() {
                if let Err(e) = std::fs::create_dir_all(workflow_dir) {
                    state.add_message(
                        MessageRole::System,
                        format!("Failed to create .github/workflows directory: {e}"),
                    );
                    return;
                }
            }
            let workflow_path = workflow_dir.join("phazeai-action.yml");
            let workflow_content = r#"name: PhazeAI Code Action

on:
  issue_comment:
    types: [created]
  pull_request_review_comment:
    types: [created]

jobs:
  phazeai:
    if: contains(github.event.comment.body, '@phazeai') || contains(github.event.comment.body, '/phazeai')
    runs-on: ubuntu-latest
    permissions:
      issues: write
      pull-requests: write
      contents: write
    steps:
      - uses: actions/checkout@v4
      - name: Run PhazeAI Action
        uses: phazeai/phazeai-code-action@v1
        with:
          github_token: ${{ secrets.GITHUB_TOKEN }}
          provider: "anthropic"  # Options: anthropic, openai, ollama
          model: "claude-sonnet-4-20250514" # Options: any supported model by provider
"#;
            match std::fs::write(&workflow_path, workflow_content) {
                Ok(_) => {
                    state.add_message(
                        MessageRole::System,
                        format!("Successfully created PhazeAI GitHub Action workflow at `{}`\nCommit and push this file to enable PhazeAI automation in your repository.", workflow_path.display())
                    );
                }
                Err(e) => {
                    state.add_message(
                        MessageRole::System,
                        format!("Failed to write workflow file: {e}"),
                    );
                }
            }
        }
        CommandResult::Undo => {
            let output = std::process::Command::new("git")
                .args(["diff", "--stat", "HEAD"])
                .output();
            match output {
                Ok(o) => {
                    let diff_stat = String::from_utf8_lossy(&o.stdout);
                    if diff_stat.trim().is_empty() {
                        state.add_message(
                            MessageRole::System,
                            "Nothing to undo — no uncommitted changes.".into(),
                        );
                    } else {
                        let result = std::process::Command::new("git")
                            .args(["checkout", "--", "."])
                            .output();
                        match result {
                            Ok(_) => {
                                state.add_message(
                                    MessageRole::System,
                                    format!("Reverted all uncommitted changes:\n{diff_stat}"),
                                );
                            }
                            Err(e) => {
                                state.add_message(
                                    MessageRole::System,
                                    format!("Failed to undo: {e}"),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    state.add_message(MessageRole::System, format!("Git not available: {e}"));
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
    let chars: Vec<char> = s.chars().collect();
    let char_pos = s[..pos].chars().count();
    if char_pos == 0 {
        return 0;
    }
    let mut i = char_pos - 1;
    while i > 0 && chars[i].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    chars[..i].iter().collect::<String>().len()
}

fn word_boundary_right(s: &str, pos: usize) -> usize {
    let chars: Vec<char> = s.chars().collect();
    let char_pos = s[..pos].chars().count();
    let len = chars.len();
    if char_pos >= len {
        return s.len();
    }
    let mut i = char_pos;
    while i < len && !chars[i].is_whitespace() {
        i += 1;
    }
    while i < len && chars[i].is_whitespace() {
        i += 1;
    }
    chars[..i].iter().collect::<String>().len()
}

fn estimate_cost(provider: &str, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let p = provider.to_lowercase();
    let m = model.to_lowercase();
    let (input_rate, output_rate) = if p.contains("claude") || p.contains("anthropic") {
        if m.contains("opus") {
            (15.0, 75.0)
        } else if m.contains("haiku") {
            (0.25, 1.25)
        } else {
            (3.0, 15.0) // sonnet default
        }
    } else if p.contains("openai") {
        if m.contains("gpt-4o-mini") {
            (0.15, 0.60)
        } else if m.contains("gpt-4o") || m.contains("gpt-4") {
            (2.50, 10.0)
        } else if m.contains("o1") {
            (15.0, 60.0)
        } else {
            (2.50, 10.0)
        }
    } else if p.contains("groq") {
        (0.05, 0.08)
    } else {
        (0.0, 0.0) // local models
    };
    (input_tokens as f64 * input_rate + output_tokens as f64 * output_rate) / 1_000_000.0
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
        "/install-github-action",
        "/install-github-app",
        "/setup-github-action",
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
    chrono::Local::now().format("%H:%M").to_string()
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
    } else {
        config_path.filter(|p| p.exists())?
    };

    let mut manager = phazeai_sidecar::SidecarManager::new(python, &script_path);
    if manager.start().await.is_err() {
        return None;
    }

    let process = manager.take_process()?;
    phazeai_sidecar::SidecarClient::from_process(process).ok()
}
