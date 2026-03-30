use std::rc::Rc;
use std::sync::Arc;

use floem::{
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, Modifiers},
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::{
    Agent, AgentEvent, ConversationMetadata, ConversationStore, SavedConversation, SavedMessage,
    Settings,
};

use crate::{
    components::icon::{icons, phaze_icon},
    theme::PhazeTheme,
    util::safe_get,
};

// ── AI Mode ───────────────────────────────────────────────────────────────────

/// Selects the conversational role / system-prompt variant used when sending
/// a message to the AI. Previously this lived in the now-removed `ai_panel`;
/// it has been merged here so there is a single AI surface in the IDE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiMode {
    Chat,
    Ask,
    Debug,
    Plan,
    Edit,
}

impl AiMode {
    pub fn label(self) -> &'static str {
        match self {
            AiMode::Chat => "Chat",
            AiMode::Ask => "Ask",
            AiMode::Debug => "Debug",
            AiMode::Plan => "Plan",
            AiMode::Edit => "Edit",
        }
    }

    /// Returns a brief system-prompt prefix injected before the user message.
    /// Empty string for the default Chat mode so no prefix is added.
    pub fn system_hint(self) -> &'static str {
        match self {
            AiMode::Chat => "",
            AiMode::Ask => "Answer concisely and precisely. No extra prose.\n\n",
            AiMode::Debug => "You are a debugging expert. Focus on root causes and fixes.\n\n",
            AiMode::Plan => "You are a software architect. Produce clear step-by-step plans.\n\n",
            AiMode::Edit => "You are a code editor. Produce only code changes, no commentary.\n\n",
        }
    }
}

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
    pub is_error: bool,
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
    /// An error occurred.
    Err(String),
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn now_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // RFC3339-ish: YYYY-MM-DDTHH:MM:SSZ
    let secs = now;
    let days = secs / 86400;
    let rem = secs % 86400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;
    let s = rem % 60;
    let a = days.saturating_sub(32044);
    let b = (4 * a + 3) / 146097;
    let c = a - (146097 * b) / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - (1461 * d) / 4;
    let m = (5 * e + 2) / 153;
    let day = (e - (153 * m + 2) / 5) + 1;
    let month = (m + 3) % 12 + 1;
    let year = 2000 + b * 100 + d - 4800 + (m >= 10) as u64;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, mins, s
    )
}

fn save_conversation(
    messages: &[ChatMessage],
    conversation_id: &str,
    model_name: &str,
    workspace_root: &std::path::Path,
) {
    let store = ConversationStore::new().unwrap_or_else(|_| ConversationStore::default());

    let saved_messages: Vec<SavedMessage> = messages
        .iter()
        .map(|m| SavedMessage {
            role: match m.role {
                ChatRole::User => "user".into(),
                ChatRole::Assistant => "assistant".into(),
                ChatRole::Tool => "tool".into(),
            },
            content: m.content.clone(),
            timestamp: now_str(),
            tool_name: None,
        })
        .collect();

    let title = messages
        .iter()
        .find_map(|m| {
            if m.role == ChatRole::User {
                let t = m.content.chars().take(80).collect::<String>();
                Some(if m.content.len() > 80 {
                    format!("{}...", t)
                } else {
                    t
                })
            } else {
                None
            }
        })
        .unwrap_or_else(|| "Untitled".into());

    let cwd = Some(workspace_root.display().to_string());

    let metadata = ConversationMetadata {
        id: conversation_id.to_string(),
        title,
        created_at: now_str(),
        updated_at: now_str(),
        message_count: saved_messages.len(),
        model: model_name.to_string(),
        project_dir: cwd,
    };

    let conversation = SavedConversation {
        metadata,
        messages: saved_messages,
        system_prompt: None,
    };

    let _ = store.save(&conversation);
}

