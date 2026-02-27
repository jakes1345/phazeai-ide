use floem::{
    reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};

use crate::{
    components::icon::{icons, phaze_icon},
    theme::{PhazeTheme, ThemeVariant},
};

// ─── helpers ────────────────────────────────────────────────────────────────

/// A thin horizontal rule used to separate sections.
fn divider(theme: RwSignal<PhazeTheme>) -> impl IntoView {
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
fn section_header(text: &'static str, theme: RwSignal<PhazeTheme>) -> impl IntoView {
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
    theme: RwSignal<PhazeTheme>,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
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
    value: RwSignal<u32>,
    min: u32,
    max: u32,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    let dec = stepper_btn("-", theme, move || {
        value.update(|v| { if *v > min { *v -= 1; } });
    });

    let inc = stepper_btn("+", theme, move || {
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
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
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

fn theme_section(theme: RwSignal<PhazeTheme>) -> impl IntoView {
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
        move |(_i, name)| theme_tile(name, theme),
    )
    .style(|s| {
        s.flex_row()
         .flex_wrap(floem::style::FlexWrap::Wrap)
         .gap(6.0)
         .width_full()
    });

    stack((
        section_header("THEME", theme),
        tiles,
    ))
    .style(|s| s.flex_col().width_full())
}

fn editor_section(theme: RwSignal<PhazeTheme>) -> impl IntoView {
    let font_size = create_rw_signal::<u32>(14);
    let tab_size = create_rw_signal::<u32>(4);

    stack((
        section_header("EDITOR", theme),
        stepper_row("Font Size", font_size, 8, 48, theme),
        stepper_row("Tab Size", tab_size, 1, 16, theme),
    ))
    .style(|s| s.flex_col().width_full())
}

fn ai_section(theme: RwSignal<PhazeTheme>) -> impl IntoView {
    let model_text = create_rw_signal("phaze-beast".to_string());

    // Provider row
    let provider_row = stack((
        label(|| "AI Provider")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
            }),
        container(
            label(|| "Ollama (Local)")
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.font_size(12.0).color(p.text_secondary)
                }),
        )
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.padding_horiz(8.0)
             .padding_vert(4.0)
             .background(p.bg_surface)
             .border(1.0)
             .border_color(p.border)
             .border_radius(4.0)
        }),
    ))
    .style(|s| {
        s.flex_row()
         .items_center()
         .width_full()
         .padding_vert(4.0)
    });

    // Model row
    let model_row = stack((
        label(|| "Model")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
            }),
        text_input(model_text)
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.width(140.0)
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
    .style(|s| {
        s.flex_row()
         .items_center()
         .width_full()
         .padding_vert(4.0)
    });

    // Temperature row (static display)
    let temperature_row = stack((
        label(|| "Temperature")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
            }),
        // Slider track with fill (static at 0.7)
        container(
            stack((
                // Track background
                container(label(|| ""))
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        s.width_full()
                         .height(4.0)
                         .background(p.bg_elevated)
                         .border_radius(2.0)
                    }),
                // Fill (70% = 0.7)
                container(label(|| ""))
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        s.width_pct(70.0)
                         .height(4.0)
                         .background(p.accent)
                         .border_radius(2.0)
                         .position(floem::style::Position::Absolute)
                    }),
                // Thumb
                container(label(|| ""))
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        s.width(10.0)
                         .height(10.0)
                         .background(p.accent)
                         .border_radius(5.0)
                         .position(floem::style::Position::Absolute)
                         .inset_left_pct(70.0)
                         .margin_top(-3.0)
                         .margin_left(-5.0)
                    }),
            ))
            .style(|s| s.flex_row().items_center().width_full().height(10.0).position(floem::style::Position::Relative)),
        )
        .style(move |s| {
            s.width(140.0).height(10.0).items_center()
        }),
        // Value label
        label(|| " 0.7")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(12.0).color(p.text_secondary).margin_left(6.0)
            }),
    ))
    .style(|s| {
        s.flex_row()
         .items_center()
         .width_full()
         .padding_vert(4.0)
    });

    stack((
        section_header("AI", theme),
        provider_row,
        model_row,
        temperature_row,
    ))
    .style(|s| s.flex_col().width_full())
}

fn about_section(theme: RwSignal<PhazeTheme>) -> impl IntoView {
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
        section_header("ABOUT", theme),
        icon_row,
        link,
    ))
    .style(|s| s.flex_col().width_full())
}

// ─── public entry point ──────────────────────────────────────────────────────

/// The settings panel. Pass the same `theme` signal used by the rest of the IDE
/// so that theme changes propagate globally in real-time.
pub fn settings_panel(theme: RwSignal<PhazeTheme>) -> impl IntoView {
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
        theme_section(theme),
        divider(theme),
        editor_section(theme),
        divider(theme),
        ai_section(theme),
        divider(theme),
        about_section(theme),
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
