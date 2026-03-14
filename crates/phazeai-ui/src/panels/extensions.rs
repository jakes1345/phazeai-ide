use crate::app::IdeState;
use crate::components::button::{phaze_button, ButtonVariant};
use crate::components::input::phaze_input;
use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, v_stack, Decorators},
    IntoView,
};

/// Native plugin manager panel.
///
/// Scans `~/.phazeai/plugins/` for native Rust plugins (cdylib + plugin.toml).
/// Shows loaded plugins with name, version, author, status, and commands.
pub fn extensions_panel(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    let search_query = create_rw_signal(String::new());

    // Channel for scan results — created once
    let (scan_tx, scan_rx) =
        std::sync::mpsc::sync_channel::<Result<Vec<String>, String>>(1);
    let scan_result = create_signal_from_channel(scan_rx);
    {
        let state = state.clone();
        create_effect(move |_| {
            if let Some(res) = scan_result.get() {
                state.ext_loading.set(false);
                match res {
                    Ok(names) => {
                        state.extensions.set(names);
                        crate::app::show_toast(
                            state.status_toast,
                            "Plugins scanned successfully",
                        );
                    }
                    Err(e) => {
                        crate::app::show_toast(
                            state.status_toast,
                            format!("Scan failed: {}", e),
                        );
                    }
                }
            }
        });
    }

    // Scan plugins action
    let scan_action = {
        let state = state.clone();
        let scan_tx = scan_tx.clone();
        move |_: ()| {
            state.ext_loading.set(true);
            let manager = state.ext_manager.clone();
            let tx = scan_tx.clone();
            std::thread::spawn(move || {
                let mut mgr = match manager.lock() {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = tx.send(Err(format!("Lock error: {}", e)));
                        return;
                    }
                };
                let host = phazeai_core::ext_host::DummyDelegate;
                let host = phazeai_core::ext_host::IdeDelegateHost::new(
                    std::sync::Arc::new(host),
                );
                mgr.scan_plugins(&host);
                let names: Vec<String> = mgr
                    .get_plugins()
                    .into_iter()
                    .map(|p| format!("{} v{} — {}", p.name, p.version, p.description))
                    .collect();
                let _ = tx.send(Ok(names));
            });
        }
    };

    // Open plugins directory
    let open_dir_action = move |_: ()| {
        let dir = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(".phazeai")
            .join("plugins");
        // Create the directory if it doesn't exist
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&dir).spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
        }
    };

    let ext_list = scroll(
        v_stack((
            label(|| "Installed Plugins".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(13.0)
                    .font_weight(floem::text::Weight::BOLD)
                    .margin_bottom(10.0)
                    .color(p.text_primary)
            }),
            label(move || {
                if state.ext_loading.get() {
                    "Scanning plugins...".to_string()
                } else if state.extensions.get().is_empty() {
                    "No plugins loaded. Place plugin directories in ~/.phazeai/plugins/ and click Scan.".to_string()
                } else {
                    format!("{} plugin(s) loaded.", state.extensions.get().len())
                }
            })
            .style(move |s| {
                let p = theme.get().palette;
                s.color(p.text_muted).font_size(11.0).margin_bottom(8.0)
            }),
            dyn_stack(
                move || state.extensions.get(),
                |ext| ext.clone(),
                move |ext| {
                    container(label(move || ext.clone()).style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(12.0).color(p.text_primary)
                    }))
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.padding(8.0)
                            .width_full()
                            .border_bottom(1.0)
                            .border_color(p.glass_border)
                    })
                },
            ),
        ))
        .style(|s| s.padding(10.0).width_full()),
    )
    .style(|s| s.width_full().flex_grow(1.0));

    v_stack((
        // Header
        container(
            label(|| "EXTENSIONS".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0)
                    .font_weight(floem::text::Weight::BOLD)
                    .color(p.text_muted)
            }),
        )
        .style(move |s| {
            let p = theme.get().palette;
            s.padding(10.0)
                .width_full()
                .border_bottom(1.0)
                .border_color(p.glass_border)
        }),
        // Actions
        h_stack((
            phaze_button("Scan Plugins", ButtonVariant::Primary, theme, {
                let scan = scan_action.clone();
                move || scan(())
            }),
            phaze_button("Open Directory", ButtonVariant::Secondary, theme, {
                let open = open_dir_action.clone();
                move || open(())
            }),
        ))
        .style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(8.0)
                .width_full()
                .gap(8.0)
        }),
        // Search filter
        container(
            phaze_input(search_query, "Filter plugins...", theme).style(|s| s.width_full()),
        )
        .style(|s| s.padding_horiz(10.0).padding_bottom(10.0).width_full()),
        // Help text
        container(
            label(|| "Native Rust plugins: place a directory with plugin.toml + .so/.dylib in ~/.phazeai/plugins/".to_string())
                .style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(10.0).color(p.text_muted)
                }),
        )
        .style(|s| s.padding_horiz(10.0).padding_bottom(8.0).width_full()),
        // List
        ext_list,
    ))
    .style(move |s| {
        let t = theme.get().palette;
        s.width_full()
            .height_full()
            .background(t.bg_base)
            .color(t.text_primary)
            .font_size(13.0)
    })
}
