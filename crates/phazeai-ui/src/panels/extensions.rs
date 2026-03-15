use crate::app::IdeState;
use crate::components::button::{phaze_button, ButtonVariant};
use crate::components::input::phaze_input;
use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, v_stack, Decorators},
    IntoView,
};
use rfd::FileDialog;

/// Combined extension manager panel.
///
/// Supports two extension types:
/// 1. **Native Rust plugins** — cdylib + plugin.toml in ~/.phazeai/plugins/
/// 2. **VSCode extensions** — .vsix files extracted to ~/.phazeai/extensions/
///    (themes, grammars, snippets, language configs loaded natively — no JS)
pub fn extensions_panel(state: IdeState) -> impl IntoView {
    let theme = state.theme;
    let search_query = create_rw_signal(String::new());

    // ── Channel for all extension scan/install results ────────────────────────

    let (result_tx, result_rx) =
        std::sync::mpsc::sync_channel::<Result<Vec<String>, String>>(4);
    let result_signal = create_signal_from_channel(result_rx);
    {
        let state = state.clone();
        create_effect(move |_| {
            if let Some(res) = result_signal.get() {
                state.ext_loading.set(false);
                match res {
                    Ok(names) => {
                        state.extensions.set(names);
                    }
                    Err(e) => {
                        crate::app::show_toast(
                            state.status_toast,
                            format!("Extension error: {}", e),
                        );
                    }
                }
            }
        });
    }

    // ── Scan all extensions (native plugins + VSCode assets) ─────────────────

    let scan_action = {
        let state = state.clone();
        let tx = result_tx.clone();
        move |_: ()| {
            state.ext_loading.set(true);
            let manager = state.ext_manager.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut all_names: Vec<String> = Vec::new();

                // 1. Scan native Rust plugins
                if let Ok(mut mgr) = manager.lock() {
                    let host = phazeai_core::ext_host::DummyDelegate;
                    let host = phazeai_core::ext_host::IdeDelegateHost::new(
                        std::sync::Arc::new(host),
                    );
                    mgr.scan_plugins(&host);
                    for p in mgr.get_plugins() {
                        all_names.push(format!(
                            "[Plugin] {} v{} — {}",
                            p.name, p.version, p.description
                        ));
                    }
                }

                // 2. Scan VSCode extension assets
                let mut registry = phazeai_core::ext_host::registry::ExtensionRegistry::new();
                registry.load_all();
                for summary in registry.summary() {
                    all_names.push(format!("[VSCode] {}", summary));
                }

                let _ = tx.send(Ok(all_names));
            });
        }
    };

    // ── Install .vsix file ───────────────────────────────────────────────────

    // Channel for install toast messages
    let (toast_tx, toast_rx) = std::sync::mpsc::sync_channel::<String>(4);
    let toast_signal = create_signal_from_channel(toast_rx);
    {
        let toast = state.status_toast;
        create_effect(move |_| {
            if let Some(msg) = toast_signal.get() {
                crate::app::show_toast(toast, msg);
            }
        });
    }

    let install_vsix_action = {
        let state = state.clone();
        let tx = result_tx.clone();
        let toast_tx = toast_tx.clone();
        move |_: ()| {
            let Some(path) = FileDialog::new()
                .add_filter("VSCode Extension", &["vsix"])
                .set_title("Install VSCode Extension (.vsix)")
                .pick_file()
            else {
                return;
            };

            state.ext_loading.set(true);
            let tx = tx.clone();
            let toast_tx = toast_tx.clone();
            std::thread::spawn(move || {
                match phazeai_core::ext_host::asset_loader::install_vsix(&path) {
                    Ok(ext) => {
                        let name = ext
                            .manifest
                            .display_name
                            .as_deref()
                            .unwrap_or(&ext.manifest.name)
                            .to_string();
                        let contributes = ext.manifest.contributes.as_ref();
                        let themes = contributes.map(|c| c.themes.len()).unwrap_or(0);
                        let grammars = contributes.map(|c| c.grammars.len()).unwrap_or(0);
                        let snippets = contributes.map(|c| c.snippets.len()).unwrap_or(0);

                        let _ = toast_tx.send(format!(
                            "Installed {} — {} themes, {} grammars, {} snippets",
                            name, themes, grammars, snippets
                        ));

                        // Rescan all extensions
                        let mut registry =
                            phazeai_core::ext_host::registry::ExtensionRegistry::new();
                        registry.load_all();
                        let names: Vec<String> = registry
                            .summary()
                            .into_iter()
                            .map(|s| format!("[VSCode] {}", s))
                            .collect();
                        let _ = tx.send(Ok(names));
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            });
        }
    };

    // ── Open directories ─────────────────────────────────────────────────────

    let open_plugins_dir = move |_: ()| {
        let dir = home_dir().join(".phazeai").join("plugins");
        let _ = std::fs::create_dir_all(&dir);
        open_in_file_manager(&dir);
    };

    let open_extensions_dir = move |_: ()| {
        let dir = home_dir().join(".phazeai").join("extensions");
        let _ = std::fs::create_dir_all(&dir);
        open_in_file_manager(&dir);
    };

    // ── UI ───────────────────────────────────────────────────────────────────

    let ext_list = scroll(
        v_stack((
            // Section: installed extensions
            label(move || {
                if state.ext_loading.get() {
                    "Scanning...".to_string()
                } else if state.extensions.get().is_empty() {
                    "No extensions installed.\n\nInstall a .vsix file or place native plugins in ~/.phazeai/plugins/".to_string()
                } else {
                    format!("{} extension(s)", state.extensions.get().len())
                }
            })
            .style(move |s| {
                let p = theme.get().palette;
                s.color(p.text_muted)
                    .font_size(11.0)
                    .margin_bottom(8.0)
                    .padding(4.0)
                    .width_full()
            }),
            dyn_stack(
                move || state.extensions.get(),
                |ext| ext.clone(),
                move |ext| {
                    let is_vscode = ext.starts_with("[VSCode]");
                    container(label(move || ext.clone()).style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(11.5).color(if is_vscode {
                            p.accent
                        } else {
                            p.text_primary
                        })
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
        // Primary actions
        v_stack((
            h_stack((
                phaze_button("Install .vsix", ButtonVariant::Primary, theme, {
                    let action = install_vsix_action.clone();
                    move || action(())
                }),
                phaze_button("Scan All", ButtonVariant::Secondary, theme, {
                    let scan = scan_action.clone();
                    move || scan(())
                }),
            ))
            .style(|s| s.gap(8.0).width_full()),
            h_stack((
                phaze_button("Plugins Dir", ButtonVariant::Secondary, theme, move || {
                    open_plugins_dir(())
                }),
                phaze_button("Extensions Dir", ButtonVariant::Secondary, theme, move || {
                    open_extensions_dir(())
                }),
            ))
            .style(|s| s.gap(8.0).width_full().margin_top(4.0)),
        ))
        .style(|s| {
            s.padding_horiz(10.0)
                .padding_vert(8.0)
                .width_full()
        }),
        // Search filter
        container(
            phaze_input(search_query, "Filter extensions...", theme).style(|s| s.width_full()),
        )
        .style(|s| s.padding_horiz(10.0).padding_bottom(8.0).width_full()),
        // Info
        container(
            v_stack((
                label(|| "[Plugin] = Native Rust (.so/.dylib)".to_string())
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(9.5).color(p.text_muted)
                    }),
                label(|| "[VSCode] = Themes, grammars, snippets (no JS)".to_string())
                    .style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(9.5).color(p.accent)
                    }),
            ))
            .style(|s| s.gap(2.0)),
        )
        .style(|s| s.padding_horiz(10.0).padding_bottom(8.0).width_full()),
        // Extension list
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

// ── Helpers ──────────────────────────────────────────────────────────────────

fn home_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn open_in_file_manager(dir: &std::path::Path) {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(dir).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
}
