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

#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub enum AiMode {
    Chat,
    Ask,
    Debug,
    Plan,
    Edit,
}

impl AiMode {
    pub fn label(&self) -> &'static str {
        match self {
            AiMode::Chat => "Chat",
            AiMode::Ask => "Ask",
            AiMode::Debug => "Debug",
            AiMode::Plan => "Plan",
            AiMode::Edit => "Edit",
        }
    }

    pub fn system_hint(&self) -> &'static str {
        match self {
            AiMode::Chat => "You are a helpful AI assistant integrated into the PhazeAI IDE.",
            AiMode::Ask  => "Answer concisely and precisely. No extra prose.",
            AiMode::Debug => "You are a debugging expert. Focus on root causes and fixes.",
            AiMode::Plan => "You are a software architect. Produce clear step-by-step plans.",
            AiMode::Edit => "You are a code editor. Produce only code changes, no commentary.",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum MsgRole { User, Assistant }

#[derive(Clone, Debug)]
struct Msg {
    role: MsgRole,
    content: String,
    loading: bool,
}

#[derive(Clone, Debug)]
enum AiUpdate {
    Partial(String),
    Done(String),
    Err(String),
}

fn send_message(text: String, settings: Settings, mode: AiMode, tx: std::sync::mpsc::SyncSender<AiUpdate>) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => { let _ = tx.send(AiUpdate::Err(format!("Runtime: {e}"))); return; }
        };
        rt.block_on(async move {
            let client = match settings.build_llm_client() {
                Ok(c) => c,
                Err(e) => { let _ = tx.send(AiUpdate::Err(format!("LLM: {e}"))); return; }
            };
            let agent = Agent::new(client);
            let prompt = format!("{}\n\nUser: {}", mode.system_hint(), text);
            let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
            let run_fut = agent.run_with_events(&prompt, agent_tx);
            let drain_fut = async {
                let mut acc = String::new();
                while let Some(ev) = agent_rx.recv().await {
                    match ev {
                        AgentEvent::TextDelta(t) => {
                            acc.push_str(&t);
                            let _ = tx.send(AiUpdate::Partial(acc.clone()));
                        }
                        AgentEvent::Complete { .. } => {
                            let _ = tx.send(AiUpdate::Done(acc.clone()));
                            break;
                        }
                        AgentEvent::Error(e) => {
                            let _ = tx.send(AiUpdate::Err(e));
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

/// AI panel — replaces the old "AI context — coming soon" placeholder in the left sidebar.
pub fn ai_panel(theme: RwSignal<PhazeTheme>) -> impl IntoView {
    let settings = Settings::load();
    let mode = create_rw_signal(AiMode::Chat);
    let messages: RwSignal<Vec<Msg>> = create_rw_signal(vec![Msg {
        role: MsgRole::Assistant,
        content: "PhazeAI ready. Select a mode above.".to_string(),
        loading: false,
    }]);
    let input_text = create_rw_signal(String::new());
    let is_loading = create_rw_signal(false);

    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<AiUpdate>(256);
    let update_signal = create_signal_from_channel(update_rx);

    create_effect(move |_| {
        if let Some(update) = update_signal.get() {
            match update {
                AiUpdate::Partial(text) => {
                    messages.update(|list| {
                        if let Some(last) = list.last_mut() {
                            if last.role == MsgRole::Assistant && last.loading {
                                last.content = text;
                            }
                        }
                    });
                }
                AiUpdate::Done(text) => {
                    messages.update(|list| {
                        if let Some(last) = list.last_mut() {
                            if last.role == MsgRole::Assistant && last.loading {
                                last.content = if text.is_empty() { "(no response)".to_string() } else { text };
                                last.loading = false;
                            }
                        }
                    });
                    is_loading.set(false);
                }
                AiUpdate::Err(e) => {
                    messages.update(|list| {
                        if let Some(last) = list.last_mut() {
                            if last.role == MsgRole::Assistant && last.loading {
                                last.content = format!("Error: {e}");
                                last.loading = false;
                            }
                        }
                    });
                    is_loading.set(false);
                }
            }
        }
    });

    let update_tx = Arc::new(update_tx);

    let do_send: Rc<dyn Fn()> = Rc::new({
        let update_tx = update_tx.clone();
        let settings = settings.clone();
        move || {
            let text = input_text.get();
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() || is_loading.get() { return; }
            let current_mode = mode.get();
            messages.update(|list| {
                list.push(Msg { role: MsgRole::User, content: trimmed.clone(), loading: false });
                list.push(Msg { role: MsgRole::Assistant, content: String::new(), loading: true });
            });
            input_text.set(String::new());
            is_loading.set(true);
            send_message(trimmed, settings.clone(), current_mode, (*update_tx).clone());
        }
    });

    // ── Neon strip ────────────────────────────────────────────────────────────
    let neon_strip = container(label(|| ""))
        .style(move |s| s.height(2.0).width_full().background(theme.get().palette.accent));

    // ── Mode tabs ─────────────────────────────────────────────────────────────
    let modes = [AiMode::Chat, AiMode::Ask, AiMode::Debug, AiMode::Plan, AiMode::Edit];
    let mode_tabs = stack((
        {
            let m = modes[0];
            let is_hov = create_rw_signal(false);
            container(label(move || m.label()))
                .style(move |s| {
                    let t = theme.get(); let p = &t.palette;
                    let active = mode.get() == m;
                    s.padding_horiz(10.0).padding_vert(5.0).font_size(11.0)
                     .color(if active { p.accent } else { p.text_muted })
                     .background(if active { p.accent_dim } else if is_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                     .border_radius(4.0).cursor(floem::style::CursorStyle::Pointer)
                     .apply_if(active, |s| s.border_bottom(2.0).border_color(p.accent))
                })
                .on_click_stop(move |_| { mode.set(m); })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { is_hov.set(true); })
                .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { is_hov.set(false); })
        },
        {
            let m = modes[1];
            let is_hov = create_rw_signal(false);
            container(label(move || m.label()))
                .style(move |s| {
                    let t = theme.get(); let p = &t.palette;
                    let active = mode.get() == m;
                    s.padding_horiz(10.0).padding_vert(5.0).font_size(11.0)
                     .color(if active { p.accent } else { p.text_muted })
                     .background(if active { p.accent_dim } else if is_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                     .border_radius(4.0).cursor(floem::style::CursorStyle::Pointer)
                     .apply_if(active, |s| s.border_bottom(2.0).border_color(p.accent))
                })
                .on_click_stop(move |_| { mode.set(m); })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { is_hov.set(true); })
                .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { is_hov.set(false); })
        },
        {
            let m = modes[2];
            let is_hov = create_rw_signal(false);
            container(label(move || m.label()))
                .style(move |s| {
                    let t = theme.get(); let p = &t.palette;
                    let active = mode.get() == m;
                    s.padding_horiz(10.0).padding_vert(5.0).font_size(11.0)
                     .color(if active { p.accent } else { p.text_muted })
                     .background(if active { p.accent_dim } else if is_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                     .border_radius(4.0).cursor(floem::style::CursorStyle::Pointer)
                     .apply_if(active, |s| s.border_bottom(2.0).border_color(p.accent))
                })
                .on_click_stop(move |_| { mode.set(m); })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { is_hov.set(true); })
                .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { is_hov.set(false); })
        },
        {
            let m = modes[3];
            let is_hov = create_rw_signal(false);
            container(label(move || m.label()))
                .style(move |s| {
                    let t = theme.get(); let p = &t.palette;
                    let active = mode.get() == m;
                    s.padding_horiz(10.0).padding_vert(5.0).font_size(11.0)
                     .color(if active { p.accent } else { p.text_muted })
                     .background(if active { p.accent_dim } else if is_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                     .border_radius(4.0).cursor(floem::style::CursorStyle::Pointer)
                     .apply_if(active, |s| s.border_bottom(2.0).border_color(p.accent))
                })
                .on_click_stop(move |_| { mode.set(m); })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { is_hov.set(true); })
                .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { is_hov.set(false); })
        },
        {
            let m = modes[4];
            let is_hov = create_rw_signal(false);
            container(label(move || m.label()))
                .style(move |s| {
                    let t = theme.get(); let p = &t.palette;
                    let active = mode.get() == m;
                    s.padding_horiz(10.0).padding_vert(5.0).font_size(11.0)
                     .color(if active { p.accent } else { p.text_muted })
                     .background(if active { p.accent_dim } else if is_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                     .border_radius(4.0).cursor(floem::style::CursorStyle::Pointer)
                     .apply_if(active, |s| s.border_bottom(2.0).border_color(p.accent))
                })
                .on_click_stop(move |_| { mode.set(m); })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { is_hov.set(true); })
                .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { is_hov.set(false); })
        },
    ))
    .style(move |s| {
        let t = theme.get(); let p = &t.palette;
        s.width_full().background(p.glass_bg).border_bottom(1.0).border_color(p.glass_border).items_center().padding_horiz(4.0).padding_vert(4.0)
    });

    // ── Model indicator ───────────────────────────────────────────────────────
    let model_name = settings.llm.model.clone();
    let model_bar = container(
        stack((
            phaze_icon(icons::ROBOT, 10.0, move |p| p.accent, theme),
            label(move || format!("  {}", model_name))
                .style(move |s| s.font_size(10.0).color(theme.get().palette.text_muted)),
        ))
        .style(|s| s.items_center()),
    )
    .style(move |s| {
        let t = theme.get(); let p = &t.palette;
        s.padding_horiz(12.0).padding_vert(5.0).width_full().background(p.bg_deep).border_bottom(1.0).border_color(p.glass_border)
    });

    // ── Messages ──────────────────────────────────────────────────────────────
    let msg_list = dyn_stack(
        move || messages.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(_, msg)| {
            let is_user = msg.role == MsgRole::User;
            let content = msg.content.clone();
            let loading = msg.loading;
            let text_content = if loading && content.is_empty() { "●●●".to_string() } else { content };
            let is_typing = loading && text_content.starts_with('●');

            container(
                label(move || text_content.clone()).style(move |s| {
                    let t = theme.get(); let p = &t.palette;
                    s.font_size(12.0).color(if is_typing { p.accent } else if is_user { p.text_primary } else { p.text_secondary })
                     .max_width_pct(100.0).line_height(1.5)
                })
            )
            .style(move |s| {
                let t = theme.get(); let p = &t.palette;
                if is_user {
                    s.width_full().padding_horiz(12.0).padding_vert(8.0)
                     .background(p.accent_dim).border(1.0).border_color(p.glass_border)
                     .border_radius(10.0).margin_bottom(6.0)
                } else {
                    s.width_full().padding_horiz(12.0).padding_vert(8.0)
                     .background(p.glass_bg).border(1.0).border_color(p.glass_border)
                     .border_radius(8.0).margin_bottom(6.0)
                }
            })
        },
    )
    .style(|s| s.flex_col().padding(8.0).width_full());

    let messages_scroll = scroll(msg_list).style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    // ── Input bar ─────────────────────────────────────────────────────────────
    let do_send_btn = do_send.clone();
    let do_send_key = do_send.clone();
    let _ = do_send;

    let send_btn = container(
        label(|| "↵").style(move |s| {
            s.font_size(13.0).color(if is_loading.get() { theme.get().palette.text_disabled } else { theme.get().palette.bg_base })
        })
    )
    .style(move |s| {
        let t = theme.get(); let p = &t.palette;
        s.width(28.0).height(28.0)
         .background(if is_loading.get() { p.bg_elevated } else { p.accent })
         .border_radius(6.0).items_center().justify_center()
         .cursor(floem::style::CursorStyle::Pointer).margin_left(6.0)
    })
    .on_click_stop(move |_| { (do_send_btn)(); });

    let input_widget = text_input(input_text)
        .style(move |s| {
            let t = theme.get(); let p = &t.palette;
            s.flex_grow(1.0).background(p.glass_bg).border(1.0).border_color(p.border_focus)
             .border_radius(6.0).color(p.text_primary).padding_horiz(10.0).padding_vert(6.0)
             .font_size(12.0).min_width(0.0)
        })
        .on_event_stop(EventListener::KeyDown, move |event| {
            if let Event::KeyDown(e) = event {
                let enter = match &e.key.logical_key {
                    Key::Character(ch) => ch.as_str() == "\r" || ch.as_str() == "\n",
                    Key::Named(floem::keyboard::NamedKey::Enter) => true,
                    _ => false,
                };
                if enter && !e.modifiers.contains(Modifiers::SHIFT) { (do_send_key)(); }
            }
        });

    let mode_hint = label(move || {
        match mode.get() {
            AiMode::Chat  => "Chat with AI",
            AiMode::Ask   => "Ask a question",
            AiMode::Debug => "Describe the bug",
            AiMode::Plan  => "Describe the feature",
            AiMode::Edit  => "Describe the change",
        }
    })
    .style(move |s| s.font_size(10.0).color(theme.get().palette.text_disabled).margin_bottom(4.0));

    let input_bar = container(
        stack((
            mode_hint,
            stack((input_widget, send_btn)).style(|s| s.items_center().width_full()),
        ))
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        let t = theme.get(); let p = &t.palette;
        s.padding(10.0).border_top(1.0).border_color(p.glass_border).width_full().background(p.glass_bg)
    });

    // ── Assemble panel ────────────────────────────────────────────────────────
    stack((neon_strip, mode_tabs, model_bar, messages_scroll, input_bar))
        .style(move |s| {
            let t = theme.get(); let p = &t.palette;
            s.flex_col().width_full().height_full().background(p.glass_bg)
        })
}
