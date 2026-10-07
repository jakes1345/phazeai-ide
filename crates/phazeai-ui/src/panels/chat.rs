//! AI chat panel — the human's pair-programming partner.
//!
//! What makes this more than a text box:
//! * **Editor context** — workspace, active file, cursor, selection, open tabs and live
//!   LSP diagnostics are attached to a message (only when they changed since the last one).
//! * **Memory** — one shared `ConversationHistory` for the whole chat, so "now do the same for
//!   X" works. "New chat" starts fresh.
//! * **System prompt** — project type, git state, project rules (`CLAUDE.md`, `.phazerules`…),
//!   the tool list and IDE-specific guidance.
//! * **Rooted tools** — bash/file tools operate in the IDE workspace, not the process cwd.
//! * **Safety** — mutating tools ask for approval through an inline card; **Stop** cancels.
//! * **Friendly failures** — "Ollama isn't running" instead of a raw socket error.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use floem::{
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, Modifiers},
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::{
    config::LlmProvider,
    mcp::McpManager,
    tools::{BashTool, ToolApprovalManager, ToolApprovalMode},
    Agent, AgentEvent, ApprovalFn, ConversationHistory, Settings, SystemPromptBuilder,
    ToolRegistry,
};

use crate::{
    app::IdeState,
    components::icon::{icons, phaze_icon},
};

// ── Chat Types ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    /// Display content. For the streaming assistant message this is updated live.
    pub content: String,
    /// True while AI is still generating this message.
    pub loading: bool,
}

/// What the background AI thread sends to the Floem UI thread.
#[derive(Clone, Debug)]
enum ChatUpdate {
    /// Partial streamed text — contains the FULL accumulated text so far.
    Partial(String),
    /// Generation complete — final text.
    Done(String),
    /// Tool execution started.
    ToolStart { name: String },
    /// Tool execution finished.
    ToolResult { name: String, summary: String },
    /// A mutating tool wants permission to run.
    Approval { name: String, preview: String },
    /// An error occurred (already user-friendly).
    Err(String),
}

type SharedConversation = Arc<tokio::sync::Mutex<ConversationHistory>>;
type ApprovalSlot = Arc<Mutex<Option<tokio::sync::oneshot::Sender<bool>>>>;
/// MCP servers are expensive to spawn; keep them alive per workspace.
type McpCache = Arc<Mutex<Option<(PathBuf, Arc<Mutex<McpManager>>)>>>;

const WELCOME: &str = "Hi! I can see the file you have open, your selection and its diagnostics. \
Ask me to explain, fix, refactor or test something — or right-click code in the editor.";

/// IDE-specific guidance appended to the base system prompt.
const IDE_GUIDE: &str = "\
## Working inside PhazeAI IDE
- The user is a human developer sitting next to you in an IDE. Be a collaborator: concise, concrete, honest about uncertainty.
- A user message may start with an `<ide_context>` block: workspace root, the file they have open, cursor line, the text they have SELECTED, open tabs and current compiler/LSP diagnostics. \"this\", \"here\" and \"it\" usually refer to the selection or the code at the cursor. Context is only re-sent when it changes, so earlier blocks still apply.
- Read a file before editing it. Make small, targeted edits with the `edit_file` tool rather than rewriting whole files. After changing code, say what you changed and why in a sentence or two.
- File and shell tools run relative to the workspace root. Tools that modify files or run commands ask the user for approval; if denied, propose an alternative instead of retrying.
- Never claim you ran something you did not run.";

// ── Context ───────────────────────────────────────────────────────────────────

fn language_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" => "cpp",
        "java" => "java",
        "rb" => "ruby",
        "sh" | "bash" => "bash",
        "toml" => "toml",
        "json" => "json",
        "md" => "markdown",
        "html" => "html",
        "css" => "css",
        "yml" | "yaml" => "yaml",
        _ => "text",
    }
}

