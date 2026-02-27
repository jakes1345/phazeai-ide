use floem::{
    reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, Decorators},
    IntoView,
};

use crate::theme::PhazeTheme;

#[derive(Clone)]
pub struct TabItem {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
}

impl TabItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), icon: None }
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }
}

pub fn phaze_tabs(
    items: Vec<TabItem>,
    active_id: RwSignal<String>,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    // Store items in a signal so dyn_stack can react
    let items_signal = create_rw_signal(items);

    dyn_stack(
        move || items_signal.get(),
        |item| item.id.clone(),
        move |item| {
            let item_id = item.id.clone();
            let item_id_click = item_id.clone();
            let tab_label = if let Some(icon) = &item.icon {
                format!("{} {}", icon, item.label)
            } else {
                item.label.clone()
            };

            container(label(move || tab_label.clone()))
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    let is_active = active_id.get() == item_id;
                    s.padding_horiz(12.0)
                     .padding_vert(7.0)
                     .border_radius(6.0)
                     .font_size(12.0)
                     .cursor(floem::style::CursorStyle::Pointer)
                     .apply_if(is_active, |s| {
                         s.background(p.bg_elevated)
                          .color(p.text_primary)
                          .border_bottom(2.0)
                          .border_color(p.accent)
                     })
                     .apply_if(!is_active, |s| {
                         s.background(floem::peniko::Color::TRANSPARENT)
                          .color(p.text_muted)
                     })
                })
                .on_click_stop(move |_| {
                    active_id.set(item_id_click.clone());
                })
        },
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.border_bottom(1.0)
         .border_color(p.border)
         .gap(2.0)
         .padding_horiz(4.0)
    })
}
