use floem::{
    reactive::{RwSignal, SignalGet},
    views::{text_input, Decorators},
    IntoView,
};

use crate::theme::PhazeTheme;
use crate::tokens::{font, radius, space, stroke};

pub fn phaze_input(
    value: RwSignal<String>,
    placeholder: impl Into<String>,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    let placeholder = placeholder.into();
    text_input(value)
        .placeholder(placeholder)
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.background(p.bg_elevated)
                .border(stroke::HAIRLINE)
                .border_color(p.border)
                .border_radius(radius::MD)
                .color(p.text_primary)
                .padding_horiz(space::MD + space::XXS)
                .padding_vert(space::SM)
                .font_size(font::UI)
                .min_width(0.0)
                .flex_grow(1.0)
        })
        .on_event_stop(floem::event::EventListener::FocusGained, move |_| {})
}