/// Snapshot of what the human is looking at, rendered for the model.
fn build_editor_context(state: &IdeState) -> String {
    let root = state.workspace_root.get_untracked();
    let mut out = format!("workspace: {}\n", root.display());

    let Some(file) = state.open_file.get_untracked() else {
        out.push_str("active_file: (none)\n");
        return out;
    };
    out.push_str(&format!(
        "active_file: {} ({})\n",
        file.display(),
        language_for(&file)
    ));

    if let Some((p, line, col)) = state.active_cursor.get_untracked() {
        if p == file {
            out.push_str(&format!("cursor: line {}, column {}\n", line + 1, col + 1));
        }
    }

    let tabs = state.open_tabs.get_untracked();
    if tabs.len() > 1 {
        let names: Vec<String> = tabs
            .iter()
            .take(12)
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        out.push_str(&format!("open_tabs: {}\n", names.join(", ")));
    }

    let diags: Vec<_> = state
        .diagnostics
        .get_untracked()
        .into_iter()
        .filter(|d| d.path == file)
        .take(12)
        .collect();
    if !diags.is_empty() {
        out.push_str("diagnostics:\n");
        for d in diags {
            out.push_str(&format!(
                "  - line {} [{:?}] {}\n",
                d.line,
                d.severity,
                phazeai_core::text::truncate_bytes(&d.message, 300)
            ));
        }
    }

    let sel = state.editor_selection.get_untracked();
    if !sel.trim().is_empty() {
        let shown = phazeai_core::text::truncate_bytes(&sel, 8000);
        let note = if shown.len() < sel.len() {
            " (truncated)"
        } else {
            ""
        };
        out.push_str(&format!(
            "selection{note}:\n```{}\n{}\n```\n",
            language_for(&file),
            shown
        ));
    }
    out
}

// ── Friendly errors ───────────────────────────────────────────────────────────

/// Turn a raw provider error into something a person can act on.
fn friendly_error(settings: &Settings, raw: &str) -> String {
    let lower = raw.to_lowercase();
    let model = &settings.llm.model;
    let unreachable = lower.contains("error sending request")
        || lower.contains("connection refused")
        || lower.contains("connect error")
        || lower.contains("tcp connect")
        || lower.contains("dns error");

    match settings.llm.provider {
        LlmProvider::Ollama if unreachable => {
            let url = settings
                .llm
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            format!(
                "I can't reach Ollama at {url}.\n\n\
                 • Install it from https://ollama.com and start it (`ollama serve`)\n\
                 • then run `ollama pull {model}`\n\
                 • or pick a different provider in Settings → AI."
            )
        }
        LlmProvider::Ollama if lower.contains("not found") || lower.contains("404") => format!(
            "Ollama is running but the model '{model}' isn't installed.\n\n\
             Run `ollama pull {model}` (or choose another model in Settings → AI)."
        ),
        LlmProvider::LmStudio if unreachable => {
            "I can't reach LM Studio. Start its local server (Developer tab → Start Server) \
             or choose another provider in Settings → AI."
                .to_string()
        }
        _ if lower.contains("environment variable") || lower.contains("api key") => format!(
            "{raw}\n\nSet the key in the environment you launch PhazeAI from \
             (e.g. `export ANTHROPIC_API_KEY=…`), then restart. \
             Local models via Ollama need no key."
        ),
        _ if lower.contains("401") || lower.contains("unauthorized") => {
            "The provider rejected the API key (401). Check that the key is valid and not expired."
                .to_string()
        }
        _ if lower.contains("429") || lower.contains("rate limit") => {
            "The provider is rate-limiting requests (429). Wait a moment and try again.".to_string()
        }
        _ if unreachable => format!("Couldn't reach the model provider: {raw}"),
        _ => raw.to_string(),
    }
}

// ── Agent runner ──────────────────────────────────────────────────────────────

struct RunConfig {
    user_message: String,
    settings: Settings,
    root: PathBuf,
    conversation: SharedConversation,
    approvals: Arc<Mutex<ToolApprovalManager>>,
    slot: ApprovalSlot,
    mcp: McpCache,
    cancel: Arc<AtomicBool>,
}

