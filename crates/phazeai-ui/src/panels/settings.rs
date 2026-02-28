use floem::{
    reactive::{create_effect, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};

use crate::{
    app::{save_settings, IdeState},
    components::icon::{icons, phaze_icon},
    theme::{PhazeTheme, ThemeVariant},
};

// ─── helpers ────────────────────────────────────────────────────────────────

/// A thin horizontal rule used to separate sections.
fn divider(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    container(label(|| ""))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.height(1.0)
             .width_full()
             .background(p.border)
             .margin_vert(8.0)
        })
}

/// An uppercase section header label.
fn section_header(text: &'static str, state: IdeState) -> impl IntoView {
    let theme = state.theme;
    label(move || text)
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.font_size(10.0)
             .color(p.accent)
             .font_weight(floem::text::Weight::BOLD)
             .margin_bottom(10.0)
        })
}

/// A small +/- stepper button.
fn stepper_btn(
    icon: &'static str,
    state: IdeState,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let theme = state.theme;
    let is_hovered = create_rw_signal(false);

    container(
        phaze_icon(icon, 14.0, move |p| if is_hovered.get() { p.accent } else { p.text_secondary }, theme)
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let bg = if is_hovered.get() { p.bg_elevated } else { p.bg_surface };
        s.width(24.0)
         .height(24.0)
         .border(1.0)
         .border_color(p.border)
         .border_radius(4.0)
         .background(bg)
         .items_center()
         .justify_center()
         .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| on_click())
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
        is_hovered.set(true);
    })
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
        is_hovered.set(false);
    })
}

/// A labelled row with a stepper (label | spacer | − · value · +).
fn stepper_row(
    row_label: &'static str,
    value: floem::reactive::RwSignal<u32>,
    min: u32,
    max: u32,
    state: IdeState,
) -> impl IntoView {
    let theme = state.theme;
    let dec = stepper_btn("-", state.clone(), move || {
        value.update(|v| { if *v > min { *v -= 1; } });
    });

    let inc = stepper_btn("+", state.clone(), move || {
        value.update(|v| { if *v < max { *v += 1; } });
    });

    let value_display = container(
        label(move || value.get().to_string())
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(13.0).color(p.text_primary)
            }),
    )
    .style(|s| s.width(28.0).items_center().justify_center());

    let controls = stack((dec, value_display, inc))
        .style(|s| s.flex_row().items_center().gap(2.0));

    stack((
        label(move || row_label)
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
            }),
        controls,
    ))
    .style(|s| {
        s.flex_row()
         .items_center()
         .width_full()
         .padding_vert(4.0)
    })
}

// ─── theme tiles ─────────────────────────────────────────────────────────────

fn theme_tile(
    name: &'static str,
    state: IdeState,
) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let theme = state.theme;
    let is_hovered = create_rw_signal(false);

    container(
        label(move || name)
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let active = t.variant.name() == name;
                let color = if active { p.accent } else { p.text_secondary };
                s.font_size(12.0).color(color)
            }),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let active = t.variant.name() == name;
        let hovered = is_hovered.get();
        let border_color = if active { p.accent } else if hovered { p.border_focus } else { p.border };
        let bg = if active { p.accent_dim } else if hovered { p.bg_elevated } else { p.bg_surface };
        s.width(100.0)
         .padding_vert(7.0)
         .padding_horiz(4.0)
         .background(bg)
         .border(if active { 2.0 } else { 1.0 })
         .border_color(border_color)
         .border_radius(6.0)
         .items_center()
         .justify_center()
         .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| {
        theme.set(PhazeTheme::from_str(name));
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
        is_hovered.set(true);
    })
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
        is_hovered.set(false);
    })
}

// ─── sections ────────────────────────────────────────────────────────────────

fn theme_section(state: IdeState) -> impl IntoView {
    // Build a wrapping grid of tiles for all 12 theme variants
    let tiles = dyn_stack(
        move || {
            ThemeVariant::all()
                .iter()
                .enumerate()
                .map(|(i, v)| (i, v.name()))
                .collect::<Vec<_>>()
        },
        |(i, _name)| *i,
        {
            let state = state.clone();
            move |(_i, name)| theme_tile(name, state.clone())
        },
    )
    .style(|s| {
        s.flex_row()
         .flex_wrap(floem::style::FlexWrap::Wrap)
         .gap(6.0)
         .width_full()
    });

    stack((
        section_header("THEME", state.clone()),
        tiles,
    ))
    .style(|s| s.flex_col().width_full())
}

