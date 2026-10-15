//! Interactive TUI and single-prompt mode.

mod actions;
mod chat;
mod keys;
mod view;
mod worker;

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use phazeai_core::{
    collect_git_info,
    context::{ConversationMetadata, ConversationStore, SavedConversation, SavedMessage},
    tools::{ToolApprovalManager, ToolApprovalMode},
    Agent, AgentEvent, Settings, SystemPromptBuilder,
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::companion::Companion;
use crate::input::{History, InputBuffer};
use crate::setup;
use crate::theme::{ColorMode, Theme};
use crate::tool_view::{self, ApprovalPolicy};

use chat::{ChatItem, ChatMessage, MessageRole, ToolCard, ToolStatus};
use worker::{ApprovalRequest, UiEvent, WorkerCommand, WorkerConfig};

// ── Single-prompt mode ──────────────────────────────────────────────────

/// Run one prompt non-interactively. Assistant text goes to stdout (so it can
/// be piped); tool activity and errors go to stderr.
pub async fn run_single_prompt(
    settings: &Settings,
    prompt: &str,
    extra_instructions: Option<&str>,
    color: ColorMode,
    allow: crate::oneshot::Allow,
    can_ask: bool,
) -> Result<()> {
    let llm = settings.build_llm_client().map_err(|e| {
        anyhow::anyhow!(setup::explain_build_error(
            settings,
            &e.to_string(),
            setup::Surface::OneShot
        ))
    })?;
    let system_prompt = build_system_prompt(extra_instructions);

    let refusals = crate::oneshot::Refusals::default();
    let mut agent = Agent::new(llm)
        .with_system_prompt(system_prompt)
        .with_approval(crate::oneshot::approval_fn(
            allow,
            can_ask,
            refusals.clone(),
        ));
    if let Some(client) = try_start_sidecar().await {
        let client = Arc::new(client);
        agent.register_tool(Box::new(phazeai_sidecar::CodeSearchTool::new(
            client.clone(),
        )));
        agent.register_tool(Box::new(phazeai_sidecar::BuildIndexTool::new(client)));
    }

    let styled = color.has_color() && io::stderr().is_terminal();
    let paint = |code: &str, s: &str| {
        if styled {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    };

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let agent_handle = tokio::spawn({
        let prompt = prompt.to_string();
        async move { agent.run_with_events(prompt, event_tx).await }
    });

    let mut last_error: Option<String> = None;
    let mut stdout = io::stdout();
    while let Some(event) = event_rx.recv().await {
        match event {
            AgentEvent::TextDelta(text) => {
                print!("{text}");
                let _ = stdout.flush();
            }
            AgentEvent::ToolStart { name } => {
                eprintln!("\n{} {}", paint("36", "●"), paint("1", &name));
            }
            AgentEvent::ToolResult {
                name,
                success,
                summary,
            } => {
                let d = tool_view::digest(&name, success, &summary);
                let line = if d.headline.is_empty() {
                    d.body.first().cloned().unwrap_or_default()
                } else {
                    d.headline
                };
                let line = crate::text::truncate_to_width(&line, 100);
                if success {
                    eprintln!("  {} {}", paint("2", "⎿"), paint("2", &line));
                } else {
                    eprintln!("  {} {}", paint("31", "✗"), paint("31", &line));
                }
            }
            AgentEvent::Complete { .. } => println!(),
            AgentEvent::Error(e) => last_error = Some(e),
            AgentEvent::McpReconnected { servers } => {
                eprintln!(
                    "{}",
                    paint("2", &format!("MCP reconnected: {}", servers.join(", ")))
                );
            }
            _ => {}
        }
    }

    let result = agent_handle.await?;
    let refused = refusals.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(note) = crate::oneshot::refusal_summary(&refused) {
        eprintln!("\n{}", paint("33", &note));
    }
    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            let raw = last_error.unwrap_or_else(|| e.to_string());
            let msg = match setup::explain_runtime_error(settings, &raw, setup::Surface::OneShot) {
                Some(hint) => format!("{hint}\n\n({raw})"),
                None => raw,
            };
            Err(anyhow::anyhow!(msg))
        }
    }
}

// ── Interactive TUI state ───────────────────────────────────────────────

/// A tool call the user is being asked about.
struct PendingApproval {
    request: ApprovalRequest,
    scroll: usize,
}

/// Saved-session picker overlay.
struct SessionPicker {
    list: Vec<ConversationMetadata>,
    index: usize,
}

pub(crate) struct AppState {
    settings: Settings,
    /// Why the LLM client could not be built, if it couldn't.
    llm_error: Option<String>,

