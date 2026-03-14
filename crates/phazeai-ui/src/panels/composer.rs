use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, text_input, v_stack, Decorators},
    IntoView,
};
use phazeai_core::{Agent, AgentEvent, Settings};
use phazeai_core::tools::{BashTool, ToolRegistry};

use crate::app::IdeState;
use crate::components::button::{phaze_button, ButtonVariant};

// ── Composer Event Types ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum ComposerUpdate {
    /// Agent is thinking (new iteration).
    Thinking(usize),
    /// Partial streamed text from agent.
    TextDelta(String),
    /// Tool execution started.
    ToolStart { name: String },
    /// Tool execution finished.
    ToolResult { name: String, success: bool, summary: String },
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
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum EventKind {
    Thinking,
    Text,
    ToolStart,
    ToolResult,
    Done,
    Error,
    Diff,
}

// ── Composer Panel ───────────────────────────────────────────────────────────

/// Multi-file AI agent panel. Runs the full Agent with all tools,
/// auto-approves everything, streams events, and shows git diff cards after run.
pub fn composer_panel(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    let task_input = create_rw_signal(String::new());
    let is_running = create_rw_signal(false);
    let event_log: RwSignal<Vec<EventLogEntry>> = create_rw_signal(Vec::new());
    let diff_cards: RwSignal<Vec<DiffCard>> = create_rw_signal(Vec::new());
    let agent_text = create_rw_signal(String::new());
    let cancel_token: RwSignal<Option<Arc<AtomicBool>>> = create_rw_signal(None);

    // Single channel pair for all updates — created once
    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<ComposerUpdate>(512);
    let update_signal = create_signal_from_channel(update_rx);

    // Process incoming updates
    create_effect(move |_| {
        if let Some(update) = update_signal.get() {
            match update {
                ComposerUpdate::Thinking(iter) => {
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::Thinking,
                            text: format!("Iteration {}", iter),
                        });
                    });
                }
                ComposerUpdate::TextDelta(text) => {
                    agent_text.set(text);
                }
                ComposerUpdate::ToolStart { name } => {
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::ToolStart,
                            text: format!("Running: {}", name),
                        });
                    });
                }
                ComposerUpdate::ToolResult { name, success, summary } => {
                    let icon = if success { "+" } else { "x" };
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::ToolResult,
                            text: format!("[{}] {}: {}", icon, name, summary),
                        });
                    });
                }
                ComposerUpdate::Done { iterations } => {
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::Done,
                            text: format!("Completed in {} iterations", iterations),
                        });
                    });
                    is_running.set(false);
                    state.ai_thinking.set(false);
                    cancel_token.set(None);
                }
                ComposerUpdate::Err(e) => {
                    event_log.update(|log| {
                        log.push(EventLogEntry {
                            kind: EventKind::Error,
                            text: format!("Error: {}", e),
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
            }]);
            diff_cards.set(Vec::new());
            agent_text.set(String::new());
            is_running.set(true);
            state.ai_thinking.set(true);

            let token = Arc::new(AtomicBool::new(false));
            cancel_token.set(Some(token.clone()));

            let tx = (*update_tx).clone();
            let ws = workspace.get_untracked();

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
                    let agent = Agent::new(client)
                        .with_tools(tools)
                        .with_cancel_token(token);

                    // Connect MCP servers
                    let mcp_configs = phazeai_core::mcp::McpManager::load_config(&ws);
                    if !mcp_configs.is_empty() {
                        let mut mcp_manager = phazeai_core::mcp::McpManager::new();
                        mcp_manager.connect_all(&mcp_configs);
                        // MCP tools registered via agent if needed
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
                                    let _ = tx2.send(ComposerUpdate::ToolStart { name });
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
    };

    let stop_action = move || {
        if let Some(token) = cancel_token.get_untracked() {
            token.store(true, Ordering::Relaxed);
        }
    };

    // ── UI ───────────────────────────────────────────────────────────────────

    // Header
    let header = container(
        label(|| "COMPOSER".to_string()).style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .font_weight(floem::text::Weight::BOLD)
                .color(p.text_muted)
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

    // Task input
    let input_area = container(
        v_stack((
            label(|| "Describe your task:".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0)
                    .color(p.text_secondary)
                    .margin_bottom(6.0)
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
                phaze_button(
                    "Run",
                    ButtonVariant::Primary,
                    theme,
                    {
                        let run = run_action.clone();
                        move || run()
                    },
                ),
                phaze_button(
                    "Stop",
                    ButtonVariant::Secondary,
                    theme,
                    move || stop_action(),
                ),
            ))
            .style(|s| s.gap(8.0).margin_top(8.0)),
        ))
        .style(|s| s.width_full()),
    )
    .style(|s| s.padding(12.0).width_full());

    // Status indicator
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
            "Ready — enter a task and click Run".to_string()
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

    // Event log
    let event_log_view = scroll(
        dyn_stack(
            move || {
                let log = event_log.get();
                (0..log.len()).collect::<Vec<_>>()
            },
            |idx| *idx,
            move |idx| {
                let log = event_log.get_untracked();
                let entry = log.get(idx).cloned().unwrap_or(EventLogEntry {
                    kind: EventKind::Text,
                    text: String::new(),
                });
                let kind = entry.kind.clone();
                let text = entry.text.clone();
                container(label(move || text.clone())).style(move |s| {
                    let p = theme.get().palette;
                    let color = match &kind {
                        EventKind::Thinking => p.text_muted,
                        EventKind::Text => p.text_primary,
                        EventKind::ToolStart => p.accent,
                        EventKind::ToolResult => floem::peniko::Color::from_rgb8(100, 200, 120),
                        EventKind::Done => floem::peniko::Color::from_rgb8(100, 200, 120),
                        EventKind::Error => floem::peniko::Color::from_rgb8(255, 100, 100),
                        EventKind::Diff => p.text_secondary,
                    };
                    s.width_full()
                        .padding_horiz(12.0)
                        .padding_vert(3.0)
                        .font_size(11.0)
                        .color(color)
                        .font_family("monospace".to_string())
                })
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().flex_grow(1.0).min_height(100.0));

    // Agent response text
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
        .style(|s| s.width_full().max_height(200.0)),
    )
    .style(move |s| {
        let p = theme.get().palette;
        let has_text = !agent_text.get().is_empty();
        s.width_full()
            .border_top(1.0)
            .border_color(p.glass_border)
            .apply_if(!has_text, |s| s.display(floem::style::Display::None))
    });

    // Diff cards
    let diff_section = scroll(
        dyn_stack(
            move || {
                let cards = diff_cards.get();
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

                let diff_content = container(
                    label(move || diff.clone()).style(move |s| {
                        let p = theme.get().palette;
                        s.width_full()
                            .padding(8.0)
                            .font_size(11.0)
                            .color(p.text_secondary)
                            .font_family("monospace".to_string())
                    }),
                )
                .style(move |s| {
                    let p = theme.get().palette;
                    s.width_full()
                        .background(p.bg_deep)
                        .apply_if(!expanded.get(), |s| {
                            s.display(floem::style::Display::None)
                        })
                });

                container(v_stack((file_row, diff_content)).style(|s| s.width_full())).style(
                    move |s| {
                        s.width_full().margin_bottom(2.0)
                    },
                )
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        let has_diffs = !diff_cards.get().is_empty();
        s.width_full()
            .max_height(300.0)
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

    // Assemble
    container(
        v_stack((
            header,
            input_area,
            status_line,
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
            current_file = line
                .split(" b/")
                .nth(1)
                .unwrap_or("unknown")
                .to_string();
            current_diff = String::new();
        } else {
            if !current_file.is_empty() {
                current_diff.push_str(line);
                current_diff.push('\n');
            }
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