fn send_to_ai(
    user_message: String,
    settings: Settings,
    workspace_root: std::path::PathBuf,
    mode_hint: &'static str,
    update_tx: std::sync::mpsc::SyncSender<ChatUpdate>,
    cancel_token: Arc<std::sync::atomic::AtomicBool>,
) {
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
            let client = match settings.build_llm_client() {
                Ok(c) => c,
                Err(e) => {
                    let _ = update_tx.send(ChatUpdate::Err(format!("LLM init error: {e}")));
                    return;
                }
            };
            let mut agent = Agent::new(client).with_cancel_token(cancel_token);

            // Connect to MCP servers
            let mcp_configs = phazeai_core::mcp::McpManager::load_config(&workspace_root);
            if !mcp_configs.is_empty() {
                let mut mcp_manager = phazeai_core::mcp::McpManager::new();
                mcp_manager.connect_all(&mcp_configs);
                agent.register_mcp_tools(std::sync::Arc::new(std::sync::Mutex::new(mcp_manager)));
            }

            let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

            // Prepend the mode system hint (empty for default Chat mode).
            let full_prompt = if mode_hint.is_empty() {
                user_message.clone()
            } else {
                format!("{}{}", mode_hint, user_message)
            };
            let run_fut = agent.run_with_events(&full_prompt, agent_tx);
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
                            let _ = update_tx.send(ChatUpdate::Done(accumulated.clone()));
                            break;
                        }
                        AgentEvent::Error(e) => {
                            let _ = update_tx.send(ChatUpdate::Err(e));
                            break;
                        }
                        _ => {}
                    }
                }
            };

            let _ = tokio::join!(run_fut, drain_fut);
        });
    });
}

// ── Chat Panel ────────────────────────────────────────────────────────────────

/// Full AI chat panel with real streaming responses and neon-glass aesthetics.
///
/// `ai_thinking` — shared signal from `IdeState`; set to `true` while the AI
/// is generating a response so the sentient gutter glows.
///
/// Settings are re-loaded from disk on each send so model/provider changes in
/// the settings panel take effect immediately without restarting.
/// Expand `@filename` mentions in a chat message into file context blocks.
///
/// Scans for `@path/to/file` tokens, resolves each relative to `root`,
/// reads file contents, and prepends them as context. Returns the expanded prompt.
fn expand_file_mentions(message: &str, root: &std::path::Path) -> String {
    let re = regex::Regex::new(r"@([\w./\-]+\.\w+)").unwrap();
    let mut context_blocks = Vec::new();
    let mut clean_msg = message.to_string();

    for cap in re.captures_iter(message) {
        let mention = &cap[1];
        let file_path = root.join(mention);
        if file_path.is_file() {
            if let Ok(contents) = std::fs::read_to_string(&file_path) {
                // Truncate very large files
                let truncated = if contents.len() > 30_000 {
                    let end = contents.floor_char_boundary(30_000);
                    format!(
                        "{}...\n[truncated — {} bytes total]",
                        &contents[..end],
                        contents.len()
                    )
                } else {
                    contents
                };
                context_blocks.push(format!(
                    "<file path=\"{}\">\n{}\n</file>",
                    mention, truncated
                ));
            }
            // Remove the @mention from the visible message
            clean_msg = clean_msg.replace(&format!("@{mention}"), &format!("`{mention}`"));
        }
    }

    if context_blocks.is_empty() {
        message.to_string()
    } else {
        format!(
            "I'm providing the following file(s) as context:\n\n{}\n\nUser request: {}",
            context_blocks.join("\n\n"),
            clean_msg
        )
    }
}