fn editor_section(state: IdeState) -> impl IntoView {
    // Use the shared font_size and tab_size signals from IdeState.
    let font_size = state.font_size;
    let tab_size = state.tab_size;

    let auto_save   = state.auto_save;
    let word_wrap   = state.word_wrap;
    let theme_as    = state.theme;
    let as_hov      = floem::reactive::create_rw_signal(false);
    let ww_hov      = floem::reactive::create_rw_signal(false);

    let toggle_row = |label_text: &'static str,
                      sig: floem::reactive::RwSignal<bool>,
                      hov: floem::reactive::RwSignal<bool>,
                      theme_sig: floem::reactive::RwSignal<crate::theme::PhazeTheme>| {
        container(stack((
            label(move || label_text)
                .style(move |s| {
                    let p = theme_sig.get().palette;
                    s.font_size(12.0).color(p.text_primary).flex_grow(1.0)
                }),
            container(label(move || if sig.get() { "ON" } else { "OFF" }))
                .style(move |s| {
                    let p = theme_sig.get().palette;
                    let on = sig.get();
                    s.font_size(11.0).padding_horiz(8.0).padding_vert(3.0)
                     .border_radius(4.0)
                     .color(p.bg_base)
                     .background(if on { p.success } else { p.bg_elevated })
                     .border(1.0).border_color(if on { p.success } else { p.border })
                     .cursor(floem::style::CursorStyle::Pointer)
                     .apply_if(hov.get() && !on, |s| s.background(p.bg_panel))
                })
                .on_click_stop(move |_| { sig.update(|v| *v = !*v); })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| hov.set(true))
                .on_event_stop(floem::event::EventListener::PointerLeave, move |_| hov.set(false)),
        )).style(|s| s.flex_row().items_center().padding_vert(4.0)))
        .style(|s| s.width_full().padding_horiz(4.0))
    };

    stack((
        section_header("EDITOR", state.clone()),
        stepper_row("Font Size", font_size, 8, 48, state.clone()),
        stepper_row("Tab Size", tab_size, 1, 16, state.clone()),
        toggle_row("Auto Save (1.5 s delay)", auto_save, as_hov, theme_as),
        toggle_row("Word Wrap  (Alt+Z)", word_wrap, ww_hov, theme_as),
    ))
    .style(|s| s.flex_col().width_full())
}

/// A clickable provider option tile.
fn provider_tile(name: &'static str, state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let theme     = state.theme;
    let provider  = state.ai_provider;
    let is_hovered = create_rw_signal(false);

    container(
        label(move || name)
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let active = provider.get() == name;
                let color = if active { p.accent } else { p.text_secondary };
                s.font_size(11.0).color(color)
            }),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let active  = provider.get() == name;
        let hovered = is_hovered.get();
        let border_color = if active { p.accent } else if hovered { p.border_focus } else { p.border };
        let bg = if active { p.accent_dim } else if hovered { p.bg_elevated } else { p.bg_surface };
        s.padding_vert(5.0)
         .padding_horiz(8.0)
         .background(bg)
         .border(if active { 2.0 } else { 1.0 })
         .border_color(border_color)
         .border_radius(5.0)
         .items_center()
         .justify_center()
         .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| { provider.set(name.to_string()); })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| { is_hovered.set(true); })
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| { is_hovered.set(false); })
}

