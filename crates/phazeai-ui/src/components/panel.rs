use floem::{
    reactive::{RwSignal, SignalGet},
    views::{container, Decorators},
    IntoView,
};

use crate::theme::PhazeTheme;
use crate::tokens::{radius, stroke};

/// Solid panel with theme background and border.
pub fn phaze_panel(content: impl IntoView + 'static, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    container(content).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.background(p.bg_panel)
            .border(stroke::HAIRLINE)
            .border_color(p.border)
    })
}

/// Glass panel — semi-transparent bg with accent border.
/// Uses Floem's blur layer when available; falls back to
/// semi-transparent solid fill on platforms that don't support blur.
pub fn phaze_glass_panel(
    content: impl IntoView + 'static,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    container(content).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.background(p.glass_bg)
            .border(stroke::HAIRLINE)
            .border_color(p.glass_border)
            .border_radius(radius::LG)
    })
}
