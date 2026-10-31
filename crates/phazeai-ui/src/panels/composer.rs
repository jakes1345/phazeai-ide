use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::Duration;

use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, text_input, v_stack, Decorators},
    IntoView,
};
use phazeai_core::tools::{BashTool, ToolApprovalManager, ToolPermission, ToolRegistry};
use phazeai_core::{Agent, AgentEvent, ApprovalFn, Settings};
use serde_json::Value;

use crate::components::button::{phaze_button, ButtonVariant};
use crate::domain_state::IdeState;
use crate::util::safe_get;

// ── Approval Mode ─────────────────────────────────────────────────────────────

/// The three composer approval modes exposed in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerApprovalMode {
    /// Auto-approve every tool call (existing behavior, now explicit).
    AutoAll,
    /// Auto-approve read-only tools; require approval for write/bash/destructive.
    ApproveDestructive,
    /// Require approval before every tool call.
    ApproveAll,
}

impl ComposerApprovalMode {
    fn label(self) -> &'static str {
        match self {
            Self::AutoAll => "Auto-approve all",
            Self::ApproveDestructive => "Approve destructive",
            Self::ApproveAll => "Approve all",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::AutoAll => Self::ApproveDestructive,
            Self::ApproveDestructive => Self::ApproveAll,
            Self::ApproveAll => Self::AutoAll,
        }
    }

    /// Decide whether a tool+params pair requires user approval under this mode.
    fn needs_approval(self, tool_name: &str, params: &Value) -> bool {
        match self {
            Self::AutoAll => false,
            Self::ApproveDestructive => {
                // Only block Write, Execute, or Destructive — not ReadOnly.
                let perm = ToolApprovalManager::default().classify_tool(tool_name, params);
                perm != ToolPermission::ReadOnly
            }
            // Every call, read-only ones (read_file, code_search, `ls`) included.
            Self::ApproveAll => true,
        }
    }
}

// ── Composer Event Types ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum ComposerUpdate {
    /// Agent is thinking (new iteration).
    Thinking(usize),
    /// Partial streamed text from agent.
    TextDelta(String),
    /// Tool execution started.
    ToolStart { name: String, params: Value },
    /// Tool execution finished.
    ToolResult {
        name: String,
        success: bool,
        summary: String,
    },
    /// A tool call waiting for the user's answer, which goes back on `reply`
    /// (that call's own channel).
    ToolApprovalRequest {
        name: String,
        params: Value,
        reply: SyncSender<ApprovalResponse>,
    },
    /// Agent run completed.
    Done { iterations: usize },
    /// Error occurred.
    Err(String),
    /// Git diff output after completion.
    DiffOutput(Vec<DiffCard>),
    /// MCP stdio server(s) restarted.
    McpStatus(String),
    /// Something worth knowing about this run (e.g. context was compacted).
    Notice(String),
}

#[derive(Clone, Debug)]
struct DiffCard {
    file: String,
    diff: String,
}

#[derive(Clone, Debug)]
struct EventLogEntry {
    kind: EventKind,
    text: String,
    /// Optional file/path extracted from tool params — shown distinctly.
    path: Option<String>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum EventKind {
    Thinking,
    Text,
    ToolStart,
    ToolResult,
    ApprovalPending,
    Done,
    Error,
    Diff,
    Warning,
}

// ── Pending approval request ─────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct PendingApproval {
    tool_name: String,
    params: Value,
    /// The waiting call's own answer channel.
    reply: SyncSender<ApprovalResponse>,
}

impl PendingApproval {
    /// Answer the call this prompt shows, and only that call. Never blocks:
    /// the call takes one answer, and once it has stopped waiting (answered,
    /// timed out) the answer goes nowhere.
    fn answer(&self, response: ApprovalResponse) {
        let _ = self.reply.try_send(response);
    }
}

// Channel message used to deliver approval decision back to the agent thread.
#[derive(Clone, Debug)]
enum ApprovalResponse {
    Approved,
    Denied,
}

/// How long a tool call waits for the user's answer before it counts as denied.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