    theme: Theme,
    theme_gen: u64,
    color_mode: ColorMode,

    // Input
    input: InputBuffer,
    history: History,
    palette_index: usize,
    palette_dismissed: bool,
    input_scroll: usize,

    // Conversation view
    items: Vec<ChatItem>,
    scroll: usize,
    follow: bool,
    view_height: usize,
    expand_tools: bool,

    // Run state
    processing: bool,
    run_started: Option<Instant>,
    cancel_requested: bool,
    status_text: String,
    pending_approval: Option<PendingApproval>,
    queued: VecDeque<String>,
    tokens_in: u64,
    tokens_out: u64,
    cost: f64,
    context_tokens: Option<usize>,

    // Environment
    git_branch: Option<String>,
    git_dirty: usize,

    // Misc UI
    should_quit: bool,
    quit_armed_at: Option<Instant>,
    notice: Option<(String, Instant)>,
    show_files: bool,
    session_picker: Option<SessionPicker>,
    needs_clear: bool,
    clipboard: Option<arboard::Clipboard>,

    // Persistence
    conversation_id: String,
    conversation_store: ConversationStore,

    // Agent plumbing
    approval_manager: Arc<Mutex<ToolApprovalManager>>,
    policy: Arc<Mutex<ApprovalPolicy>>,
    cancel: Arc<AtomicBool>,
    worker_tx: mpsc::UnboundedSender<WorkerCommand>,

    ai_mode: String,
    last_user_input: String,
    companion: Companion,
}

impl AppState {
    fn push(&mut self, item: ChatItem) {
        self.items.push(item);
    }

    fn system(&mut self, text: impl Into<String>) {
        self.push(ChatItem::message(MessageRole::System, text.into()));
        self.follow = true;
    }

