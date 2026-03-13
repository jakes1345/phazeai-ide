use crate::app::IdeState;
use crate::components::button::{phaze_button, ButtonVariant};
use crate::components::input::phaze_input;
use floem::{
    ext_event::create_ext_action,
    reactive::{create_rw_signal, Scope, SignalGet, SignalUpdate},
    views::{container, h_stack, label, scroll, v_stack, Decorators},
    IntoView,
};
use phazeai_core::ext_host::vsix::VsixLoader;
use rfd::FileDialog;

pub fn extensions_panel(state: IdeState) -> impl IntoView {
    let search_query = create_rw_signal(String::new());

    // Load VSIX File Action
    let load_vsix_action = {
        let state = state.clone();
        move |_| {
            let state = state.clone();
            let Some(path) = FileDialog::new()
                .add_filter("VSCode Extension", &["vsix"])
                .pick_file()
            else {
                return;
            };

            state.ext_loading.set(true);

            let scope = Scope::new();
            let on_done = create_ext_action(scope, move |res: Result<Vec<String>, String>| {
                state.ext_loading.set(false);
                match res {
                    Ok(exts) => {
                        state.extensions.set(exts);
                        crate::app::show_toast(state.status_toast, "Extension loaded successfully");
                    }
                    Err(e) => {
                        crate::app::show_toast(state.status_toast, format!("Failed to load: {}", e));
                    }
                }
            });

            let manager = state.ext_manager.clone();
            std::thread::spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        on_done(Err(format!("Failed to create runtime: {}", e)));
                        return;
                    }
                };
                rt.block_on(async move {
                    if let Err(e) = VsixLoader::load_vsix(&path, &manager).await {
                        on_done(Err(e));
                    } else {
                        let exts = manager.get_extensions().await;
                        on_done(Ok(exts));
                    }
                });
            });
        }
    };

    let ext_list = scroll(
        v_stack((
            label(|| "Installed Extensions".to_string())
                .style(|s| s.font_size(14.0).font_weight(floem::text::Weight::BOLD).margin_bottom(10.0)),
            label(move || {
                if state.ext_loading.get() {
                    "Loading extension...".to_string()
                } else if state.extensions.get().is_empty() {
                    "No extensions loaded yet.".to_string()
                } else {
                    format!("{} extensions loaded.", state.extensions.get().len())
                }
            }).style(|s| s.color(floem::peniko::Color::from_rgb8(150, 150, 150))),
            floem::views::dyn_stack(
                move || state.extensions.get(),
                |ext| ext.clone(),
                |ext| {
                    container(label(move || ext.clone()))
                        .style(|s| s.padding(5.0).width_full().border_bottom(1.0).border_color(floem::peniko::Color::from_rgb8(50, 50, 50)))
                }
            )
        ))
        .style(|s| s.padding(10.0).width_full()),
    )
    .style(|s| s.width_full().height_full());

    v_stack((
        // Header
        h_stack((
            label(|| "EXTENSIONS".to_string())
                .style(|s| s.font_size(12.0).font_weight(floem::text::Weight::BOLD)),
        ))
        .style(|s| s.padding(10.0).width_full().justify_between()),
        
        // Actions
        h_stack((
            phaze_button("Install from VSIX...", ButtonVariant::Secondary, state.theme, move || load_vsix_action(())),
        ))
        .style(|s| s.padding_horiz(10.0).padding_bottom(10.0).width_full()),
        
        // Search
        container(
            phaze_input(
                search_query,
                "Search Extensions in Marketplace",
                state.theme,
            )
            .style(|s| s.width_full()),
        )
        .style(|s| s.padding_horiz(10.0).padding_bottom(10.0).width_full()),

        // List
        ext_list,
    ))
    .style(move |s| {
        let t = state.theme.get().palette;
        s.width_full()
            .height_full()
            .background(t.bg_base)
            .color(t.text_primary)
            .font_size(13.0)
    })
}