fn run_agent(cfg: RunConfig, update_tx: std::sync::mpsc::SyncSender<ChatUpdate>) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = update_tx.send(ChatUpdate::Err(format!("Runtime error: {e}")));
                return;
            }
        };

        rt.block_on(async move {
            let RunConfig {
                user_message,
                settings,
                root,
                conversation,
                approvals,
                slot,
                mcp,
                cancel,
            } = cfg;

            let client = match settings.build_llm_client() {
                Ok(c) => c,
                Err(e) => {
                    let _ =
                        update_tx.send(ChatUpdate::Err(friendly_error(&settings, &format!("{e}"))));
                    return;
                }
            };

            // Tools rooted in the IDE workspace (not the process cwd).
            let mut registry = ToolRegistry::default();
            registry.register(Box::new(BashTool::new(root.clone())));
            let tool_names: Vec<String> = registry
                .list()
                .iter()
                .map(|t| t.name().to_string())
                .collect();

            // Fresh system prompt every turn: git branch / dirty files / project rules may change.
            let (branch, dirty) = phazeai_core::collect_git_info(&root);
            let system_prompt = SystemPromptBuilder::new()
                .with_project_root(root.clone())
                .with_git_info(branch, dirty)
                .with_model(&format!("{:?}", settings.llm.provider), &settings.llm.model)
                .with_tools(tool_names)
                .load_project_instructions()
                .with_additional_instructions(IDE_GUIDE.to_string())
                .build();
            conversation.lock().await.set_system_prompt(system_prompt);

            // Approval: read-only tools run freely, everything else asks the human.
            let approval_tx = update_tx.clone();
            let approval_fn: ApprovalFn = Box::new(move |name, params| {
                let approvals = approvals.clone();
                let slot = slot.clone();
                let tx = approval_tx.clone();
                Box::pin(async move {
                    let (needs, preview) = {
                        let m = approvals.lock().unwrap_or_else(|e| e.into_inner());
                        (
                            m.needs_approval(&name, &params),
                            m.format_approval_prompt(&name, &params),
                        )
                    };
                    if !needs {
                        return true;
                    }
                    let (otx, orx) = tokio::sync::oneshot::channel::<bool>();
                    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(otx);
                    let _ = tx.send(ChatUpdate::Approval { name, preview });
                    // Dropped sender (e.g. window closed) counts as "deny".
                    orx.await.unwrap_or(false)
                })
            });

            let mut agent = Agent::new(client)
                .with_tools(registry)
                .with_shared_conversation(conversation)
                .with_approval(approval_fn)
                .with_cancel_token(cancel);

            // MCP servers: spawn once per workspace, reuse across messages.
            let manager = {
                let mut cache = mcp.lock().unwrap_or_else(|e| e.into_inner());
                match cache.as_ref() {
                    Some((r, m)) if *r == root => Some(m.clone()),
                    _ => {
                        let configs = McpManager::load_config(&root);
                        if configs.is_empty() {
                            *cache = None;
                            None
                        } else {
                            let mut m = McpManager::new();
                            m.connect_all(&configs);
                            let m = Arc::new(Mutex::new(m));
                            *cache = Some((root.clone(), m.clone()));
                            Some(m)
                        }
                    }
                }
            };
            if let Some(m) = manager {
                agent.register_mcp_tools(m);
            }

            let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
            let finished = std::cell::Cell::new(false);

            let run_fut = agent.run_with_events(&user_message, agent_tx);
            let drain_fut = async {
                let mut accumulated = String::new();
                while let Some(event) = agent_rx.recv().await {
                    match event {
                        AgentEvent::TextDelta(text) => {
                            accumulated.push_str(&text);
                            let _ = update_tx.send(ChatUpdate::Partial(accumulated.clone()));
                        }
                        AgentEvent::ToolStart { name } => {
                            let _ = update_tx.send(ChatUpdate::ToolStart { name });
                        }
                        AgentEvent::ToolResult { name, summary, .. } => {
                            let _ = update_tx.send(ChatUpdate::ToolResult { name, summary });
                        }
                        AgentEvent::Complete { .. } => {
                            finished.set(true);
                            let _ = update_tx.send(ChatUpdate::Done(accumulated.clone()));
                            break;
                        }
                        AgentEvent::Error(e) => {
                            finished.set(true);
                            let msg = if e == "Cancelled" {
                                "Stopped.".to_string()
                            } else {
                                friendly_error(&settings, &e)
                            };
                            let _ = update_tx.send(ChatUpdate::Err(msg));
                            break;
                        }
                        _ => {}
                    }
                }
                accumulated
            };

            let (result, accumulated) = tokio::join!(run_fut, drain_fut);
            // Guarantee the UI always leaves its "loading" state.
            if !finished.get() {
                match result {
                    Ok(_) => {
                        let _ = update_tx.send(ChatUpdate::Done(accumulated));
                    }
                    Err(e) => {
                        let _ = update_tx
                            .send(ChatUpdate::Err(friendly_error(&settings, &format!("{e}"))));
                    }
                }
            }
        });
    });
}