    fn error(&mut self, text: impl Into<String>) {
        self.push(ChatItem::message(MessageRole::Error, text.into()));
        self.follow = true;
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    fn refresh_git(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let (branch, dirty) = collect_git_info(&cwd);
        self.git_branch = branch;
        self.git_dirty = dirty.len();
    }

    fn approval_mode(&self) -> ToolApprovalMode {
        self.approval_manager
            .lock()
            .map(|m| m.mode().clone())
            .unwrap_or_default()
    }

    /// Send a prompt to the agent (or queue it while a run is active).
    fn send_prompt(&mut self, display: String, agent_input: String) {
        if let Some(err) = self.llm_error.clone() {
            self.push(ChatItem::message(MessageRole::User, display));
            self.error(setup::explain_build_error(
                &self.settings,
                &err,
                setup::Surface::Tui,
            ));
            return;
        }
        if self.processing {
            self.queued.push_back(display);
            self.notify("Queued — it will be sent when the current response finishes");
            return;
        }
        self.push(ChatItem::message(MessageRole::User, display));
        self.start_run(agent_input);
    }

    fn start_run(&mut self, agent_input: String) {
        self.processing = true;
        self.cancel_requested = false;
        self.run_started = Some(Instant::now());
        self.status_text = "Thinking".into();
        self.follow = true;
        self.companion.on_user_message();
        let _ = self.worker_tx.send(WorkerCommand::Run(agent_input));
    }

    /// Interrupt the running request.
    fn cancel_run(&mut self) {
        if !self.processing || self.cancel_requested {
            return;
        }
        self.cancel_requested = true;
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(p) = self.pending_approval.take() {
            let _ = p.request.respond.send(false);
        }
        // Hand queued follow-ups back to the editor instead of dropping them.
        if !self.queued.is_empty() {
            let mut restored: Vec<String> = self.queued.drain(..).collect();
            if !self.input.is_empty() {
                restored.push(self.input.text().to_string());
            }
            self.input.set(restored.join("\n"));
            self.notify("Interrupted — your queued message is back in the input");
        }
        self.status_text = "Interrupting".into();
    }

    /// Clear the view and reset per-conversation counters.
    fn reset_conversation_view(&mut self) {
        self.items.clear();
        self.scroll = 0;
        self.follow = true;
        self.context_tokens = Some(0);
    }

    fn save_conversation(&self) {
        let messages: Vec<SavedMessage> = self
            .items
            .iter()
            .map(|item| match item {
                ChatItem::Message(m) => SavedMessage {
                    role: m.role.as_saved().into(),
                    content: m.content.clone(),
                    timestamp: m.timestamp.clone(),
                    tool_name: None,
                },
                ChatItem::Tool(card) => {
                    let status = match card.status {
                        ToolStatus::Ok => "ok",
                        ToolStatus::Failed => "err",
                        ToolStatus::Running | ToolStatus::Pending => "interrupted",
                    };
                    let headline = card
                        .digest
                        .as_ref()
                        .map(|d| d.headline.clone())
                        .unwrap_or_default();
                    SavedMessage {
                        role: "tool".into(),
                        content: format!("[{} {}: {status}] {headline}", card.name, card.label)
                            .trim_end()
                            .to_string(),
                        timestamp: chat::now_str(),
                        tool_name: Some(card.name.clone()),
                    }
                }
            })
            .collect();

        let title = self
            .items
            .iter()
            .find_map(|item| match item {
                ChatItem::Message(m) if m.role == MessageRole::User => {
                    let first = m.content.lines().next().unwrap_or("");
                    Some(crate::text::truncate_to_width(first, 80))
                }
                _ => None,
            })
            .unwrap_or_else(|| "Untitled".into());

        let metadata = ConversationMetadata {
            id: self.conversation_id.clone(),
            title,
            created_at: chat::now_str(),
            updated_at: chat::now_str(),
            message_count: messages.len(),
            model: self.settings.llm.model.clone(),
            project_dir: std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string()),
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

    fn has_user_content(&self) -> bool {
        self.items
            .iter()
            .any(|i| i.role() == Some(MessageRole::User))
    }

    /// Replace the view with a saved conversation and return the (role,
    /// content) pairs to restore into the agent.
    fn load_saved(&mut self, conv: SavedConversation) -> Vec<(String, String)> {
        self.reset_conversation_view();
        self.conversation_id = conv.metadata.id.clone();
        let mut restore = Vec::new();
        for msg in conv.messages {
            if msg.role == "user" || msg.role == "assistant" {
                restore.push((msg.role.clone(), msg.content.clone()));
            }
            self.items.push(ChatItem::Message(ChatMessage::new(
                MessageRole::from_saved(&msg.role),
                msg.content,
                msg.timestamp,
            )));
        }
        self.system(format!(
            "Resumed \"{}\" ({} messages)",
            conv.metadata.title, conv.metadata.message_count
        ));
        restore
    }

    /// Find a saved conversation by exact id or unique-enough prefix.
    fn find_saved(&self, id: &str) -> Option<SavedConversation> {
        if let Ok(conv) = self.conversation_store.load(id) {
            return Some(conv);
        }
        let recent = self.conversation_store.list_recent(500).ok()?;
        let meta = recent.iter().find(|m| m.id.starts_with(id))?;
        self.conversation_store.load(&meta.id).ok()
    }
}

// ── Agent event handling ────────────────────────────────────────────────

fn last_open_card<'a>(items: &'a mut [ChatItem], name: &str) -> Option<&'a mut ToolCard> {
    items.iter_mut().rev().find_map(|item| match item {
        ChatItem::Tool(card) if card.name == name && !card.is_finished() => Some(card),
        _ => None,
    })
}

fn handle_ui_event(state: &mut AppState, event: UiEvent) {
    match event {
        UiEvent::Agent(ev) => handle_agent_event(state, ev),
        UiEvent::Approval(request) => {
            if state.cancel_requested {
                let _ = request.respond.send(false);
                return;
            }
            if let Some(card) = last_open_card(&mut state.items, &request.name) {
                card.status = ToolStatus::Pending;
            }
            state.pending_approval = Some(PendingApproval { request, scroll: 0 });
            state.companion.on_approval();
            state.follow = true;
        }
        UiEvent::Context(tokens) => state.context_tokens = Some(tokens),
        UiEvent::RunFinished { cancelled } => {
            state.processing = false;
            state.pending_approval = None;
            let elapsed = state.run_started.take().map(|s| s.elapsed());
            for item in &mut state.items {
                if let ChatItem::Tool(card) = item {
                    if !card.is_finished() {
                        card.status = ToolStatus::Failed;
                        card.digest = Some(tool_view::Digest {
                            headline: "interrupted".into(),
                            body: Vec::new(),
                        });
                    }
                }
            }
            if cancelled {
                state.status_text = "Interrupted".into();
                state.system(
                    "Interrupted. The conversation is intact — send a new message to continue.",
                );
            } else if let Some(e) = elapsed {
                state.status_text = format!("Done in {:.1}s", e.as_secs_f64());
            }
            state.cancel_requested = false;
            state.refresh_git();
            if state.has_user_content() {
                state.save_conversation();
            }
            if let Some(next) = state.queued.pop_front() {
                let agent_input = apply_mode_prefix(&state.ai_mode, &next);
                state.last_user_input = next.clone();
                state.push(ChatItem::message(MessageRole::User, next));
                state.start_run(agent_input);
            }
        }
    }
}

