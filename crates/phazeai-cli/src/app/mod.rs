//! Interactive TUI and single-prompt mode.

mod actions;
mod chat;
mod keys;
mod view;
mod worker;

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use phazeai_core::{
    collect_git_info,
    context::{ConversationMetadata, ConversationStore, SavedConversation, SavedMessage},
    tools::{ToolApprovalManager, ToolApprovalMode},
    Agent, AgentEvent, BuildIndexTool, CodeSearch, CodeSearchTool, LlmClient, Settings,
    SystemPromptBuilder,
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let refusals = crate::oneshot::Refusals::default();
    // Ctrl+C stops the run instead of killing this process outright: the
    // agent drops the running tool, which kills any shell command it started.
    // Those run in their own process group, so the terminal's SIGINT no
    // longer reaches them and dying here would leave them running.
    let cancel = Arc::new(AtomicBool::new(false));
    let agent = single_prompt_agent(settings, llm, &cwd, extra_instructions)
        .with_cancel_token(cancel.clone())
        .with_approval(crate::oneshot::approval_fn(
            allow,
            can_ask,
            refusals.clone(),
        ));

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
    let interrupt = tokio::spawn({
        let cancel = cancel.clone();
        async move {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            cancel.store(true, Ordering::SeqCst);
            eprintln!("\nInterrupted, stopping.");
            // A second Ctrl+C exits at once.
            if tokio::signal::ctrl_c().await.is_ok() {
                std::process::exit(130);
            }
        }
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
    interrupt.abort();
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
    /// Input lines, as typed (`/add path`, `/skill name`, or a message),
    /// waiting for the current run to finish; see [`Self::drain_queue`].
    queued: VecDeque<String>,
    tokens_in: u64,
    tokens_out: u64,
    cost: f64,
    context_tokens: Option<usize>,
    /// Conversation tokens the agent may send before compacting.
    context_budget: usize,

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
    /// `display` is what the conversation shows as the user's turn and is
    /// also the input line that reproduces the prompt; use
    /// [`Self::send_prompt_as`] when the two differ.
    fn send_prompt(&mut self, display: String, agent_input: String) {
        let resubmit = display.clone();
        self.send_prompt_as(resubmit, display, agent_input);
    }

    /// Like [`Self::send_prompt`], with `resubmit` as the input line that
    /// reproduces the prompt (e.g. `/add path` for a prompt that carries the
    /// file's contents).
    ///
    /// While a run is active the prompt is queued as that line — not as the
    /// expanded `agent_input` or the one-line `display` — and re-run through
    /// `actions::submit_line` when the run ends. So a queued `/add` or
    /// `/skill` reads the file or skill, and `/add` re-checks the privacy
    /// policy, at the moment it is actually sent; and an interrupted queue
    /// goes back to the input box as lines that can be re-submitted as typed.
    fn send_prompt_as(&mut self, resubmit: String, display: String, agent_input: String) {
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
            self.queued.push_back(resubmit);
            self.notify("Queued — it will be sent when the current response finishes");
            return;
        }
        self.push(ChatItem::message(MessageRole::User, display));
        self.start_run(agent_input);
    }

    /// Re-submit the queued input lines, in order, until one starts a run or
    /// the queue is empty (a line that produces no run — a refused `/add`, an
    /// unknown skill — must not stall the ones behind it).
    fn drain_queue(&mut self) {
        while !self.processing {
            let Some(line) = self.queued.pop_front() else {
                break;
            };
            actions::submit_line(self, &line);
        }
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
            state.drain_queue();
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
        AgentEvent::ContextCompacted {
            messages_folded,
            tokens_before,
            tokens_after,
        } => {
            state.system(format!(
                "Summarised {messages_folded} earlier messages to stay within the context \
                 budget (~{tokens_before} → ~{tokens_after} tokens)."
            ));
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

    let context_budget = settings.context_budget();
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
        context_budget,
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

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _worker = worker::spawn(
        WorkerConfig {
            llm,
            prompt: system_prompt_builder(&cwd, opts.extra_instructions),
            restore,
            approval_manager,
            policy,
            cancel,
            context_budget: state.context_budget,
            code_search: code_search(&state.settings),
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

/// What the session's system prompt is made of: the project in `root`, its
/// instruction files and git state, and `extra_instructions` (`--instructions`).
/// Read once per session; `with_context_budget` then sizes the prompt for each
/// model, so a long instructions file can't fill a small model's budget.
fn system_prompt_builder(root: &Path, extra_instructions: Option<&str>) -> SystemPromptBuilder {
    let mut builder = SystemPromptBuilder::new()
        .with_project_root(root.to_path_buf())
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

    let (branch, dirty) = collect_git_info(root);
    builder = builder.with_git_info(branch, dirty);

    if let Some(extra) = extra_instructions {
        builder = builder.with_additional_instructions(extra.to_string());
    }

    builder
}

/// The agent for one `-p` prompt, before its approval and cancel flag are
/// attached: the model's context budget sizes both the system prompt and the
/// conversation, as in the TUI.
fn single_prompt_agent(
    settings: &Settings,
    llm: Box<dyn LlmClient>,
    root: &Path,
    extra_instructions: Option<&str>,
) -> Agent {
    let context_budget = settings.context_budget();
    let system_prompt = system_prompt_builder(root, extra_instructions)
        .with_context_budget(context_budget)
        .build();
    let mut agent = Agent::new(llm)
        .with_system_prompt(system_prompt)
        .with_context_budget(context_budget);
    if let Some(search) = code_search(settings) {
        register_code_search(&mut agent, &search);
    }
    agent
}

/// The code search for this session, rooted at the working directory, or
/// `None` when `[search] enabled` is off. Nothing is read until the model
/// first searches or calls `build_search_index`, so a CLI started in a large
/// directory it never searches costs nothing (`auto_index` is an IDE setting).
pub(crate) fn code_search(settings: &Settings) -> Option<CodeSearch> {
    if !settings.search.enabled {
        return None;
    }
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Some(CodeSearch::new(root))
}

/// Give the agent the `code_search` and `build_search_index` tools.
pub(crate) fn register_code_search(agent: &mut Agent, search: &CodeSearch) {
    agent.register_tool(Box::new(CodeSearchTool::new(search.clone())));
    agent.register_tool(Box::new(BuildIndexTool::new(search.clone())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use phazeai_core::context::system_prompt::instructions_allowance;
    use phazeai_core::llm::OpenAIClient;
    use serde_json::{json, Value};

    #[test]
    fn mode_prefix() {
        assert_eq!(apply_mode_prefix("chat", "hi"), "hi");
        assert!(apply_mode_prefix("plan", "hi").starts_with("[PLANNING MODE]"));
        assert!(apply_mode_prefix("ask", "hi").ends_with("hi"));
    }

    #[test]
    fn code_search_follows_the_setting() {
        let mut settings = Settings::default();
        assert!(code_search(&settings).is_some());
        settings.search.enabled = false;
        assert!(code_search(&settings).is_none());
    }

    // ── Queue handling ──────────────────────────────────────────────────────

    /// A scratch directory removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir()
                .join(format!("phazeai-cli-{tag}-{}-{nonce}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An `AppState` with no worker thread: `rx` receives what the worker
    /// would have been told to do.
    fn test_state(dir: &std::path::Path) -> (AppState, mpsc::UnboundedReceiver<WorkerCommand>) {
        let (worker_tx, rx) = mpsc::unbounded_channel::<WorkerCommand>();
        let settings = Settings::default();
        let context_budget = settings.context_budget();
        let state = AppState {
            settings,
            llm_error: None,
            theme: Theme::by_name("dark").with_mode(ColorMode::TrueColor),
            theme_gen: 0,
            color_mode: ColorMode::TrueColor,
            input: InputBuffer::default(),
            history: History::default(),
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
            context_budget,
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
            conversation_store: ConversationStore::with_dir(dir.join("conversations")).unwrap(),
            approval_manager: Arc::new(Mutex::new(ToolApprovalManager::default())),
            policy: Arc::new(Mutex::new(ApprovalPolicy::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            worker_tx,
            ai_mode: "chat".into(),
            last_user_input: String::new(),
            companion: Companion::new(),
        };
        (state, rx)
    }

    /// The prompt of the next run handed to the worker, if any.
    fn run_input(rx: &mut mpsc::UnboundedReceiver<WorkerCommand>) -> Option<String> {
        match rx.try_recv() {
            Ok(WorkerCommand::Run(input)) => Some(input),
            _ => None,
        }
    }

    fn turns(state: &AppState, role: MessageRole) -> Vec<String> {
        state
            .items
            .iter()
            .filter_map(|item| match item {
                ChatItem::Message(m) if m.role == role => Some(m.content.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn queued_prompts_are_stored_as_their_input_line() {
        let tmp = TempDir::new("queue");
        let (mut state, mut rx) = test_state(tmp.path());
        state.processing = true;
        state.send_prompt("/skill review".into(), "You are a reviewer…".into());
        state.send_prompt_as(
            "/add src/x.rs".into(),
            "Added x.rs to the context (3 lines)".into(),
            "contents of x.rs".into(),
        );
        assert_eq!(state.queued, ["/skill review", "/add src/x.rs"]);
        assert!(run_input(&mut rx).is_none());
        assert!(turns(&state, MessageRole::User).is_empty());

        // Interrupting hands the lines back to the input box, where they can
        // be re-submitted as typed — not as the expanded text or a summary.
        state.cancel_run();
        assert_eq!(state.input.text(), "/skill review\n/add src/x.rs");
        assert!(state.queued.is_empty());
    }

    #[test]
    fn a_queued_add_is_expanded_when_it_is_sent() {
        let tmp = TempDir::new("add");
        let file = tmp.path().join("notes.txt");
        std::fs::write(&file, "hello from notes\nsecond line\n").unwrap();
        let (mut state, mut rx) = test_state(tmp.path());
        let line = format!("/add {}", file.display());

        // A response is running: the command is queued as typed, nothing is sent.
        state.processing = true;
        actions::submit_line(&mut state, &line);
        assert_eq!(state.queued, [line.as_str()]);
        assert!(run_input(&mut rx).is_none());
        assert!(turns(&state, MessageRole::User).is_empty());

        // The run ends: the file is read now and its contents reach the model.
        state.processing = false;
        state.drain_queue();
        let sent = run_input(&mut rx).expect("the dequeued /add starts a run");
        assert!(sent.contains("hello from notes\nsecond line"), "{sent}");
        assert!(sent.contains(&file.display().to_string()), "{sent}");
        assert_eq!(
            turns(&state, MessageRole::User),
            ["Added notes.txt to the context (2 lines)"]
        );
        assert!(state.processing);
        assert!(state.queued.is_empty());
    }

    #[test]
    fn a_queued_add_of_a_denied_file_is_refused_when_sent_and_the_queue_moves_on() {
        let tmp = TempDir::new("add-denied");
        let env = tmp.path().join(".env"); // on the built-in deny list
        std::fs::write(&env, "SECRET=1\n").unwrap();
        let (mut state, mut rx) = test_state(tmp.path());
        state.queued.push_back(format!("/add {}", env.display()));
        state.queued.push_back("what next?".into());

        state.drain_queue();
        let sent = run_input(&mut rx).expect("the message behind the refused /add is sent");
        assert_eq!(sent, "what next?");
        assert!(run_input(&mut rx).is_none());
        let errors = turns(&state, MessageRole::Error);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("Not added:"), "{}", errors[0]);
        assert!(!errors[0].contains("SECRET"));
        assert_eq!(turns(&state, MessageRole::User), ["what next?"]);
        assert_eq!(state.last_user_input, "what next?");
    }

    #[test]
    fn a_queued_skill_is_resolved_when_sent_not_passed_through_as_text() {
        let tmp = TempDir::new("skill");
        let (mut state, mut rx) = test_state(tmp.path());
        // The skill reaches the model only through its file: an unknown one
        // produces an error at send time, never a "/skill …" prompt.
        state
            .queued
            .push_back("/skill phazeai-test-no-such-skill-7f3a".into());
        state.drain_queue();
        assert!(run_input(&mut rx).is_none());
        let errors = turns(&state, MessageRole::Error);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("not found"), "{}", errors[0]);
        assert!(turns(&state, MessageRole::User).is_empty());
        assert!(!state.processing);
    }

    #[test]
    fn a_queued_message_is_sent_with_the_mode_prefix_of_that_moment_one_run_at_a_time() {
        let tmp = TempDir::new("message");
        let (mut state, mut rx) = test_state(tmp.path());
        state.processing = true;
        actions::submit_line(&mut state, "hello");
        actions::submit_line(&mut state, "and then");
        assert_eq!(state.queued, ["hello", "and then"]);

        state.ai_mode = "plan".into();
        state.processing = false;
        state.drain_queue();
        let sent = run_input(&mut rx).unwrap();
        assert!(sent.starts_with("[PLANNING MODE]"), "{sent}");
        assert!(sent.ends_with("hello"), "{sent}");
        assert_eq!(turns(&state, MessageRole::User), ["hello"]);
        assert_eq!(state.last_user_input, "hello");
        // Only one run at a time: the rest waits for it to finish.
        assert!(run_input(&mut rx).is_none());
        assert_eq!(state.queued, ["and then"]);
    }

    // ── The system prompt fits the model's context budget ───────────────────

    /// Numbered lines of text, at least `bytes` long in all.
    fn lines_of(word: &str, bytes: usize) -> String {
        let mut text = String::new();
        let mut n = 0;
        while text.len() < bytes {
            n += 1;
            text.push_str(&format!(
                "- {word} {n:04}: run the tests before you say it works.\n"
            ));
        }
        text
    }

    /// One streamed reply in the OpenAI format: `delta`, then the end.
    fn sse(delta: Value) -> String {
        let chunk = json!({ "choices": [{ "delta": delta }] });
        format!("data: {chunk}\n\ndata: [DONE]\n\n")
    }

    /// An OpenAI-compatible provider on a local port. It answers each chat
    /// request with the next of `replies` (then "ok") and keeps the JSON body
    /// of every request, i.e. exactly what a provider is sent.
    async fn fake_provider(replies: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let seen = bodies.clone();
        let mut replies = VecDeque::from(replies);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut chunk = [0u8; 16 * 1024];
                let mut body_len = None;
                loop {
                    let n = sock.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..n]);
                    let head_end = request.windows(4).position(|w| w == b"\r\n\r\n");
                    if let Some(end) = head_end {
                        let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                        let len: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + len || n == 0 {
                            body_len = Some((end + 4, len));
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let Some((start, len)) = body_len else {
                    continue;
                };
                let body = serde_json::from_slice(&request[start..start + len]).unwrap();
                seen.lock().unwrap().push(body);
                let reply = replies
                    .pop_front()
                    .unwrap_or_else(|| sse(json!({ "content": "ok" })));
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                sock.write_all(response.as_bytes()).await.unwrap();
                let _ = sock.shutdown().await;
            }
        });
        (url, bodies)
    }

    fn fake_client(url: &str) -> Box<dyn LlmClient> {
        Box::new(OpenAIClient::new("k").with_base_url(url).without_proxy())
    }

    fn system_prompt_sent(request: &Value) -> &str {
        assert_eq!(request["messages"][0]["role"], "system");
        request["messages"][0]["content"].as_str().unwrap()
    }

    /// The instructions section of a system prompt (it comes last).
    fn instructions_section(system_prompt: &str) -> &str {
        let at = system_prompt
            .find("\n\n## Project-Specific Instructions\n")
            .expect("an instructions section");
        &system_prompt[at..]
    }

    #[tokio::test]
    async fn a_single_prompt_is_sized_to_the_models_context_budget() {
        let tmp = TempDir::new("oneshot-budget");
        let root = tmp.path();
        let instructions = lines_of("Rule", 20 * 1024);
        std::fs::write(root.join("CLAUDE.md"), &instructions).unwrap();
        std::fs::write(root.join("build.log"), lines_of("Log", 20 * 1024)).unwrap();
        let mut settings = Settings::default();
        settings.llm.provider = phazeai_core::config::LlmProvider::Ollama;
        settings.llm.model = "qwen2.5-coder:7b".into();
        settings.search.enabled = false;

        // A local model's budget: the instructions are cut to their share,
        // and the conversation is held to the same budget (the 20 KB log the
        // model reads is cut to fit, where the agent's 32k default sent it).
        let budget = settings.context_budget();
        let read_log = json!({ "tool_calls": [{
            "index": 0,
            "id": "c1",
            "function": {
                "name": "read_file",
                "arguments": json!({ "path": root.join("build.log") }).to_string(),
            },
        }]});
        let (url, bodies) =
            fake_provider(vec![sse(read_log), sse(json!({ "content": "done" }))]).await;
        let agent = single_prompt_agent(&settings, fake_client(&url), root, None);
        let reply = agent.run("Why did the build fail?").await.unwrap();
        assert_eq!(reply.content, "done");
        let sent = bodies.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        let section = instructions_section(system_prompt_sent(&sent[0]));
        assert!(
            section.len() <= instructions_allowance(budget),
            "{} bytes of instructions for a {budget}-token budget",
            section.len()
        );
        assert!(section.contains(" is shortened here to fit this model's context budget"));
        let log_result = sent[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool")
            .expect("the log was sent back")["content"]
            .as_str()
            .unwrap();
        assert!(
            log_result.contains(" bytes of this tool output were cut to fit the context budget"),
            "the output went out whole: the conversation is not held to the budget"
        );

        // A large budget ([llm] context_tokens): the whole file, as before.
        settings.llm.context_tokens = Some(60_000);
        let (url, bodies) = fake_provider(Vec::new()).await;
        let agent = single_prompt_agent(&settings, fake_client(&url), root, None);
        agent.run("Why did the build fail?").await.unwrap();
        let sent = bodies.lock().unwrap().clone();
        assert_eq!(
            instructions_section(system_prompt_sent(&sent[0])),
            format!("\n\n## Project-Specific Instructions\n{instructions}")
        );
    }

    /// Waits for the worker to report the end of a run.
    async fn run_finished(rx: &mut mpsc::UnboundedReceiver<UiEvent>) {
        let finished = async {
            while let Some(event) = rx.recv().await {
                if matches!(event, UiEvent::RunFinished { .. }) {
                    return;
                }
            }
            panic!("the worker stopped");
        };
        tokio::time::timeout(Duration::from_secs(30), finished)
            .await
            .expect("the run did not finish");
    }

    #[tokio::test]
    async fn a_model_switch_resizes_the_system_prompt_to_the_new_budget() {
        let tmp = TempDir::new("switch-budget");
        let instructions = lines_of("Rule", 20 * 1024);
        std::fs::write(tmp.path().join("CLAUDE.md"), &instructions).unwrap();
        let (url, bodies) = fake_provider(Vec::new()).await;
        let (worker_tx, worker_rx) = mpsc::unbounded_channel();
        let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
        let _worker = worker::spawn(
            WorkerConfig {
                llm: Some(fake_client(&url)),
                prompt: system_prompt_builder(tmp.path(), None),
                restore: Vec::new(),
                approval_manager: Arc::new(Mutex::new(ToolApprovalManager::default())),
                policy: Arc::new(Mutex::new(ApprovalPolicy::default())),
                cancel: Arc::new(AtomicBool::new(false)),
                context_budget: 60_000,
                code_search: None,
            },
            worker_rx,
            ui_tx,
        );

        // A model with a large budget gets the whole file.
        worker_tx.send(WorkerCommand::Run("first".into())).unwrap();
        run_finished(&mut ui_rx).await;
        // `/provider ollama` and the like: a small local model.
        worker_tx
            .send(WorkerCommand::SetLlm {
                llm: fake_client(&url),
                context_budget: 4_096,
            })
            .unwrap();
        worker_tx.send(WorkerCommand::Run("second".into())).unwrap();
        run_finished(&mut ui_rx).await;

        let sent = bodies.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            instructions_section(system_prompt_sent(&sent[0])),
            format!("\n\n## Project-Specific Instructions\n{instructions}")
        );
        let section = instructions_section(system_prompt_sent(&sent[1]));
        assert!(
            section.len() <= instructions_allowance(4_096),
            "after the switch the instructions are still {} bytes",
            section.len()
        );
        assert!(section.contains(" is shortened here to fit this model's context budget"));
    }
}