/// The approval function the agent gets in `mode` (in Auto-approve all it
/// gets none). A call that needs an answer sends a `ToolApprovalRequest` with
/// a reply channel of its own and waits on that channel alone, so a click can
/// only answer the call its prompt shows. A call that needs none opens no
/// prompt.
fn approval_fn(mode: ComposerApprovalMode, updates: SyncSender<ComposerUpdate>) -> ApprovalFn {
    Box::new(move |tool_name: String, params: Value| {
        let updates = updates.clone();
        Box::pin(async move {
            if !mode.needs_approval(&tool_name, &params) {
                return true;
            }
            let (reply, answer) = std::sync::mpsc::sync_channel(1);
            let _ = updates.send(ComposerUpdate::ToolApprovalRequest {
                name: tool_name,
                params,
                reply,
            });
            // Wait off the runtime's thread; no answer in time counts as Deny.
            let answer = tokio::task::spawn_blocking(move || {
                answer
                    .recv_timeout(APPROVAL_TIMEOUT)
                    .unwrap_or(ApprovalResponse::Denied)
            })
            .await
            .unwrap_or(ApprovalResponse::Denied);
            matches!(answer, ApprovalResponse::Approved)
        })
    })
}

// ── Composer Panel ───────────────────────────────────────────────────────────

