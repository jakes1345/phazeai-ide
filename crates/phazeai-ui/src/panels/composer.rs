use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, text_input, v_stack, Decorators},
    IntoView,
};
use phazeai_core::tools::{BashTool, ToolApprovalManager, ToolApprovalMode, ToolPermission, ToolRegistry};
use phazeai_core::{Agent, AgentEvent, Settings};
use serde_json::Value;

use crate::app::IdeState;
use crate::components::button::{phaze_button, ButtonVariant};
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
        let mgr = ToolApprovalManager::new(match self {
            Self::AutoAll => ToolApprovalMode::AutoApprove,
            // For ApproveDestructive we use AlwaysAsk but then only block on
            // non-read-only tools — the ToolApprovalManager::needs_approval
            // logic already does this for AlwaysAsk.
            Self::ApproveDestructive => ToolApprovalMode::AlwaysAsk,
            Self::ApproveAll => ToolApprovalMode::AlwaysAsk,
        });

        match self {
            Self::AutoAll => false,
            Self::ApproveDestructive => {
                // Only block Write, Execute, or Destructive — not ReadOnly.
                let perm = mgr.classify_tool(tool_name, params);
                perm != ToolPermission::ReadOnly
            }
            Self::ApproveAll => {
                // Block everything except genuinely read-only tools.
                mgr.needs_approval(tool_name, params)
            }
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
    /// Agent requesting approval for a tool.
    ToolApprovalRequest { name: String, params: Value },
    /// Agent run completed.
    Done { iterations: usize },
    /// Error occurred.
    Err(String),
    /// Git diff output after completion.
    DiffOutput(Vec<DiffCard>),
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
}

// Channel message used to deliver approval decision back to the agent thread.
#[derive(Clone, Debug)]
enum ApprovalResponse {
    Approved,
    Denied,
}

// ── Composer Panel ───────────────────────────────────────────────────────────