pub fn chat_panel(
    theme: RwSignal<PhazeTheme>,
    ai_thinking: RwSignal<bool>,
    chat_inject: RwSignal<Option<String>>,
    workspace_root: RwSignal<std::path::PathBuf>,
) -> impl IntoView {
    let mut initial_messages = vec![ChatMessage {
        role: ChatRole::Assistant,
        content: "Welcome to PhazeAI. How can I help you?".to_string(),
        loading: false,
        is_error: false,
    }];
    let mut initial_id = ConversationStore::generate_id();

    if let Ok(store) = ConversationStore::new() {
        if let Ok(recent) = store.list_recent(1) {
            if let Some(meta) = recent.first() {
                if let Ok(conv) = store.load(&meta.id) {
                    initial_id = meta.id.clone();
                    initial_messages.clear();
                    for m in conv.messages {
                        #[allow(clippy::wildcard_in_or_patterns)]
                        let role = match m.role.as_str() {
                            "user" => ChatRole::User,
                            "assistant" => ChatRole::Assistant,
                            "tool" | "system" | _ => ChatRole::Tool,
                        };
                        initial_messages.push(ChatMessage {
                            role,
                            content: m.content,
                            loading: false,
                            is_error: false,
                        });
                    }
                }
            }
        }
    }

    let conversation_id = create_rw_signal(initial_id);
    let messages: RwSignal<Vec<ChatMessage>> = create_rw_signal(initial_messages);
    let input_text = create_rw_signal(String::new());
    let is_loading = create_rw_signal(false);
    let mode = create_rw_signal(AiMode::Chat);
    let current_cancel_token: RwSignal<Option<Arc<std::sync::atomic::AtomicBool>>> =
        create_rw_signal(None);

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
                            content: format!("Running tool: {}...", name),
                            loading: true,
                            is_error: false,
                        });
                    });
                }
                ChatUpdate::ToolResult { name, summary } => {
                    messages.update(|list| {
                        if let Some(last) = list.last_mut() {
                            if last.role == ChatRole::Tool && last.loading {
                                last.content = format!("{}: {}", name, summary);
                                last.loading = false;
                            }
                        }
                    });
                    let msgs = messages.get_untracked();
                    save_conversation(
                        &msgs,
                        &conversation_id.get_untracked(),
                        &Settings::load().llm.model,
                        &workspace_root.get_untracked(),
                    );
                }
                ChatUpdate::Done(text) => {
                    messages.update(|list| {
                        // Ensure we finalize any "hanging" assistant message
                        if let Some(last) = list.last_mut() {
                            if last.role == ChatRole::Assistant && last.loading {
                                last.content = if text.is_empty() {
                                    "(no response)".to_string()
                                } else {
                                    text
                                };
                                last.loading = false;
                            }
                        }
                    });
                    is_loading.set(false);
                    ai_thinking.set(false);
                    let msgs = messages.get_untracked();
                    save_conversation(
                        &msgs,
                        &conversation_id.get_untracked(),
                        &Settings::load().llm.model,
                        &workspace_root.get_untracked(),
                    );
                }
                ChatUpdate::Err(e) => {
                    messages.update(|list| {
                        if let Some(last) = list.last_mut() {
                            if last.loading {
                                last.content = format!("Error: {}", e);
                                last.loading = false;
                                last.is_error = true;
                            } else {
                                list.push(ChatMessage {
                                    role: ChatRole::Assistant,
                                    content: format!("Error: {}", e),
                                    loading: false,
                                    is_error: true,
                                });
                            }
                        }
                    });
                    is_loading.set(false);
                    ai_thinking.set(false);
                    let msgs = messages.get_untracked();
                    save_conversation(
                        &msgs,
                        &conversation_id.get_untracked(),
                        &Settings::load().llm.model,
                        &workspace_root.get_untracked(),
                    );
                }
            }
        }
    });

    // ── Send closure ──────────────────────────────────────────────────────────

    let update_tx = Arc::new(update_tx);

    let do_send: Rc<dyn Fn()> = Rc::new({
        let update_tx = update_tx.clone();
        move || {
            let text = input_text.get();
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() || is_loading.get() {
                return;
            }

            // Expand @file mentions into context blocks before sending to AI
            let root = workspace_root.get_untracked();
            let prompt = expand_file_mentions(&trimmed, &root);

            messages.update(|list| {
                list.push(ChatMessage {
                    role: ChatRole::User,
                    content: trimmed.clone(),
                    loading: false,
                    is_error: false,
                });
                list.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: String::new(),
                    loading: true,
                    is_error: false,
                });
            });
            input_text.set(String::new());
            is_loading.set(true);
            ai_thinking.set(true);

            let token = Arc::new(std::sync::atomic::AtomicBool::new(false));
            current_cancel_token.set(Some(token.clone()));

            // Re-read settings on every send so model/provider changes in the
            // settings panel take effect immediately (no restart needed).
            let live_settings = Settings::load();
            let hint = mode.get_untracked().system_hint();
            send_to_ai(
                prompt,
                live_settings,
                root,
                hint,
                (*update_tx).clone(),
                token,
            );
        }
    });

    // ── Inject from context menu (Explain Selection / Generate Tests / Fix) ──
    {
        let do_send = do_send.clone();
        create_effect(move |_| {
            if let Some(text) = chat_inject.get() {
                input_text.set(text);
                chat_inject.set(None);
                do_send();
            }
        });
    }

    // ── Header — neon strip + title ───────────────────────────────────────────

    // 2px accent-colored top strip (the "neon line" on top of the panel)
    let neon_strip = container(label(|| "")).style(move |s| {
        s.height(2.0)
            .width_full()
            .background(theme.get().palette.accent)
    });

    let header_content = container(
        stack((
            phaze_icon(icons::AI, 14.0, move |p| p.accent, theme),
            label(|| "  PHAZEAI").style(move |s| {
                s.font_size(11.0)
                    .color(theme.get().palette.accent)
                    .font_weight(floem::text::Weight::BOLD)
            }),
        ))
        .style(|s| s.items_center()),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(14.0)
            .padding_vert(10.0)
            .border_bottom(1.0)
            .border_color(p.glass_border)
            .width_full()
            .background(p.glass_bg)
    });

    let header = stack((neon_strip, header_content)).style(|s| s.flex_col().width_full());

    // ── Mode tabs (Chat / Ask / Debug / Plan / Edit) ──────────────────────────

    let all_modes = [
        AiMode::Chat,
        AiMode::Ask,
        AiMode::Debug,
        AiMode::Plan,
        AiMode::Edit,
    ];

    let mode_tab = |m: AiMode| {
        let is_hov = create_rw_signal(false);
        container(label(move || m.label()))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let active = mode.get() == m;
                s.padding_horiz(9.0)
                    .padding_vert(4.0)
                    .font_size(11.0)
                    .color(if active { p.accent } else { p.text_muted })
                    .background(if active {
                        p.accent_dim
                    } else if is_hov.get() {
                        p.bg_elevated
                    } else {
                        floem::peniko::Color::TRANSPARENT
                    })
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .apply_if(active, |s| s.border_bottom(2.0).border_color(p.accent))
            })
            .on_click_stop(move |_| {
                mode.set(m);
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                is_hov.set(true);
            })
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
                is_hov.set(false);
            })
    };

    let mode_tabs = stack((
        mode_tab(all_modes[0]),
        mode_tab(all_modes[1]),
        mode_tab(all_modes[2]),
        mode_tab(all_modes[3]),
        mode_tab(all_modes[4]),
    ))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.width_full()
            .background(p.glass_bg)
            .border_bottom(1.0)
            .border_color(p.glass_border)
            .items_center()
            .padding_horiz(4.0)
            .padding_vert(4.0)
    });

    let do_retry: Rc<dyn Fn()> = Rc::new({
        let update_tx = update_tx.clone();
        move || {
            if is_loading.get() {
                return;
            }

            let msgs = messages.get_untracked();
            let mut last_user_msg = None;
            for msg in msgs.iter().rev() {
                if msg.role == ChatRole::User {
                    last_user_msg = Some(msg.content.clone());
                    break;
                }
            }

            if let Some(user_msg) = last_user_msg {
                messages.update(|list| {
                    while let Some(last) = list.last() {
                        if last.role != ChatRole::User {
                            list.pop();
                        } else {
                            break;
                        }
                    }
                    list.push(ChatMessage {
                        role: ChatRole::Assistant,
                        content: String::new(),
                        loading: true,
                        is_error: false,
                    });
                });

                is_loading.set(true);
                ai_thinking.set(true);

                let token = Arc::new(std::sync::atomic::AtomicBool::new(false));
                current_cancel_token.set(Some(token.clone()));

                let root = workspace_root.get_untracked();
                let prompt = expand_file_mentions(&user_msg, &root);
                let live_settings = Settings::load();
                let hint = mode.get_untracked().system_hint();
                send_to_ai(
                    prompt,
                    live_settings,
                    root,
                    hint,
                    (*update_tx).clone(),
                    token,
                );
            }
        }
    });

    // ── Message bubbles ───────────────────────────────────────────────────────

    let msg_list = dyn_stack(
        move || {
            let list = safe_get(messages, Vec::new());
            let len = list.len();
            list.into_iter()
                .enumerate()
                .map(|(i, msg)| (i, msg, i == len - 1))
                .collect::<Vec<_>>()
        },
        |(i, _, _)| *i,
        move |(_, msg, is_last)| {
            let is_user = msg.role == ChatRole::User;
            let content = msg.content.clone();
            let loading = msg.loading;
            let is_error = msg.is_error;

            let text_content = if loading && content.is_empty() {
                "●●●".to_string()
            } else {
                content
            };
            let is_typing = loading && text_content.starts_with('●');
            let is_tool = msg.role == ChatRole::Tool;
            let show_retry = !is_user && is_last && !is_loading.get();
            let do_retry_btn = do_retry.clone();

            let retry_btn = container(phaze_icon(
                icons::REFRESH,
                12.0,
                move |p| p.text_secondary,
                theme,
            ))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding(4.0)
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(p.bg_elevated))
                    .apply_if(!show_retry, |s| s.display(floem::style::Display::None))
            })
            .on_click_stop(move |_| {
                (do_retry_btn)();
            });

            container(
                stack((
                    stack((
                        phaze_icon(icons::CHIP, 11.0, move |p| p.accent, theme).style(
                            move |s: floem::style::Style| {
                                s.apply_if(!is_tool, |s| s.display(floem::style::Display::None))
                            },
                        ),
                        label(move || text_content.clone()).style(move |s| {
                            let t = theme.get();
                            let p = &t.palette;
                            s.font_size(if is_tool { 11.0 } else { 13.0 })
                                .color(if is_user {
                                    p.text_primary
                                } else if is_error {
                                    p.error
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
                    .style(|s| s.items_center().flex_grow(1.0)),
                    retry_btn,
                ))
                .style(|s| s.items_center().justify_between().width_full()),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                if is_user {
                    // User bubble: accent tinted glass
                    s.width_full()
                        .padding_horiz(14.0)
                        .padding_vert(10.0)
                        .background(p.accent_dim)
                        .border(1.0)
                        .border_color(p.glass_border)
                        .border_radius(12.0)
                        .margin_bottom(8.0)
                        // Subtle inner glow
                        .box_shadow_blur(12.0)
                        .box_shadow_color(p.glow)
                        .box_shadow_spread(0.0)
                        .box_shadow_h_offset(0.0)
                        .box_shadow_v_offset(0.0)
                } else if is_tool {
                    // Tool card: specialized micro-bubble
                    s.width_full()
                        .padding_horiz(10.0)
                        .padding_vert(6.0)
                        .background(p.bg_deep.with_alpha(0.6))
                        .border(1.0)
                        .border_color(p.glass_border)
                        .border_radius(6.0)
                        .margin_bottom(6.0)
                        .margin_horiz(20.0) // Indent tool calls
                } else if is_error {
                    s.width_full()
                        .padding_horiz(14.0)
                        .padding_vert(10.0)
                        .background(p.error.with_alpha(0.1))
                        .border(1.0)
                        .border_color(p.error.with_alpha(0.3))
                        .border_radius(10.0)
                        .margin_bottom(8.0)
                } else {
                    // Assistant bubble: darker glass for better readability
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

    // ── Input bar ─────────────────────────────────────────────────────────────

    let do_send_btn = do_send.clone();
    let do_send_key = do_send.clone();
    let _ = do_send;

    let send_btn = container(
        stack((
            label(|| "↵").style(move |s| {
                s.font_size(14.0)
                    .color(theme.get().palette.bg_base)
                    .apply_if(is_loading.get(), |s| s.display(floem::style::Display::None))
            }),
            phaze_icon(icons::STOP, 14.0, move |p| p.text_primary, theme).style(move |s| {
                s.apply_if(!is_loading.get(), |s| {
                    s.display(floem::style::Display::None)
                })
            }),
        ))
        .style(|s| s.items_center().justify_center()),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let loading = is_loading.get();
        s.width(32.0)
            .height(32.0)
            .background(if loading { p.bg_elevated } else { p.accent })
            .border_radius(8.0)
            .items_center()
            .justify_center()
            .cursor(floem::style::CursorStyle::Pointer)
            .margin_left(8.0)
            // Glow on the send button when active
            .apply_if(!loading, |s| {
                s.box_shadow_blur(10.0)
                    .box_shadow_color(p.glow)
                    .box_shadow_spread(0.0)
                    .box_shadow_h_offset(0.0)
                    .box_shadow_v_offset(0.0)
            })
    })
    .on_click_stop(move |_| {
        if is_loading.get() {
            if let Some(token) = current_cancel_token.get() {
                token.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        } else {
            (do_send_btn)();
        }
    });

    let input_widget = text_input(input_text)
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
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
        let t = theme.get();
        let p = &t.palette;
        s.padding(10.0)
            .border_top(1.0)
            .border_color(p.glass_border)
            .width_full()
            .background(p.glass_bg)
    });

    // ── Full panel ────────────────────────────────────────────────────────────

    stack((header, mode_tabs, messages_scroll, input_bar)).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.flex_col()
            .width(340.0)
            .height_full()
            .background(p.glass_bg)
            .border_left(1.0)
            .border_color(p.glass_border)
    })
}