fn ai_section(state: IdeState) -> impl IntoView {
    let theme      = state.theme;
    let ai_model   = state.ai_model;

    // Provider tiles
    const PROVIDERS: &[&str] = &[
        "Claude (Anthropic)",
        "OpenAI",
        "Google Gemini",
        "Groq",
        "Together.ai",
        "OpenRouter",
        "Ollama (Local)",
        "LM Studio (Local)",
    ];

    let provider_tiles = dyn_stack(
        move || PROVIDERS.iter().enumerate().map(|(i, n)| (i, *n)).collect::<Vec<_>>(),
        |(i, _)| *i,
        {
            let state = state.clone();
            move |(_i, name)| provider_tile(name, state.clone())
        },
    )
    .style(|s| s.flex_row().flex_wrap(floem::style::FlexWrap::Wrap).gap(4.0).width_full());

    let provider_section = stack((
        label(|| "Provider")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(12.0).color(p.text_muted).margin_bottom(6.0)
            }),
        provider_tiles,
    ))
    .style(|s| s.flex_col().width_full().padding_vert(4.0));

    // Model row — free-text input
    let model_row = stack((
        label(|| "Model")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
            }),
        text_input(ai_model)
            .placeholder("e.g. claude-sonnet-4-6")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.width(160.0)
                 .background(p.bg_elevated)
                 .border(1.0)
                 .border_color(p.border_focus)
                 .border_radius(4.0)
                 .color(p.text_primary)
                 .padding_horiz(8.0)
                 .padding_vert(4.0)
                 .font_size(12.0)
                 .min_width(0.0)
            }),
    ))
    .style(|s| s.flex_row().items_center().width_full().padding_vert(4.0));

    stack((
        section_header("AI", state.clone()),
        provider_section,
        model_row,
    ))
    .style(|s| s.flex_col().width_full())
}

fn about_section(state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let theme = state.theme;
    let is_link_hovered = create_rw_signal(false);

    let icon_row = stack((
        phaze_icon(icons::AI, 22.0, move |p| p.accent, theme)
            .style(move |s: floem::style::Style| {
                s.margin_right(8.0)
            }),
        stack((
            label(|| "PhazeAI IDE")
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.font_size(15.0)
                     .color(p.text_primary)
                     .font_weight(floem::text::Weight::BOLD)
                }),
            label(|| "Version 0.1.0")
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.font_size(11.0).color(p.text_muted)
                }),
        ))
        .style(|s| s.flex_col().gap(2.0)),
    ))
    .style(|s| s.flex_row().items_center().padding_vert(4.0));

    let link = container(
        label(|| "github.com/phazeai/ide")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let color = if is_link_hovered.get() { p.accent_hover } else { p.accent };
                s.font_size(12.0).color(color)
            }),
    )
    .style(move |s| {
        s.padding_vert(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
        is_link_hovered.set(true);
    })
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
        is_link_hovered.set(false);
    });

    stack((
        section_header("ABOUT", state.clone()),
        icon_row,
        link,
    ))
    .style(|s| s.flex_col().width_full())
}

// ─── public entry point ──────────────────────────────────────────────────────

/// The settings panel. Accepts IdeState so that theme/font_size/tab_size are
/// the shared signals — changes here propagate to the rest of the IDE and are
/// persisted to disk via reactive effects wired in IdeState::new().
pub fn settings_panel(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    let font_size = state.font_size;
    let tab_size = state.tab_size;

    // Wire a local save effect so that changes made in the settings panel
    // (theme tiles, steppers) are flushed to disk immediately.
    create_effect(move |_| {
        let theme_name = theme.get().variant.name().to_string();
        let fs = font_size.get();
        let ts = tab_size.get();
        save_settings(&theme_name, fs, ts);
    });

    // Panel header
    let header = container(
        stack((
            phaze_icon(icons::SETTINGS, 14.0, move |p| p.accent, theme),
            label(|| "  SETTINGS")
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.font_size(11.0)
                     .color(p.accent)
                     .font_weight(floem::text::Weight::BOLD)
                }),
        ))
        .style(|s| s.flex_row().items_center()),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding(12.0)
         .border_bottom(1.0)
         .border_color(p.border)
         .width_full()
    });

    // Scrollable body containing all sections
    let body = stack((
        theme_section(state.clone()),
        divider(state.clone()),
        editor_section(state.clone()),
        divider(state.clone()),
        ai_section(state.clone()),
        divider(state.clone()),
        about_section(state.clone()),
        // Bottom breathing room
        container(label(|| "")).style(|s| s.height(24.0)),
    ))
    .style(|s| {
        s.flex_col()
         .width_full()
         .padding(16.0)
         .gap(0.0)
    });

    let scrollable_body = scroll(body)
        .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    stack((header, scrollable_body))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.flex_col()
             .width_full()
             .height_full()
             .background(p.bg_panel)
        })
}