/// Multi-file AI agent panel. Runs the full Agent with all tools and streams
/// events. Includes workspace display, approval-mode toggle, tool visibility,
/// and a no-git-repo warning banner.
pub fn composer_panel(state: IdeState) -> impl IntoView {
    let theme = state.theme;
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
        let workspace = state.workspace_root;
        create_effect(move |_| {
            let ws = workspace.get();
            let has_git = std::path::Path::new(&ws).join(".git").exists()
                || std::process::Command::new("git")
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

    // Approval response channel — the UI sends back decisions here.
    let (approval_tx, approval_rx) =
        std::sync::mpsc::sync_channel::<ApprovalResponse>(4);
    let approval_tx = Arc::new(approval_tx);
    let approval_rx_arc = Arc::new(std::sync::Mutex::new(approval_rx));

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
                ComposerUpdate::ToolApprovalRequest { name, params } => {
                    let path = extract_path_from_params(&name, &params);
                    let display = format_tool_display(&name, &params);
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::ApprovalPending,
                            text: format!("Waiting for approval: {}", display),
                            path: path.clone(),
                        });
                    });
                    pending_approval.set(Some(PendingApproval {
                        tool_name: name,
                        params,
                    }));
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
                    state.ai_thinking.set(false);
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
                    state.ai_thinking.set(false);
                    cancel_token.set(None);
                }
                ComposerUpdate::DiffOutput(cards) => {
                    diff_cards.set(cards);
                }
            }
        }
    });

    // ── Run action ───────────────────────────────────────────────────────────

    let update_tx = Arc::new(update_tx);

    let run_action = {
        let update_tx = update_tx.clone();
        let approval_tx = approval_tx.clone();
        let approval_rx_arc = approval_rx_arc.clone();
        let workspace = state.workspace_root;
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
            state.ai_thinking.set(true);

            let token = Arc::new(AtomicBool::new(false));
            cancel_token.set(Some(token.clone()));

            let tx = (*update_tx).clone();
            let ws = workspace.get_untracked();
            let mode = approval_mode.get_untracked();
            // Clone the sync_channel sender for the approval callback.
            let _approval_tx_cb = (*approval_tx).clone();
            // Clone the approval_rx end — we move it into the thread.
            let approval_rx_arc = approval_rx_arc.clone();

            // Drain any stale responses in the approval channel before starting.
            // (Best-effort — ignore errors.)
            while approval_rx_arc.lock().unwrap().try_recv().is_ok() {}

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
                            let _ =
                                tx.send(ComposerUpdate::Err(format!("LLM init error: {e}")));
                            return;
                        }
                    };

                    // Build agent with all tools + workspace-aware bash
                    let mut tools = ToolRegistry::default();
                    tools.register(Box::new(BashTool::new(ws.clone())));
                    let mut agent = Agent::new(client)
                        .with_tools(tools)
                        .with_cancel_token(token);

                    // Connect MCP servers
                    let mcp_configs = phazeai_core::mcp::McpManager::load_config(&ws);
                    if !mcp_configs.is_empty() {
                        let mut mcp_manager = phazeai_core::mcp::McpManager::new();
                        mcp_manager.connect_all(&mcp_configs);
                        agent.register_mcp_tools(Arc::new(std::sync::Mutex::new(
                            mcp_manager,
                        )));
                    }

                    // Wire approval function when mode is not AutoAll.
                    if mode != ComposerApprovalMode::AutoAll {
                        let tx_appr = tx.clone();
                        let rx_arc = approval_rx_arc.clone();
                        agent = agent.with_approval(Box::new(
                            move |tool_name: String, params: Value| {
                                let tx_inner = tx_appr.clone();
                                let rx_inner = rx_arc.clone();
                                Box::pin(async move {
                                    if !mode.needs_approval(&tool_name, &params) {
                                        // Read-only or safe — auto-approve silently.
                                        return true;
                                    }
                                    // Send approval request to UI.
                                    let _ = tx_inner.send(
                                        ComposerUpdate::ToolApprovalRequest {
                                            name: tool_name.clone(),
                                            params: params.clone(),
                                        },
                                    );
                                    // Block this async task on the sync response channel.
                                    // Use spawn_blocking so we don't starve the runtime.
                                    let result: ApprovalResponse = tokio::task::spawn_blocking(move || {
                                        let lock = rx_inner.lock().unwrap();
                                        // Wait up to 5 minutes for user response.
                                        lock.recv_timeout(std::time::Duration::from_secs(300))
                                            .unwrap_or(ApprovalResponse::Denied)
                                    })
                                    .await
                                    .unwrap_or(ApprovalResponse::Denied);

                                    matches!(result, ApprovalResponse::Approved)
                                })
                            },
                        ));
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
                                    let _ = tx2.send(ComposerUpdate::TextDelta(
                                        accumulated.clone(),
                                    ));
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
                                AgentEvent::ToolApprovalRequest { name, params } => {
                                    let _ = tx2.send(ComposerUpdate::ToolApprovalRequest {
                                        name,
                                        params,
                                    });
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
                    if let Ok(output) = std::process::Command::new("git")
                        .args(["diff", "HEAD"])
                        .current_dir(&ws)
                        .output()
                    {
                        if output.status.success() {
                            let diff_text =
                                String::from_utf8_lossy(&output.stdout).to_string();
                            let cards = parse_diff_cards(&diff_text);
                            if !cards.is_empty() {
                                let _ = tx.send(ComposerUpdate::DiffOutput(cards));
                            }
                        }
                    }
                });
            });
        }
    };

    let stop_action = {
        let approval_tx = approval_tx.clone();
        move || {
            if let Some(token) = cancel_token.get_untracked() {
                token.store(true, Ordering::Relaxed);
            }
            // Unblock any pending approval so the agent thread can exit.
            let _ = approval_tx.send(ApprovalResponse::Denied);
            pending_approval.set(None);
        }
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
        .style(|s| s.width_full().items_center().gap(8.0).padding_horiz(12.0).padding_vert(8.0)),
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
                s.font_size(10.0)
                    .color(p.text_muted)
                    .min_width(30.0)
            }),
            label(move || {
                state.workspace_root.get().display().to_string()
            })
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(10.0)
                    .color(p.text_secondary)
                    .font_family("monospace".to_string())
            }),
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
                    move || run()
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
    let approval_tx_approve = approval_tx.clone();
    let approval_tx_deny = approval_tx.clone();

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
                    let _ = approval_tx_approve.send(ApprovalResponse::Approved);
                    pending_approval.set(None);
                }),
                phaze_button("Deny", ButtonVariant::Danger, theme, move || {
                    let _ = approval_tx_deny.send(ApprovalResponse::Denied);
                    pending_approval.set(None);
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
                let path_label = container(label(move || {
                    path_opt.clone().unwrap_or_default()
                }))
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
                .style(move |s| {
                    s.width_full().padding_horiz(12.0).padding_vert(3.0)
                })
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
                let expanded = create_rw_signal(false);

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
        "read_file" | "write_file" | "edit_file" | "delete_path" | "copy_path"
        | "move_path" | "open" => params
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        "bash" => params
            .get("command")
            .and_then(|v| v.as_str())
            .map(|cmd| {
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
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("read_file  {}", path)
        }
        "write_file" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("write_file  {}", path)
        }
        "edit_file" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
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
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
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
