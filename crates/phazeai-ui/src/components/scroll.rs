use floem::{
    reactive::{RwSignal, SignalGet},
    views::{scroll, Decorators},
    IntoView,
};

use crate::theme::PhazeTheme;

pub fn phaze_scroll(
    content: impl IntoView + 'static,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    scroll(content).style(move |s| {
        let _t = theme.get();
        s.flex_grow(1.0)
         .min_height(0.0)
         .background(floem::peniko::Color::TRANSPARENT)
    })
}