fn handle_agent_event(state: &mut AppState, event: AgentEvent) {
    match event {
        AgentEvent::Thinking { iteration } => {
            state.status_text = if iteration > 1 {
                format!("Thinking · step {iteration}")
            } else {
                "Thinking".into()
            };
            state.companion.on_thinking();
        }
        AgentEvent::ToolApprovalRequest { name, params } => {
            // Emitted for every tool call before approval is decided; use it
            // to open the card with a readable argument summary.
            let label = tool_view::call_label(&name, &params);
            state.push(ChatItem::Tool(ToolCard::new(
                name,
                label,
                ToolStatus::Running,
            )));
        }
        AgentEvent::TextDelta(text) => {
            if let Some(ChatItem::Message(m)) = state.items.last_mut() {
                if m.role == MessageRole::Assistant {
                    m.content.push_str(&text);
                    return;
                }
            }
            state.status_text = "Responding".into();
            state.push(ChatItem::message(MessageRole::Assistant, text));
        }
        AgentEvent::ToolStart { name } => {
            state.status_text = format!("Running {name}");
            match last_open_card(&mut state.items, &name) {
                Some(card) => {
                    card.status = ToolStatus::Running;
                    card.started = Some(Instant::now());
                }
                None => {
                    let mut card = ToolCard::new(name, String::new(), ToolStatus::Running);
                    card.started = Some(Instant::now());
                    state.push(ChatItem::Tool(card));
                }
            }
            state.companion.on_tool_start();
        }
        AgentEvent::ToolResult {
            name,
            success,
            summary,
        } => {
            let digest = tool_view::digest(&name, success, &summary);
            if let Some(card) = last_open_card(&mut state.items, &name) {
                card.status = if success {
                    ToolStatus::Ok
                } else {
                    ToolStatus::Failed
                };
                card.elapsed = card.started.map(|s| s.elapsed());
                card.digest = Some(digest);
            }
            if state
                .pending_approval
                .as_ref()
                .is_some_and(|p| p.request.name == name)
            {
                state.pending_approval = None;
            }
        }
        AgentEvent::TokenUsage {
            input_tokens,
            output_tokens,
        } => {
            state.tokens_in += input_tokens;
            state.tokens_out += output_tokens;
            state.cost += setup::estimate_cost(
                &state.settings.llm.provider,
                &state.settings.llm.model,
                input_tokens,
                output_tokens,
            );
        }
        AgentEvent::Complete { .. } => {
            state.companion.on_complete();
        }
        AgentEvent::Error(e) => {
            if e == "Cancelled" {
                return;
            }
            let msg = match setup::explain_runtime_error(&state.settings, &e, setup::Surface::Tui) {
                Some(hint) => format!("{hint}\n({e})"),
                None => e,
            };
            state.error(msg);
            state.status_text = "Error".into();
            state.companion.on_error();
        }
        AgentEvent::FilePatch { path, .. } => {
            state.status_text = format!("Writing {path}");
        }
        AgentEvent::BrowserFetchStart { url } => {
            state.status_text = format!("Fetching {url}");
        }
        AgentEvent::BrowserFetchComplete { .. } | AgentEvent::BrowserFetchError { .. } => {}
        AgentEvent::McpReconnected { servers } => {
            state.notify(format!("MCP reconnected: {}", servers.join(", ")));
        }
    }
}

// ── TUI entry point ─────────────────────────────────────────────────────

pub struct TuiOptions<'a> {
    pub theme: &'a str,
    pub color: ColorMode,
    pub continue_last: bool,
    pub resume_id: Option<String>,
    pub extra_instructions: Option<&'a str>,
}

