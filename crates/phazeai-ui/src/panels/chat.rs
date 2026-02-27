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
use phazeai_core::{Agent, AgentEvent, Settings};

use crate::{components::icon::{icons, phaze_icon}, theme::PhazeTheme};

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
    /// An error occurred.
    Err(String),
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn send_to_ai(
    user_message: String,
    settings: Settings,
    update_tx: std::sync::mpsc::SyncSender<ChatUpdate>,
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
            let agent = Agent::new(client);
            let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();

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
pub fn chat_panel(theme: RwSignal<PhazeTheme>, ai_thinking: RwSignal<bool>) -> impl IntoView {

    let messages: RwSignal<Vec<ChatMessage>> = create_rw_signal(vec![ChatMessage {
        role: ChatRole::Assistant,
        content: "Welcome to PhazeAI. How can I help you?".to_string(),
        loading: false,
    }]);
    let input_text = create_rw_signal(String::new());
    let is_loading = create_rw_signal(false);

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
                }
                ChatUpdate::Err(e) => {
                    messages.update(|list| {
                        if let Some(last) = list.last_mut() {
                            if last.loading {
                                last.content = format!("Error: {}", e);
                                last.loading = false;
                            }
                        }
                    });
                    is_loading.set(false);
                    ai_thinking.set(false);
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
            messages.update(|list| {
                list.push(ChatMessage {
                    role: ChatRole::User,
                    content: trimmed.clone(),
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
            // Re-read settings on every send so model/provider changes in the
            // settings panel take effect immediately (no restart needed).
            let live_settings = Settings::load();
            send_to_ai(trimmed, live_settings, (*update_tx).clone());
        }
    });

    // ── Header — neon strip + title ───────────────────────────────────────────

    // 2px accent-colored top strip (the "neon line" on top of the panel)
    let neon_strip = container(label(|| ""))
        .style(move |s| {
            s.height(2.0)
             .width_full()
             .background(theme.get().palette.accent)
        });

    let header_content = container(
        stack((
            phaze_icon(icons::AI, 14.0, move |p| p.accent, theme),
            label(|| "  PHAZEAI")
                .style(move |s| {
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

    let header = stack((neon_strip, header_content))
        .style(|s| s.flex_col().width_full());

    // ── Message bubbles ───────────────────────────────────────────────────────

    let msg_list = dyn_stack(
        move || {
            messages
                .get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
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
                    // Icon/Label for Tool cards
                    phaze_icon(icons::CHIP, 11.0, move |p| p.accent, theme)
                    .style(move |s: floem::style::Style| {
                        s.apply_if(!is_tool, |s| s.display(floem::style::Display::None))
                    }),
                    label(move || text_content.clone()).style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
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
                .style(|s| s.items_center())
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

    let messages_scroll = scroll(msg_list)
        .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    // ── Input bar ─────────────────────────────────────────────────────────────

    let do_send_btn = do_send.clone();
    let do_send_key = do_send.clone();
    let _ = do_send;

    let send_btn = container(
        label(|| "↵").style(move |s| {
            s.font_size(14.0).color(
                if is_loading.get() {
                    theme.get().palette.text_disabled
                } else {
                    theme.get().palette.bg_base
                }
            )
        }),
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
    .on_click_stop(move |_| { (do_send_btn)(); });

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

    stack((header, messages_scroll, input_bar))
        .style(move |s| {
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