// ── Chat Panel ────────────────────────────────────────────────────────────────

/// Full AI chat panel with real streaming responses and neon-glass aesthetics.
pub fn chat_panel(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    let ai_thinking = state.ai_thinking;

    let welcome = || ChatMessage {
        role: ChatRole::Assistant,
        content: WELCOME.to_string(),
        loading: false,
    };
    let messages: RwSignal<Vec<ChatMessage>> = create_rw_signal(vec![welcome()]);
    let input_text = create_rw_signal(String::new());
    let is_loading = create_rw_signal(false);
    // (tool name, preview) of a tool call waiting for the human's decision.
    let pending_approval: RwSignal<Option<(String, String)>> = create_rw_signal(None);

    // Shared, long-lived pieces (survive across messages).
    let conversation: Rc<RefCell<SharedConversation>> = Rc::new(RefCell::new(Arc::new(
        tokio::sync::Mutex::new(ConversationHistory::new()),
    )));
    let approvals = Arc::new(Mutex::new(ToolApprovalManager::new(
        ToolApprovalMode::AlwaysAsk,
    )));
    let slot: ApprovalSlot = Arc::new(Mutex::new(None));
    let mcp: McpCache = Arc::new(Mutex::new(None));
    let cancel: Rc<RefCell<Arc<AtomicBool>>> =
        Rc::new(RefCell::new(Arc::new(AtomicBool::new(false))));
    let last_context: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<ChatUpdate>(256);
    let update_signal = create_signal_from_channel(update_rx);

    create_effect(move |_| {
        if let Some(update) = update_signal.get() {
            match update {
                ChatUpdate::Partial(text) => {
                    messages.update(|list| {
                        if let Some(last) = list.last_mut() {
                            if last.role == ChatRole::Assistant && last.loading {
                                last.content = text;
                            }
                        }
                    });
                }
                ChatUpdate::ToolStart { name } => {
                    messages.update(|list| {
                        list.push(ChatMessage {
                            role: ChatRole::Tool,
                            content: format!("Running tool: {name}..."),
                            loading: true,
                        });
                    });
                }
                ChatUpdate::ToolResult { name, summary } => {
                    messages.update(|list| {
                        if let Some(last) = list
                            .iter_mut()
                            .rev()
                            .find(|m| m.role == ChatRole::Tool && m.loading)
                        {
                            last.content = format!("{name}: {summary}");
                            last.loading = false;
                        }
                    });
                }
                ChatUpdate::Approval { name, preview } => {
                    pending_approval.set(Some((name, preview)));
                }
                ChatUpdate::Done(text) => {
                    messages.update(|list| {
                        if let Some(last) = list
                            .iter_mut()
                            .rev()
                            .find(|m| m.role == ChatRole::Assistant && m.loading)
                        {
                            last.content = if text.is_empty() {
                                "(no response)".to_string()
                            } else {
                                text
                            };
                            last.loading = false;
                        }
                    });
                    pending_approval.set(None);
                    is_loading.set(false);
                    ai_thinking.set(false);
                }
                ChatUpdate::Err(e) => {
                    messages.update(|list| {
                        if let Some(last) = list.iter_mut().rev().find(|m| m.loading) {
                            last.content = e;
                            last.loading = false;
                        }
                        // Also close any dangling tool cards.
                        for m in list.iter_mut().filter(|m| m.loading) {
                            m.loading = false;
                        }
                    });
                    pending_approval.set(None);
                    is_loading.set(false);
                    ai_thinking.set(false);
                }
            }
        }
    });

    // ── Send ──────────────────────────────────────────────────────────────────

    let update_tx = Arc::new(update_tx);

    let send_text: Rc<dyn Fn(String)> = Rc::new({
        let update_tx = update_tx.clone();
        let conversation = conversation.clone();
        let approvals = approvals.clone();
        let slot = slot.clone();
        let mcp = mcp.clone();
        let cancel = cancel.clone();
        let last_context = last_context.clone();
        let state = state.clone();
        move |text: String| {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() || is_loading.get_untracked() {
                return;
            }

            // Attach editor context only when it changed since the last message.
            let ctx = build_editor_context(&state);
            let message_for_model = {
                let mut last = last_context.borrow_mut();
                if *last != ctx {
                    *last = ctx.clone();
                    format!("<ide_context>\n{ctx}</ide_context>\n\n{trimmed}")
                } else {
                    trimmed.clone()
                }
            };

            messages.update(|list| {
                list.push(ChatMessage {
                    role: ChatRole::User,
                    content: trimmed,
                    loading: false,
                });
                list.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: String::new(),
                    loading: true,
                });
            });
            input_text.set(String::new());
            is_loading.set(true);
            ai_thinking.set(true);

            let root = state.workspace_root.get_untracked();
            if root.is_dir() {
                // Keep relative paths in file tools consistent with the IDE workspace.
                let _ = std::env::set_current_dir(&root);
            }

            let token = Arc::new(AtomicBool::new(false));
            *cancel.borrow_mut() = token.clone();

            run_agent(
                RunConfig {
                    user_message: message_for_model,
                    // Re-read so provider/model changes in Settings apply immediately.
                    settings: Settings::load(),
                    root,
                    conversation: conversation.borrow().clone(),
                    approvals: approvals.clone(),
                    slot: slot.clone(),
                    mcp: mcp.clone(),
                    cancel: token,
                },
                (*update_tx).clone(),
            );
        }
    });

    // Prompts queued by right-click AI actions / "Fix with AI" etc.
    {
        let send_text = send_text.clone();
        create_effect(move |_| {
            if let Some(prompt) = state.pending_ai_prompt.get() {
                state.pending_ai_prompt.set(None);
                state.show_right_panel.set(true);
                if is_loading.get_untracked() {
                    // Don't lose it — park it in the input box for the user.
                    input_text.set(prompt);
                } else {
                    send_text(prompt);
                }
            }
        });
    }

    // ── Approval + stop + new chat actions ────────────────────────────────────

    let respond: Rc<dyn Fn(bool, bool)> = Rc::new({
        let slot = slot.clone();
        let approvals = approvals.clone();
        move |allow: bool, allow_all: bool| {
            if allow_all {
                approvals
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_mode(ToolApprovalMode::AutoApprove);
            }
            if let Some(tx) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = tx.send(allow);
            }
            pending_approval.set(None);
        }
    });

    let stop: Rc<dyn Fn()> = Rc::new({
        let cancel = cancel.clone();
        let respond = respond.clone();
        move || {
            cancel.borrow().store(true, Ordering::Relaxed);
            // Unblock a pending approval so the agent loop can observe the cancel.
            respond(false, false);
        }
    });

    let new_chat: Rc<dyn Fn()> = Rc::new({
        let conversation = conversation.clone();
        let last_context = last_context.clone();
        let approvals = approvals.clone();
        let stop = stop.clone();
        move || {
            if is_loading.get_untracked() {
                stop();
            }
            *conversation.borrow_mut() =
                Arc::new(tokio::sync::Mutex::new(ConversationHistory::new()));
            last_context.borrow_mut().clear();
            // A new chat also resets "allow all" back to asking.
            approvals
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_mode(ToolApprovalMode::AlwaysAsk);
            messages.set(vec![ChatMessage {
                role: ChatRole::Assistant,
                content: WELCOME.to_string(),
                loading: false,
            }]);
            pending_approval.set(None);
            is_loading.set(false);
            ai_thinking.set(false);
        }
    });

    // ── Header ────────────────────────────────────────────────────────────────

    let neon_strip = container(label(|| "")).style(move |s| {
        s.height(2.0)
            .width_full()
            .background(theme.get().palette.accent)
    });

    let header_button = move |text: &'static str, tip_color_error: bool| {
        container(label(move || text)).style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .padding_horiz(8.0)
                .padding_vert(3.0)
                .border_radius(5.0)
                .border(1.0)
                .border_color(p.glass_border)
                .color(if tip_color_error {
                    p.error
                } else {
                    p.text_muted
                })
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(p.bg_elevated).color(p.text_primary))
        })
    };

    let new_btn = {
        let new_chat = new_chat.clone();
        header_button("↺ New", false).on_click_stop(move |_| new_chat())
    };
    let stop_btn = {
        let stop = stop.clone();
        header_button("■ Stop", true)
            .on_click_stop(move |_| stop())
            .style(move |s| {
                s.apply_if(!is_loading.get(), |s| {
                    s.display(floem::style::Display::None)
                })
            })
    };

    let header_content = container(
        stack((
            phaze_icon(icons::AI, 14.0, move |p| p.accent, theme),
            label(|| "  PHAZEAI").style(move |s| {
                s.font_size(11.0)
                    .color(theme.get().palette.accent)
                    .font_weight(floem::text::Weight::BOLD)
                    .flex_grow(1.0)
            }),
            stop_btn,
            new_btn,
        ))
        .style(|s| s.items_center().gap(6.0).width_full()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.padding_horiz(14.0)
            .padding_vert(10.0)
            .border_bottom(1.0)
            .border_color(p.glass_border)
            .width_full()
            .background(p.glass_bg)
    });

    // "What the AI can see" strip — transparency about context.
    let context_strip = label(move || {
        let file = state
            .open_file
            .get()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
        let sel = state.editor_selection.get();
        let model = state.ai_model.get();
        let mut parts = Vec::new();
        parts.push(match file {
            Some(f) => format!("📄 {f}"),
            None => "no file open".to_string(),
        });
        if !sel.trim().is_empty() {
            parts.push(format!("✂ {} lines selected", sel.lines().count().max(1)));
        }
        parts.push(format!("🧠 {model}"));
        parts.join("  ·  ")
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(10.0)
            .color(p.text_muted)
            .padding_horiz(14.0)
            .padding_vert(5.0)
            .width_full()
            .background(p.bg_deep.with_alpha(0.5))
    });

    let header =
        stack((neon_strip, header_content, context_strip)).style(|s| s.flex_col().width_full());

    // ── Message bubbles ───────────────────────────────────────────────────────

    let msg_list = dyn_stack(
        move || messages.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, m)| (*i, m.content.len(), m.loading),
        move |(_, msg)| {
            let is_user = msg.role == ChatRole::User;
            let content = msg.content.clone();
            let loading = msg.loading;

            let text_content = if loading && content.is_empty() {
                "●●●".to_string()
            } else {
                content
            };
            let is_typing = loading && text_content.starts_with('●');
            let is_tool = msg.role == ChatRole::Tool;

            container(
                stack((
                    phaze_icon(icons::CHIP, 11.0, move |p| p.accent, theme).style(
                        move |s: floem::style::Style| {
                            s.apply_if(!is_tool, |s| s.display(floem::style::Display::None))
                        },
                    ),
                    label(move || text_content.clone()).style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(if is_tool { 11.0 } else { 13.0 })
                            .color(if is_user {
                                p.text_primary
                            } else if is_typing || is_tool {
                                p.accent
                            } else {
                                p.text_secondary
                            })
                            .max_width_pct(100.0)
                            .line_height(1.5)
                            .apply_if(is_tool, |s| s.font_weight(floem::text::Weight::MEDIUM))
                    }),
                ))
                .style(|s| s.items_center()),
            )
            .style(move |s| {
                let p = theme.get().palette;
                if is_user {
                    s.width_full()
                        .padding_horiz(14.0)
                        .padding_vert(10.0)
                        .background(p.accent_dim)
                        .border(1.0)
                        .border_color(p.glass_border)
                        .border_radius(12.0)
                        .margin_bottom(8.0)
                        .box_shadow_blur(12.0)
                        .box_shadow_color(p.glow)
                        .box_shadow_spread(0.0)
                        .box_shadow_h_offset(0.0)
                        .box_shadow_v_offset(0.0)
                } else if is_tool {
                    s.width_full()
                        .padding_horiz(10.0)
                        .padding_vert(6.0)
                        .background(p.bg_deep.with_alpha(0.6))
                        .border(1.0)
                        .border_color(p.glass_border)
                        .border_radius(6.0)
                        .margin_bottom(6.0)
                        .margin_horiz(20.0)
                } else {
                    s.width_full()
                        .padding_horiz(14.0)
                        .padding_vert(10.0)
                        .background(p.bg_panel)
                        .border(1.0)
                        .border_color(p.glass_border)
                        .border_radius(10.0)
                        .margin_bottom(8.0)
                }
            })
        },
    )
    .style(|s| s.flex_col().padding(10.0).gap(0.0).width_full());

    let messages_scroll = scroll(msg_list).style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    // ── Approval card ─────────────────────────────────────────────────────────

    let approval_button = move |text: &'static str, primary: bool| {
        container(label(move || text)).style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .padding_horiz(10.0)
                .padding_vert(5.0)
                .border_radius(6.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .color(if primary { p.bg_base } else { p.text_primary })
                .background(if primary { p.accent } else { p.bg_elevated })
                .border(1.0)
                .border_color(p.glass_border)
        })
    };

    let approval_card = container(
        stack((
            label(move || match pending_approval.get() {
                Some((name, _)) => format!("🔐 The AI wants to run “{name}”"),
                None => String::new(),
            })
            .style(move |s| {
                s.font_size(12.0)
                    .font_weight(floem::text::Weight::BOLD)
                    .color(theme.get().palette.accent)
            }),
            label(move || {
                pending_approval
                    .get()
                    .map(|(_, preview)| preview)
                    .unwrap_or_default()
            })
            .style(move |s| {
                s.font_size(11.0)
                    .color(theme.get().palette.text_secondary)
                    .max_width_pct(100.0)
                    .padding_vert(4.0)
            }),
            stack((
                {
                    let respond = respond.clone();
                    approval_button("Allow", true).on_click_stop(move |_| respond(true, false))
                },
                {
                    let respond = respond.clone();
                    approval_button("Deny", false).on_click_stop(move |_| respond(false, false))
                },
                {
                    let respond = respond.clone();
                    approval_button("Allow all this chat", false)
                        .on_click_stop(move |_| respond(true, true))
                },
            ))
            .style(|s| s.gap(6.0)),
        ))
        .style(|s| s.flex_col().gap(2.0).width_full()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.padding(12.0)
            .margin_horiz(10.0)
            .margin_bottom(6.0)
            .border(1.0)
            .border_color(p.accent)
            .border_radius(8.0)
            .background(p.bg_panel)
            .apply_if(pending_approval.get().is_none(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    // ── Input bar ─────────────────────────────────────────────────────────────

    let send_from_input: Rc<dyn Fn()> = Rc::new({
        let send_text = send_text.clone();
        move || send_text(input_text.get_untracked())
    });
    let do_send_btn = send_from_input.clone();
    let do_send_key = send_from_input.clone();

    let send_btn = container(label(|| "↵").style(move |s| {
        s.font_size(14.0).color(if is_loading.get() {
            theme.get().palette.text_disabled
        } else {
            theme.get().palette.bg_base
        })
    }))
    .style(move |s| {
        let p = theme.get().palette;
        let loading = is_loading.get();
        s.width(32.0)
            .height(32.0)
            .background(if loading { p.bg_elevated } else { p.accent })
            .border_radius(8.0)
            .items_center()
            .justify_center()
            .cursor(floem::style::CursorStyle::Pointer)
            .margin_left(8.0)
            .apply_if(!loading, |s| {
                s.box_shadow_blur(10.0)
                    .box_shadow_color(p.glow)
                    .box_shadow_spread(0.0)
                    .box_shadow_h_offset(0.0)
                    .box_shadow_v_offset(0.0)
            })
    })
    .on_click_stop(move |_| (do_send_btn)());

    let input_widget = text_input(input_text)
        .placeholder("Ask about your code…")
        .style(move |s| {
            let p = theme.get().palette;
            s.flex_grow(1.0)
                .background(p.glass_bg)
                .border(1.0)
                .border_color(p.border_focus)
                .border_radius(8.0)
                .color(p.text_primary)
                .padding_horiz(12.0)
                .padding_vert(8.0)
                .font_size(13.0)
                .min_width(0.0)
        })
        .on_event_stop(EventListener::KeyDown, move |event| {
            if let Event::KeyDown(e) = event {
                let enter = match &e.key.logical_key {
                    Key::Character(ch) => ch.as_str() == "\r" || ch.as_str() == "\n",
                    Key::Named(floem::keyboard::NamedKey::Enter) => true,
                    _ => false,
                };
                if enter && !e.modifiers.contains(Modifiers::SHIFT) {
                    (do_send_key)();
                }
            }
        });

    let input_bar = container(
        stack((input_widget, send_btn)).style(|s| s.items_center().width_full()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.padding(10.0)
            .border_top(1.0)
            .border_color(p.glass_border)
            .width_full()
            .background(p.glass_bg)
    });

    // ── Full panel ────────────────────────────────────────────────────────────

    stack((header, messages_scroll, approval_card, input_bar)).style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width(340.0)
            .height_full()
            .background(p.glass_bg)
            .border_left(1.0)
            .border_color(p.glass_border)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ollama() -> Settings {
        let mut s = Settings::default();
        s.llm.provider = LlmProvider::Ollama;
        s.llm.model = "qwen2.5-coder:7b".into();
        s
    }

    #[test]
    fn ollama_down_tells_the_user_how_to_fix_it() {
        let msg = friendly_error(
            &ollama(),
            "error sending request for url (http://localhost:11434/api/chat)",
        );
        assert!(msg.contains("can't reach Ollama"), "{msg}");
        assert!(msg.contains("ollama pull qwen2.5-coder:7b"), "{msg}");
        assert!(
            !msg.contains("error sending request"),
            "raw error leaked: {msg}"
        );
    }

    #[test]
    fn missing_model_names_the_model() {
        let msg = friendly_error(
            &ollama(),
            "model 'qwen2.5-coder:7b' not found, try pulling it first (404)",
        );
        assert!(
            msg.contains("isn't installed") && msg.contains("ollama pull"),
            "{msg}"
        );
    }

    #[test]
    fn missing_api_key_explains_env_var() {
        let mut s = ollama();
        s.llm.provider = LlmProvider::Claude;
        let msg = friendly_error(&s, "Set ANTHROPIC_API_KEY environment variable for Claude");
        assert!(
            msg.contains("ANTHROPIC_API_KEY") && msg.contains("restart"),
            "{msg}"
        );
    }

    #[test]
    fn auth_and_rate_limit_are_readable() {
        let mut s = ollama();
        s.llm.provider = LlmProvider::OpenAI;
        assert!(friendly_error(&s, "HTTP 401 Unauthorized").contains("API key"));
        assert!(friendly_error(&s, "HTTP 429 Too Many Requests").contains("rate-limiting"));
    }

    #[test]
    fn unknown_errors_pass_through_unchanged() {
        assert_eq!(friendly_error(&ollama(), "something odd"), "something odd");
    }

    #[test]
    fn language_detection_by_extension() {
        assert_eq!(language_for(Path::new("a/b/main.rs")), "rust");
        assert_eq!(language_for(Path::new("x.tsx")), "tsx");
        assert_eq!(language_for(Path::new("Makefile")), "text");
    }
}