/// Multi-file AI agent panel. Runs the full Agent with all tools and streams
/// events. Includes workspace display, approval-mode toggle, tool visibility,
/// and a no-git-repo warning banner.
pub fn composer_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let task_input = create_rw_signal(String::new());
    let is_running = create_rw_signal(false);
    let event_log: RwSignal<Vec<EventLogEntry>> = create_rw_signal(Vec::new());
    let diff_cards: RwSignal<Vec<DiffCard>> = create_rw_signal(Vec::new());
    let agent_text = create_rw_signal(String::new());
    let cancel_token: RwSignal<Option<Arc<AtomicBool>>> = create_rw_signal(None);

    // Approval mode — default to ApproveDestructive for safer beta experience.
    let approval_mode: RwSignal<ComposerApprovalMode> =
        create_rw_signal(ComposerApprovalMode::ApproveDestructive);

    // Pending approval — Some when the agent is waiting for the user.
    let pending_approval: RwSignal<Option<PendingApproval>> = create_rw_signal(None);

    // Whether the workspace is a git repo.
    let is_git_repo: RwSignal<bool> = create_rw_signal(true);

    // Check git repo status on load and whenever workspace changes.
    {
        let workspace = state.project.workspace_root;
        create_effect(move |_| {
            let ws = workspace.get();
            let has_git = std::path::Path::new(&ws).join(".git").exists()
                || phazeai_core::process::std_command("git")
                    .args(["rev-parse", "--is-inside-work-tree"])
                    .current_dir(&ws)
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false);
            is_git_repo.set(has_git);
        });
    }

    // Single channel pair for all agent updates — created once.
    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<ComposerUpdate>(512);
    let update_signal = create_signal_from_channel(update_rx);

    // Process incoming updates from the agent thread.
    create_effect(move |_| {
        if let Some(update) = update_signal.get() {
            match update {
                ComposerUpdate::Thinking(iter) => {
                    // Clear any stale pending approval on a new iteration.
                    pending_approval.set(None);
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::Thinking,
                            text: format!("Iteration {}", iter),
                            path: None,
                        });
                        if log.len() > 500 {
                            log.drain(0..log.len() - 500);
                        }
                    });
                }
                ComposerUpdate::TextDelta(text) => {
                    agent_text.set(text);
                }
                ComposerUpdate::ToolStart { name, params } => {
                    let path = extract_path_from_params(&name, &params);
                    let display = format_tool_display(&name, &params);
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::ToolStart,
                            text: display,
                            path,
                        });
                    });
                }
                ComposerUpdate::ToolResult {
                    name,
                    success,
                    summary,
                } => {
                    let icon = if success { "+" } else { "x" };
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::ToolResult,
                            text: format!("[{}] {} done: {}", icon, name, summary),
                            path: None,
                        });
                    });
                }
                ComposerUpdate::ToolApprovalRequest {
                    name,
                    params,
                    reply,
                } => {
                    let pending = PendingApproval {
                        tool_name: name,
                        params,
                        reply,
                    };
                    // Stop was clicked while this request was on its way: deny
                    // it rather than ask, so the run still stops.
                    if cancel_token
                        .get_untracked()
                        .is_some_and(|token| token.load(Ordering::Relaxed))
                    {
                        pending.answer(ApprovalResponse::Denied);
                        return;
                    }
                    let path = extract_path_from_params(&pending.tool_name, &pending.params);
                    let display = format_tool_display(&pending.tool_name, &pending.params);
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::ApprovalPending,
                            text: format!("Waiting for approval: {}", display),
                            path: path.clone(),
                        });
                    });
                    pending_approval.set(Some(pending));
                }
                ComposerUpdate::Done { iterations } => {
                    pending_approval.set(None);
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::Done,
                            text: format!("Completed in {} iterations", iterations),
                            path: None,
                        });
                    });
                    is_running.set(false);
                    state.ai.thinking.set(false);
                    cancel_token.set(None);
                }
                ComposerUpdate::Err(e) => {
                    pending_approval.set(None);
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::Error,
                            text: format!("Error: {}", e),
                            path: None,
                        });
                    });
                    is_running.set(false);
                    state.ai.thinking.set(false);
                    cancel_token.set(None);
                }
                ComposerUpdate::McpStatus(msg) | ComposerUpdate::Notice(msg) => {
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::Warning,
                            text: msg.clone(),
                            path: None,
                        });
                        if log.len() > 500 {
                            log.drain(0..log.len() - 500);
                        }
                    });
                    state.workbench.status_toast.set(Some(msg));
                }
                ComposerUpdate::DiffOutput(cards) => {
                    diff_cards.set(cards);
                }
            }
        }
    });

    // ── Run action ───────────────────────────────────────────────────────────

    // Set to true when Run is clicked in a non-git workspace — shows confirm widget.
    let needs_git_confirm: RwSignal<bool> = create_rw_signal(false);

    let update_tx = Arc::new(update_tx);

    let run_action: Rc<dyn Fn()> = Rc::new({
        let update_tx = update_tx.clone();
        let workspace = state.project.workspace_root;
        // The shared code search (unless the settings turn it off) for the agent's tools.
        let code_search = state.project.code_search_if_enabled();
        move || {
            let task = task_input.get();
            let trimmed = task.trim().to_string();
            if trimmed.is_empty() || is_running.get() {
                return;
            }

            // Clear previous run
            event_log.set(vec![EventLogEntry {
                kind: EventKind::Thinking,
                text: "Starting agent...".to_string(),
                path: None,
            }]);
            diff_cards.set(Vec::new());
            agent_text.set(String::new());
            pending_approval.set(None);
            is_running.set(true);
            state.ai.thinking.set(true);

            let token = Arc::new(AtomicBool::new(false));
            cancel_token.set(Some(token.clone()));

            let tx = (*update_tx).clone();
            let ws = workspace.get_untracked();
            let mode = approval_mode.get_untracked();
            let code_search = code_search.clone();

            std::thread::spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(ComposerUpdate::Err(format!("Runtime error: {e}")));
                        return;
                    }
                };

                rt.block_on(async move {
                    // Build LLM client from settings
                    let settings = Settings::load();
                    let client = match settings.build_llm_client() {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.send(ComposerUpdate::Err(format!("LLM init error: {e}")));
                            return;
                        }
                    };

                    // Build agent with all tools + workspace-aware bash
                    let mut tools = ToolRegistry::default();
                    tools.register(Box::new(BashTool::new(ws.clone())));
                    let mut agent = Agent::new(client)
                        .with_tools(tools)
                        .with_context_budget(settings.context_budget())
                        .with_cancel_token(token);

                    // Let the agent search the workspace's code.
                    if let Some(search) = code_search {
                        agent.register_tool(Box::new(phazeai_core::CodeSearchTool::new(
                            search.clone(),
                        )));
                        agent.register_tool(Box::new(phazeai_core::BuildIndexTool::new(search)));
                    }

                    // Connect MCP servers
                    let mcp_configs = phazeai_core::mcp::McpManager::load_config(&ws);
                    if !mcp_configs.is_empty() {
                        let mut mcp_manager = phazeai_core::mcp::McpManager::new();
                        mcp_manager.connect_all(&mcp_configs);
                        agent.register_mcp_tools(Arc::new(std::sync::Mutex::new(mcp_manager)));
                    }

                    // Wire approval function when mode is not AutoAll.
                    if mode != ComposerApprovalMode::AutoAll {
                        agent = agent.with_approval(approval_fn(mode, tx.clone()));
                    }

                    let (agent_tx, mut agent_rx) =
                        tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

                    let run_fut = agent.run_with_events(&trimmed, agent_tx);
                    let tx2 = tx.clone();
                    let drain_fut = async move {
                        let mut accumulated = String::new();
                        while let Some(event) = agent_rx.recv().await {
                            match event {
                                AgentEvent::Thinking { iteration } => {
                                    let _ = tx2.send(ComposerUpdate::Thinking(iteration));
                                }
                                AgentEvent::TextDelta(text) => {
                                    accumulated.push_str(&text);
                                    let _ =
                                        tx2.send(ComposerUpdate::TextDelta(accumulated.clone()));
                                }
                                AgentEvent::ToolStart { name } => {
                                    // We don't have params here — use empty.
                                    let _ = tx2.send(ComposerUpdate::ToolStart {
                                        name,
                                        params: Value::Null,
                                    });
                                }
                                AgentEvent::ToolResult {
                                    name,
                                    success,
                                    summary,
                                } => {
                                    let _ = tx2.send(ComposerUpdate::ToolResult {
                                        name,
                                        success,
                                        summary,
                                    });
                                }
                                // Sent for every call put to the approval
                                // function, including those it lets through
                                // unasked; it opens the prompt itself when it
                                // needs an answer, so this one shows nothing.
                                AgentEvent::ToolApprovalRequest { .. } => {}
                                AgentEvent::McpReconnected { servers } => {
                                    let msg = if servers.len() == 1 {
                                        format!("MCP server '{}' reconnected", servers[0])
                                    } else {
                                        format!("MCP servers reconnected: {}", servers.join(", "))
                                    };
                                    let _ = tx2.send(ComposerUpdate::McpStatus(msg));
                                }
                                AgentEvent::ContextCompacted {
                                    messages_folded,
                                    tokens_before,
                                    tokens_after,
                                } => {
                                    let _ = tx2.send(ComposerUpdate::Notice(format!(
                                        "Summarised {messages_folded} earlier messages to stay \
                                         within the context budget (~{tokens_before} → \
                                         ~{tokens_after} tokens)"
                                    )));
                                }
                                AgentEvent::Complete { iterations } => {
                                    let _ = tx2.send(ComposerUpdate::Done { iterations });
                                    break;
                                }
                                AgentEvent::Error(e) => {
                                    let _ = tx2.send(ComposerUpdate::Err(e));
                                    break;
                                }
                                _ => {}
                            }
                        }
                    };

                    let _ = tokio::join!(run_fut, drain_fut);

                    // After completion, get git diff to show changed files
                    if let Ok(output) = phazeai_core::process::std_command("git")
                        .args(["diff", "HEAD"])
                        .current_dir(&ws)
                        .output()
                    {
                        if output.status.success() {
                            let diff_text = String::from_utf8_lossy(&output.stdout).to_string();
                            let cards = parse_diff_cards(&diff_text);
                            if !cards.is_empty() {
                                let _ = tx.send(ComposerUpdate::DiffOutput(cards));
                            }
                        }
                    }
                });
            });
        }
    });

    // Answer the call the prompt shows (if one is showing) and hide the prompt.
    let answer_pending = move |response: ApprovalResponse| {
        if let Some(pending) = pending_approval.get_untracked() {
            pending.answer(response);
        }
        pending_approval.set(None);
    };

    let stop_action = move || {
        if let Some(token) = cancel_token.get_untracked() {
            token.store(true, Ordering::Relaxed);
        }
        // Deny the call waiting for approval, if any, so the agent thread can exit.
        answer_pending(ApprovalResponse::Denied);
    };

    // ── UI ───────────────────────────────────────────────────────────────────

    // ── Header bar ───────────────────────────────────────────────────────────
    let header = container(
        h_stack((
            label(|| "COMPOSER".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0)
                    .font_weight(floem::text::Weight::BOLD)
                    .color(p.text_muted)
                    .flex_grow(1.0)
            }),
            // Approval mode toggle button — cycles through the three modes.
            container(label(move || {
                format!("Mode: {}", approval_mode.get().label())
            }))
            .style(move |s| {
                let p = theme.get().palette;
                let mode = approval_mode.get();
                let bg = match mode {
                    ComposerApprovalMode::AutoAll => p.warning.with_alpha(0.18),
                    ComposerApprovalMode::ApproveDestructive => p.accent.with_alpha(0.18),
                    ComposerApprovalMode::ApproveAll => p.success.with_alpha(0.18),
                };
                let fg = match mode {
                    ComposerApprovalMode::AutoAll => p.warning,
                    ComposerApprovalMode::ApproveDestructive => p.accent,
                    ComposerApprovalMode::ApproveAll => p.success,
                };
                s.padding_horiz(8.0)
                    .padding_vert(3.0)
                    .font_size(10.0)
                    .color(fg)
                    .background(bg)
                    .border(1.0)
                    .border_color(fg.with_alpha(0.4))
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| {
                approval_mode.update(|m| *m = m.next());
            }),
        ))
        .style(|s| {
            s.width_full()
                .items_center()
                .gap(8.0)
                .padding_horiz(12.0)
                .padding_vert(8.0)
        }),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full()
            .border_bottom(1.0)
            .border_color(p.glass_border)
    });

    // ── Workspace path bar ────────────────────────────────────────────────────
    let workspace_bar = container(
        h_stack((
            label(|| "CWD:".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(10.0).color(p.text_muted).min_width(30.0)
            }),
            label(move || state.project.workspace_root.get().display().to_string()).style(
                move |s| {
                    let p = theme.get().palette;
                    s.font_size(10.0)
                        .color(p.text_secondary)
                        .font_family("monospace".to_string())
                },
            ),
        ))
        .style(|s| s.items_center().gap(6.0)),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full()
            .padding_horiz(12.0)
            .padding_vert(5.0)
            .background(p.bg_deep)
            .border_bottom(1.0)
            .border_color(p.glass_border)
    });

    // ── No-git-repo warning banner ────────────────────────────────────────────
    let no_git_banner = container(
        h_stack((
            label(|| "  No git repo detected".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0)
                    .color(p.warning)
                    .font_weight(floem::text::Weight::BOLD)
            }),
            label(|| " \u{2014} changes cannot be undone with git".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0).color(p.warning)
            }),
        ))
        .style(|s| s.items_center()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        let show = !is_git_repo.get();
        s.width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .background(p.warning.with_alpha(0.10))
            .border_bottom(1.0)
            .border_color(p.warning.with_alpha(0.35))
            .apply_if(!show, |s| s.display(floem::style::Display::None))
    });

    // ── Non-git confirmation widget ───────────────────────────────────────────
    // Shown when Run is clicked in a workspace with no git repo.
    let git_confirm_widget = {
        let run = run_action.clone();
        container(
            v_stack((
                label(|| "Run without git?").style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(12.0)
                        .font_weight(floem::text::Weight::BOLD)
                        .color(p.warning)
                        .margin_bottom(4.0)
                }),
                label(|| "No git repo detected — changes cannot be undone with git restore.")
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(11.0).color(p.text_secondary).margin_bottom(8.0)
                    }),
                h_stack((
                    phaze_button("Run anyway", ButtonVariant::Primary, theme, move || {
                        needs_git_confirm.set(false);
                        run();
                    }),
                    phaze_button("Cancel", ButtonVariant::Secondary, theme, move || {
                        needs_git_confirm.set(false);
                    }),
                ))
                .style(|s| s.gap(8.0)),
            ))
            .style(|s| s.width_full()),
        )
        .style(move |s| {
            let p = theme.get().palette;
            s.width_full()
                .padding(12.0)
                .background(p.warning.with_alpha(0.10))
                .border_bottom(1.0)
                .border_color(p.warning.with_alpha(0.4))
                .apply_if(!needs_git_confirm.get(), |s| {
                    s.display(floem::style::Display::None)
                })
        })
    };

    // ── Task input ────────────────────────────────────────────────────────────
    let input_area = container(
        v_stack((
            label(|| "Describe your task:".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0).color(p.text_secondary).margin_bottom(6.0)
            }),
            text_input(task_input)
                .placeholder("e.g. Refactor auth module, add tests, fix the login bug...")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.width_full()
                        .padding(8.0)
                        .font_size(12.0)
                        .background(p.bg_surface)
                        .color(p.text_primary)
                        .border(1.0)
                        .border_color(p.glass_border)
                        .border_radius(4.0)
                }),
            h_stack((
                phaze_button("Run", ButtonVariant::Primary, theme, {
                    let run = run_action.clone();
                    move || {
                        if !is_git_repo.get() {
                            // Intercept: show confirmation widget instead of running directly.
                            needs_git_confirm.set(true);
                        } else {
                            run();
                        }
                    }
                }),
                phaze_button("Stop", ButtonVariant::Secondary, theme, stop_action),
            ))
            .style(|s| s.gap(8.0).margin_top(8.0)),
        ))
        .style(|s| s.width_full()),
    )
    .style(|s| s.padding(12.0).width_full());

    // ── Status indicator ──────────────────────────────────────────────────────
    let status_line = container(label(move || {
        if is_running.get() {
            "Agent is running...".to_string()
        } else if !event_log.get().is_empty() {
            let log = event_log.get();
            if let Some(last) = log.last() {
                last.text.clone()
            } else {
                "Ready".to_string()
            }
        } else {
            "Ready \u{2014} enter a task and click Run".to_string()
        }
    }))
    .style(move |s| {
        let p = theme.get().palette;
        let running = is_running.get();
        s.width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .font_size(11.0)
            .color(if running { p.accent } else { p.text_muted })
            .background(p.bg_surface)
            .border_bottom(1.0)
            .border_color(p.glass_border)
    });

    // ── Pending approval widget ───────────────────────────────────────────────
    // This block is visible only while the agent waits for an approval decision.
    let approval_widget = container(
        v_stack((
            // Description
            label(move || {
                if let Some(pa) = pending_approval.get() {
                    format!(
                        "Allow tool: {}   {}",
                        pa.tool_name,
                        extract_path_from_params(&pa.tool_name, &pa.params)
                            .map(|p| format!("({p})"))
                            .unwrap_or_default()
                    )
                } else {
                    String::new()
                }
            })
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0)
                    .color(p.warning)
                    .font_weight(floem::text::Weight::BOLD)
                    .margin_bottom(6.0)
            }),
            // Allow / Deny buttons
            h_stack((
                phaze_button("Allow", ButtonVariant::Primary, theme, move || {
                    answer_pending(ApprovalResponse::Approved)
                }),
                phaze_button("Deny", ButtonVariant::Danger, theme, move || {
                    answer_pending(ApprovalResponse::Denied)
                }),
            ))
            .style(|s| s.gap(8.0)),
        ))
        .style(|s| s.width_full()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        let visible = pending_approval.get().is_some();
        s.width_full()
            .padding(12.0)
            .background(p.warning.with_alpha(0.08))
            .border_bottom(1.0)
            .border_color(p.warning.with_alpha(0.4))
            .apply_if(!visible, |s| s.display(floem::style::Display::None))
    });

    // ── Event log ─────────────────────────────────────────────────────────────
    let event_log_view = scroll(
        dyn_stack(
            move || {
                let log = safe_get(event_log, Vec::new());
                (0..log.len()).collect::<Vec<_>>()
            },
            |idx| *idx,
            move |idx| {
                let log = event_log.get_untracked();
                let entry = log.get(idx).cloned().unwrap_or(EventLogEntry {
                    kind: EventKind::Text,
                    text: String::new(),
                    path: None,
                });
                let kind = entry.kind.clone();
                let text = entry.text.clone();
                let path_opt = entry.path.clone();

                // Two-part row: main label + optional path label.
                let path_label = container(label(move || path_opt.clone().unwrap_or_default()))
                    .style(move |s| {
                        let p = theme.get().palette;
                        let has_path = entry.path.is_some();
                        s.font_size(10.0)
                            .color(p.text_muted)
                            .font_family("monospace".to_string())
                            .margin_left(8.0)
                            .apply_if(!has_path, |s| s.display(floem::style::Display::None))
                    });

                container(
                    h_stack((
                        label(move || text.clone()).style(move |s| {
                            let p = theme.get().palette;
                            let color = match &kind {
                                EventKind::Thinking => p.text_muted,
                                EventKind::Text => p.text_primary,
                                EventKind::ToolStart => p.accent,
                                EventKind::ToolResult => p.success,
                                EventKind::ApprovalPending => p.warning,
                                EventKind::Done => p.success,
                                EventKind::Error => p.error,
                                EventKind::Diff => p.text_secondary,
                                EventKind::Warning => p.warning,
                            };
                            s.font_size(11.0)
                                .color(color)
                                .font_family("monospace".to_string())
                        }),
                        path_label,
                    ))
                    .style(|s| s.items_center().width_full()),
                )
                .style(move |s| s.width_full().padding_horiz(12.0).padding_vert(3.0))
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().flex_grow(1.0).min_height(0.0));

    // ── Agent response text ────────────────────────────────────────────────────
    let response_view = container(
        scroll(
            label(move || {
                let text = agent_text.get();
                if text.is_empty() {
                    String::new()
                } else {
                    text
                }
            })
            .style(move |s| {
                let p = theme.get().palette;
                s.width_full()
                    .padding(10.0)
                    .font_size(12.0)
                    .color(p.text_primary)
                    .font_family("monospace".to_string())
            }),
        )
        .style(|s| {
            s.width_full()
                .max_height(phazeai_core::constants::ui::MAX_DROPDOWN_HEIGHT)
        }),
    )
    .style(move |s| {
        let p = theme.get().palette;
        let has_text = !agent_text.get().is_empty();
        s.width_full()
            .border_top(1.0)
            .border_color(p.glass_border)
            .apply_if(!has_text, |s| s.display(floem::style::Display::None))
    });

    // ── Diff cards ─────────────────────────────────────────────────────────────
    let diff_section = scroll(
        dyn_stack(
            move || {
                let cards = safe_get(diff_cards, Vec::new());
                (0..cards.len()).collect::<Vec<_>>()
            },
            |idx| *idx,
            move |idx| {
                let cards = diff_cards.get_untracked();
                let card = cards.get(idx).cloned().unwrap_or(DiffCard {
                    file: String::new(),
                    diff: String::new(),
                });
                let file = card.file.clone();
                let diff = card.diff.clone();
                let expanded = create_rw_signal(true);

                let file_row = container(
                    h_stack((
                        label(move || {
                            if expanded.get() {
                                "v ".to_string()
                            } else {
                                "> ".to_string()
                            }
                        })
                        .style(move |s| {
                            let p = theme.get().palette;
                            s.font_size(10.0).color(p.text_muted).min_width(14.0)
                        }),
                        label(move || file.clone()).style(move |s| {
                            let p = theme.get().palette;
                            s.font_size(11.0)
                                .font_weight(floem::text::Weight::BOLD)
                                .color(p.accent)
                        }),
                    ))
                    .style(|s| s.items_center()),
                )
                .style(move |s| {
                    let p = theme.get().palette;
                    s.width_full()
                        .padding(8.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                        .background(p.bg_surface)
                        .border_bottom(1.0)
                        .border_color(p.glass_border)
                })
                .on_click_stop(move |_| {
                    expanded.update(|e| *e = !*e);
                });

                let diff_content = container(label(move || diff.clone()).style(move |s| {
                    let p = theme.get().palette;
                    s.width_full()
                        .padding(8.0)
                        .font_size(11.0)
                        .color(p.text_secondary)
                        .font_family("monospace".to_string())
                }))
                .style(move |s| {
                    let p = theme.get().palette;
                    s.width_full()
                        .background(p.bg_deep)
                        .apply_if(!expanded.get(), |s| s.display(floem::style::Display::None))
                });

                container(v_stack((file_row, diff_content)).style(|s| s.width_full()))
                    .style(move |s| s.width_full().margin_bottom(2.0))
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        let has_diffs = !diff_cards.get().is_empty();
        s.width_full()
            .max_height(phazeai_core::constants::ui::MAX_DIFF_HEIGHT)
            .apply_if(!has_diffs, |s| s.display(floem::style::Display::None))
    });

    let diff_header = container(label(|| "Changed Files".to_string())).style(move |s| {
        let p = theme.get().palette;
        let has_diffs = !diff_cards.get().is_empty();
        s.width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .font_size(11.0)
            .font_weight(floem::text::Weight::BOLD)
            .color(p.text_muted)
            .border_top(1.0)
            .border_color(p.glass_border)
            .apply_if(!has_diffs, |s| s.display(floem::style::Display::None))
    });

    // ── Assemble ──────────────────────────────────────────────────────────────
    container(
        v_stack((
            header,
            workspace_bar,
            no_git_banner,
            git_confirm_widget,
            input_area,
            status_line,
            approval_widget,
            event_log_view,
            response_view,
            diff_header,
            diff_section,
        ))
        .style(|s| s.flex_col().width_full().height_full()),
    )
    .style(move |s| {
        let t = theme.get();
        s.width_full()
            .height_full()
            .background(t.palette.bg_base)
            .color(t.palette.text_primary)
            .font_size(13.0)
    })
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Extract a file path from tool parameters for prominent display.
fn extract_path_from_params(tool_name: &str, params: &Value) -> Option<String> {
    match tool_name {
        "read_file" | "write_file" | "edit_file" | "delete_path" | "copy_path" | "move_path"
        | "open" => params
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        "bash" => params.get("command").and_then(|v| v.as_str()).map(|cmd| {
            // Show a truncated version of the command as the "path".
            let trimmed = cmd.trim();
            if trimmed.len() > 80 {
                format!("{}...", &trimmed[..80])
            } else {
                trimmed.to_string()
            }
        }),
        "grep" => params
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        "glob" | "list_files" | "find_path" => params
            .get("path")
            .or_else(|| params.get("pattern"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

/// Build a concise, readable one-line description of a tool invocation.
fn format_tool_display(tool_name: &str, params: &Value) -> String {
    match tool_name {
        "read_file" => {
            let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("read_file  {}", path)
        }
        "write_file" => {
            let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("write_file  {}", path)
        }
        "edit_file" => {
            let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("edit_file  {}", path)
        }
        "bash" => {
            let cmd = params
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let short = if cmd.len() > 60 {
                format!("{}...", &cmd[..60])
            } else {
                cmd.to_string()
            };
            format!("bash  {}", short)
        }
        "grep" => {
            let pat = params
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("grep  {}", pat)
        }
        "glob" => {
            let pat = params
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("glob  {}", pat)
        }
        "list_files" => {
            let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            format!("list_files  {}", path)
        }
        _ => tool_name.to_string(),
    }
}

fn parse_diff_cards(diff_text: &str) -> Vec<DiffCard> {
    let mut cards = Vec::new();
    let mut current_file = String::new();
    let mut current_diff = String::new();

    for line in diff_text.lines() {
        if line.starts_with("diff --git") {
            if !current_file.is_empty() {
                cards.push(DiffCard {
                    file: current_file.clone(),
                    diff: current_diff.clone(),
                });
            }
            // Extract filename: "diff --git a/foo b/foo" → "foo"
            current_file = line.split(" b/").nth(1).unwrap_or("unknown").to_string();
            current_diff = String::new();
        } else if !current_file.is_empty() {
            current_diff.push_str(line);
            current_diff.push('\n');
        }
    }

    if !current_file.is_empty() {
        cards.push(DiffCard {
            file: current_file,
            diff: current_diff,
        });
    }

    cards
}

#[cfg(test)]
mod tests {
    use super::ComposerApprovalMode::{ApproveAll, ApproveDestructive, AutoAll};
    use super::{approval_fn, ApprovalResponse, ComposerUpdate, PendingApproval};
    use serde_json::json;
    use std::sync::mpsc::{sync_channel, Receiver};
    use std::time::Duration;

    /// The next prompt the panel would show, as its update effect builds it.
    #[track_caller]
    fn next_prompt(shown: &Receiver<ComposerUpdate>) -> PendingApproval {
        match shown.recv_timeout(Duration::from_secs(10)) {
            Ok(ComposerUpdate::ToolApprovalRequest {
                name,
                params,
                reply,
            }) => PendingApproval {
                tool_name: name,
                params,
                reply,
            },
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_click_answers_only_the_call_its_prompt_shows() {
        let (updates, shown) = sync_channel(16);
        let approve = approval_fn(ApproveDestructive, updates);

        // A read-only call runs unasked and shows no prompt to click.
        assert!(approve("read_file".into(), json!({"path": "src/main.rs"})).await);
        assert!(
            shown.try_recv().is_err(),
            "prompt shown for a call that ran"
        );

        // A write waits for the answer to its own prompt.
        let write = json!({"path": "src/main.rs", "content": "x"});
        let write = tokio::spawn(approve("write_file".into(), write));
        let write_prompt = next_prompt(&shown);
        assert_eq!(write_prompt.tool_name, "write_file");
        write_prompt.answer(ApprovalResponse::Denied);
        assert!(!write.await.unwrap(), "Deny let the write run");

        // Allow clicked on that prompt afterwards (still on screen, or clicked
        // twice) must not answer the next call: that one waits for its own.
        write_prompt.answer(ApprovalResponse::Approved);
        let remove = json!({"command": "rm -rf target"});
        let remove = tokio::spawn(approve("bash".into(), remove));
        let remove_prompt = next_prompt(&shown);
        assert_eq!(remove_prompt.tool_name, "bash");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !remove.is_finished(),
            "a click on the write_file prompt answered the rm call"
        );
        remove_prompt.answer(ApprovalResponse::Approved);
        assert!(remove.await.unwrap());

        // "Approve all" puts the read-only call to the user as well.
        let (updates, shown) = sync_channel(16);
        let approve = approval_fn(ApproveAll, updates);
        let read = tokio::spawn(approve("read_file".into(), json!({"path": "src/main.rs"})));
        next_prompt(&shown).answer(ApprovalResponse::Approved);
        assert!(read.await.unwrap());
    }

    #[test]
    fn each_approval_mode_asks_for_what_its_label_says() {
        let read = ("read_file", json!({"path": "src/main.rs"}));
        let search = ("code_search", json!({"query": "parse config"}));
        let list = ("bash", json!({"command": "ls"}));
        let write = ("write_file", json!({"path": "src/main.rs", "content": "x"}));
        let remove = ("bash", json!({"command": "rm -rf target"}));
        let mcp = ("mcp__files__read_text_file", json!({"path": "notes.md"}));
        let calls = [&read, &search, &list, &write, &remove, &mcp];

        // "Approve all": every call, read-only ones included.
        for (tool, params) in calls {
            assert!(
                ApproveAll.needs_approval(tool, params),
                "Approve all ran {tool} {params} unasked"
            );
        }

        // "Approve destructive": read-only calls run unasked, the rest wait.
        for (tool, params) in [&read, &search, &list] {
            assert!(
                !ApproveDestructive.needs_approval(tool, params),
                "{tool} {params}"
            );
        }
        for (tool, params) in [&write, &remove, &mcp] {
            assert!(
                ApproveDestructive.needs_approval(tool, params),
                "{tool} {params}"
            );
        }

        // "Auto-approve all": never asks.
        for (tool, params) in calls {
            assert!(!AutoAll.needs_approval(tool, params), "{tool} {params}");
        }
    }
}
