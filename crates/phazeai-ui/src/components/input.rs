use floem::{
    reactive::{RwSignal, SignalGet},
    views::{text_input, Decorators},
    IntoView,
};

use crate::theme::PhazeTheme;

pub fn phaze_input(
    value: RwSignal<String>,
    placeholder: impl Into<String>,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    let _placeholder = placeholder.into();
    text_input(value).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.background(p.bg_elevated)
         .border(1.0)
         .border_color(p.border)
         .border_radius(6.0)
         .color(p.text_primary)
         .padding_horiz(10.0)
         .padding_vert(6.0)
         .font_size(13.0)
         .min_width(0.0)
         .flex_grow(1.0)
    })
    .on_event_stop(floem::event::EventListener::FocusGained, move |_| {})
}
