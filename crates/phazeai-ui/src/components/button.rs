use floem::{
    reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, label, Decorators},
    IntoView,
};

use crate::theme::PhazeTheme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

pub fn phaze_button(
    text: impl Into<String>,
    variant: ButtonVariant,
    theme: RwSignal<PhazeTheme>,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    let text = text.into();
    let is_hovered = create_rw_signal(false);

    container(label(move || text.clone()))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let (bg, fg, border_color) = match variant {
                ButtonVariant::Primary => (
                    if is_hovered.get() { p.accent_hover } else { p.accent },
                    floem::peniko::Color::WHITE,
                    p.accent,
                ),
                ButtonVariant::Secondary => (
                    if is_hovered.get() { p.bg_elevated } else { p.bg_surface },
                    p.text_primary,
                    p.border,
                ),
                ButtonVariant::Ghost => (
                    if is_hovered.get() { p.accent_dim } else { floem::peniko::Color::TRANSPARENT },
                    p.accent,
                    floem::peniko::Color::TRANSPARENT,
                ),
                ButtonVariant::Danger => (
                    if is_hovered.get() { p.error.with_alpha(0.9) } else { p.error.with_alpha(0.15) },
                    p.error,
                    p.error.with_alpha(0.5),
                ),
            };
            s.padding_horiz(12.0)
             .padding_vert(6.0)
             .border_radius(6.0)
             .background(bg)
             .color(fg)
             .border(1.0)
             .border_color(border_color)
             .font_size(13.0)
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

pub fn phaze_icon_button(
    icon: impl Into<String>,
    _tooltip: impl Into<String>,
    theme: RwSignal<PhazeTheme>,
    active: bool,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    let icon = icon.into();
    let is_hovered = create_rw_signal(false);

    container(label(move || icon.clone()))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let bg = if active {
                p.accent_dim
            } else if is_hovered.get() {
                p.bg_elevated
            } else {
                floem::peniko::Color::TRANSPARENT
            };
            let fg = if active { p.accent } else { p.text_secondary };
            s.width(32.0)
             .height(32.0)
             .border_radius(6.0)
             .background(bg)
             .color(fg)
             .font_size(16.0)
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