pub async fn run_tui(settings: Settings, opts: TuiOptions<'_>) -> Result<()> {
    let (llm, llm_error) = match settings.build_llm_client() {
        Ok(llm) => (Some(llm), None),
        Err(e) => (None, Some(e.to_string())),
    };

    let (worker_tx, worker_rx) = mpsc::unbounded_channel::<WorkerCommand>();
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiEvent>();

    let approval_manager = Arc::new(Mutex::new(ToolApprovalManager::default()));
    let policy = Arc::new(Mutex::new(ApprovalPolicy::default()));
    let cancel = Arc::new(AtomicBool::new(false));

    let mut state = AppState {
        settings,
        llm_error,
        theme: Theme::by_name(opts.theme).with_mode(opts.color),
        theme_gen: 0,
        color_mode: opts.color,
        input: InputBuffer::default(),
        history: History::load(History::default_path()),
        palette_index: 0,
        palette_dismissed: false,
        input_scroll: 0,
        items: Vec::new(),
        scroll: 0,
        follow: true,
        view_height: 0,
        expand_tools: false,
        processing: false,
        run_started: None,
        cancel_requested: false,
        status_text: String::new(),
        pending_approval: None,
        queued: VecDeque::new(),
        tokens_in: 0,
        tokens_out: 0,
        cost: 0.0,
        context_tokens: None,
        git_branch: None,
        git_dirty: 0,
        should_quit: false,
        quit_armed_at: None,
        notice: None,
        show_files: false,
        session_picker: None,
        needs_clear: false,
        clipboard: None,
        conversation_id: ConversationStore::generate_id(),
        conversation_store: ConversationStore::new()
            .unwrap_or_else(|_| ConversationStore::default()),
        approval_manager: approval_manager.clone(),
        policy: policy.clone(),
        cancel: cancel.clone(),
        worker_tx,
        ai_mode: "chat".into(),
        last_user_input: String::new(),
        companion: Companion::new(),
    };
    state.refresh_git();

    // Restore a previous conversation if requested.
    let mut restore = Vec::new();
    if opts.continue_last {
        match state.conversation_store.list_recent(1) {
            Ok(recent) => match recent.first() {
                Some(meta) => match state.conversation_store.load(&meta.id) {
                    Ok(conv) => restore = state.load_saved(conv),
                    Err(e) => state.error(format!("Couldn't load the last conversation: {e}")),
                },
                None => state.system("No saved conversation to continue."),
            },
            Err(e) => state.error(format!("Couldn't list conversations: {e}")),
        }
    } else if let Some(id) = opts.resume_id.as_deref() {
        match state.find_saved(id) {
            Some(conv) => restore = state.load_saved(conv),
            None => state.error(format!("No saved conversation matching '{id}'.")),
        }
    }

    if let Some(err) = state.llm_error.clone() {
        let msg = setup::explain_build_error(&state.settings, &err, setup::Surface::Tui);
        state.error(msg);
    }

    let system_prompt = build_system_prompt(opts.extra_instructions);
    let _worker = worker::spawn(
        WorkerConfig {
            llm,
            system_prompt,
            restore,
            approval_manager,
            policy,
            cancel,
        },
        worker_rx,
        ui_tx,
    );

    let _guard = crate::term::TerminalGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    loop {
        while let Ok(ev) = ui_rx.try_recv() {
            handle_ui_event(&mut state, ev);
        }

        if state.needs_clear {
            terminal.clear()?;
            state.needs_clear = false;
        }
        terminal.draw(|f| view::draw(f, &mut state))?;

        // Wait briefly for input, then drain everything that's queued so a
        // large paste or key-repeat burst is handled before the next frame.
        let timeout = if state.processing {
            Duration::from_millis(40)
        } else {
            Duration::from_millis(100)
        };
        if event::poll(timeout)? {
            loop {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        keys::handle_key(&mut state, key);
                    }
                    Event::Paste(text) => keys::handle_paste(&mut state, &text),
                    _ => {}
                }
                if state.should_quit || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }

        if state.should_quit {
            if state.processing {
                state.cancel.store(true, Ordering::SeqCst);
            }
            if state.has_user_content() {
                state.save_conversation();
            }
            break;
        }
    }

    drop(terminal);
    drop(_guard);
    let id = &state.conversation_id;
    if state.has_user_content() {
        println!(
            "Conversation saved. Resume with: phazeai --resume {}",
            &id[..8.min(id.len())]
        );
    }
    Ok(())
}

/// Prepend mode-specific instructions to the user's message so the LLM
/// knows which role it should take for this turn.
pub(crate) fn apply_mode_prefix(mode: &str, input: &str) -> String {
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
    format!("{prefix}{input}")
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

/// Attempt to start the Python sidecar for semantic search.
/// Returns None if Python is unavailable or the sidecar script doesn't exist.
pub(crate) async fn try_start_sidecar() -> Option<phazeai_sidecar::SidecarClient> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_prefix() {
        assert_eq!(apply_mode_prefix("chat", "hi"), "hi");
        assert!(apply_mode_prefix("plan", "hi").starts_with("[PLANNING MODE]"));
        assert!(apply_mode_prefix("ask", "hi").ends_with("hi"));
    }
}
