//! Settings → Local AI: pick, download and run a coding model on this machine
//! with PhazeAI's built-in llama.cpp server (`phazeai_core::local_ai`).
//!
//! All slow work (hardware probe, downloads, llama.cpp install, server start)
//! runs on background threads and reports back through one channel drained on
//! the UI thread with `create_signal_from_channel`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::Duration;

use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate, SignalWith},
    views::{container, dyn_stack, empty, label, stack, Decorators},
    IntoView, View,
};
use phazeai_core::{
    local_ai::{
        self,
        catalog::{self, CatalogEntry, Fit},
        download::{self, DownloadProgress},
        hardware::HardwareInfo,
        human_bytes,
        install::{self, FoundServer},
        LocalAiSettings, ServerStatus, LOCAL_AI,
    },
    ProviderId, Settings,
};

use crate::{
    components::button::{phaze_button, ButtonVariant},
    domain_state::IdeState,
    theme::PhazeTheme,
    tokens::{font, radius, space, stroke},
};

/// Messages from background work to the view.
#[derive(Clone)]
enum Event {
    Hardware(HardwareInfo),
    Progress(&'static str, DownloadProgress),
    DownloadDone(&'static str, Result<(), String>),
    InstallProgress(DownloadProgress),
    InstallDone(Result<String, String>),
}

#[derive(Clone)]
struct ActiveDownload {
    id: &'static str,
    progress: Option<DownloadProgress>,
    cancel: Arc<AtomicBool>,
}

/// `1.2 GB of 4.7 GB · 11.0 MB/s · about 6 min left`
pub fn progress_text(p: &DownloadProgress) -> String {
    let mut s = format!("{} of {}", human_bytes(p.downloaded), human_bytes(p.total));
    if p.bytes_per_sec > 0.0 {
        s.push_str(&format!(" · {}/s", human_bytes(p.bytes_per_sec as u64)));
        let left = p.total.saturating_sub(p.downloaded) as f64 / p.bytes_per_sec;
        s.push_str(&format!(" · {}", eta_text(left)));
    }
    s
}

pub fn eta_text(secs: f64) -> String {
    if secs < 60.0 {
        "under a minute left".into()
    } else if secs < 3600.0 {
        format!("about {} min left", (secs / 60.0).round() as u64)
    } else {
        format!("about {:.1} h left", secs / 3600.0)
    }
}

fn find_binary(settings: &LocalAiSettings) -> Result<Option<FoundServer>, String> {
    let path = std::env::var_os("PATH");
    install::find_server_binary(
        settings.server_path.as_deref(),
        path.as_deref(),
        &local_ai::llama_cpp_dir(),
    )
    .map_err(|e| e.to_string())
}

fn save_local_ai(mutate: impl FnOnce(&mut LocalAiSettings)) -> LocalAiSettings {
    let _guard = crate::app::SETTINGS_WRITE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut s = Settings::load();
    mutate(&mut s.local_ai);
    let _ = s.save();
    s.local_ai
}

fn caption(theme: RwSignal<PhazeTheme>, text: impl Fn() -> String + 'static) -> impl IntoView {
    label(text).style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.text_muted)
            .line_height(1.35)
    })
}

/// A button that is only shown while `visible()` holds.
fn shown_button(
    text: &'static str,
    variant: ButtonVariant,
    theme: RwSignal<PhazeTheme>,
    visible: impl Fn() -> bool + 'static,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    container(phaze_button(text, variant, theme, on_click))
        .style(move |s| s.apply_if(!visible(), |s| s.display(floem::style::Display::None)))
}

fn badge(
    text: &'static str,
    theme: RwSignal<PhazeTheme>,
    visible: impl Fn() -> bool + 'static,
) -> impl IntoView {
    label(move || text).style(move |s| {
        let p = theme.get().palette;
        s.font_size(font::CAPTION)
            .padding_horiz(space::SM)
            .padding_vert(space::XXS)
            .border_radius(radius::PILL)
            .background(p.accent_dim)
            .color(p.accent)
            .apply_if(!visible(), |s| s.display(floem::style::Display::None))
    })
}

/// The "LOCAL AI" settings section.
pub fn local_ai_section(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let initial = Settings::load().local_ai;

    let hardware = create_rw_signal(None::<HardwareInfo>);
    let active = create_rw_signal(None::<ActiveDownload>);
    let installing = create_rw_signal(None::<Option<DownloadProgress>>);
    let install_cancel = Arc::new(AtomicBool::new(false));
    let message = create_rw_signal(String::new());
    let message_is_error = create_rw_signal(false);
    // Bumped whenever files on disk change, so per-entry state is re-read.
    let disk_gen = create_rw_signal(0u64);
    let selected = create_rw_signal(initial.model.clone());
    let binary = create_rw_signal(find_binary(&initial));

    let (tx, rx) = std::sync::mpsc::sync_channel::<Event>(256);
    let events = create_signal_from_channel(rx);
    create_effect(move |_| {
        let Some(ev) = events.get() else { return };
        // `events` holds the value; take what we need without cloning the enum.
        match ev {
            Event::Hardware(hw) => hardware.set(Some(hw)),
            Event::Progress(id, p) => active.update(|a| {
                if let Some(a) = a.as_mut().filter(|a| a.id == id) {
                    a.progress = Some(p);
                }
            }),
            Event::DownloadDone(id, result) => {
                active.set(None);
                disk_gen.update(|g| *g += 1);
                let name = catalog::find(id).map(|e| e.display_name).unwrap_or(id);
                match result {
                    Ok(()) => {
                        message_is_error.set(false);
                        message.set(format!("{name} downloaded and verified (SHA-256)."));
                    }
                    Err(e) => {
                        message_is_error.set(e != "cancelled");
                        message.set(if e == "cancelled" {
                            format!("Download of {name} paused. Click Download to resume.")
                        } else {
                            format!("Download of {name} failed: {e}")
                        });
                    }
                }
            }
            Event::InstallProgress(p) => installing.set(Some(Some(p))),
            Event::InstallDone(result) => {
                installing.set(None);
                binary.set(find_binary(&Settings::load().local_ai));
                match result {
                    Ok(what) => {
                        message_is_error.set(false);
                        message.set(format!("Installed {what}."));
                    }
                    Err(e) => {
                        message_is_error.set(true);
                        message.set(e);
                    }
                }
            }
        }
    });

    // Probe the hardware off the UI thread (re-probed after each download).
    let probe = {
        let tx = tx.clone();
        move || {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Event::Hardware(
                    HardwareInfo::probe(&local_ai::models_dir()),
                ));
            });
        }
    };
    probe();
    {
        let probe = probe.clone();
        create_effect(move |prev: Option<u64>| {
            let g = disk_gen.get();
            if prev.is_some_and(|p| p != g) {
                probe();
            }
            g
        });
    }

    // Server status: polled from LOCAL_AI, sent only on change.
    let (status_tx, status_rx) = std::sync::mpsc::sync_channel::<ServerStatus>(4);
    let server_status = create_signal_from_channel(status_rx);
    std::thread::spawn(move || {
        let mut last = None;
        loop {
            let now = LOCAL_AI.status();
            if last.as_ref() != Some(&now) {
                if status_tx.send(now.clone()).is_err() {
                    break;
                }
                last = Some(now);
            }
            std::thread::sleep(Duration::from_millis(400));
        }
    });
    let status = move || server_status.get().unwrap_or(ServerStatus::Stopped);

    let recommended =
        move || hardware.with(|h| h.as_ref().and_then(catalog::recommend).map(|e| e.id));

    // ── header + hardware ───────────────────────────────────────────────────
    let header = label(|| "LOCAL AI").style(move |s| {
        s.font_size(10.0)
            .color(theme.get().palette.accent)
            .font_weight(floem::text::Weight::BOLD)
            .margin_bottom(space::SM)
    });
    let intro = caption(theme, || {
        "Run a coding model on this computer with PhazeAI's built-in llama.cpp server. \
         No account or API key, and prompts never leave your machine."
            .into()
    });
    let hw_line = label(move || {
        hardware.with(|h| match h {
            Some(h) => format!("This machine: {}", h.summary()),
            None => "Checking this machine…".into(),
        })
    })
    .style(move |s| {
        s.font_size(font::BODY)
            .color(theme.get().palette.text_secondary)
            .margin_vert(space::SM)
    });

    // ── catalog cards ───────────────────────────────────────────────────────
    let card_state = CardState {
        state: state.clone(),
        hardware,
        active,
        message,
        message_is_error,
        disk_gen,
        selected,
        tx: tx.clone(),
    };
    let cards = dyn_stack(
        move || catalog::CATALOG.iter().collect::<Vec<_>>(),
        |e| e.id,
        move |e| model_card(e, card_state.clone(), recommended),
    )
    .style(|s| s.flex_col().width_full().gap(space::MD));

    // ── server ──────────────────────────────────────────────────────────────
    let server_line = label(move || format!("Server: {}", status().describe())).style(move |s| {
        let p = theme.get().palette;
        let c = match status() {
            ServerStatus::Running { .. } => p.success,
            ServerStatus::Starting { .. } => p.warning,
            ServerStatus::Failed(_) => p.error,
            ServerStatus::Stopped => p.text_secondary,
        };
        s.font_size(font::BODY).color(c).line_height(1.35)
    });
    let binary_line = label(move || {
        binary.with(|b| match b {
            Ok(Some(found)) => format!("llama-server: {}", found.describe()),
            Ok(None) => format!(
                "llama-server isn't installed. Install llama.cpp below, or: {}",
                install::manual_install_hint(std::env::consts::OS)
            ),
            Err(e) => e.clone(),
        })
    })
    .style(move |s| {
        let p = theme.get().palette;
        let missing = binary.with(|b| !matches!(b, Ok(Some(_))));
        s.font_size(font::CAPTION)
            .color(if missing { p.warning } else { p.text_muted })
            .line_height(1.35)
    });
    let install_line = label(move || {
        installing.with(|i| match i {
            Some(Some(p)) => format!("Installing llama.cpp: {}", progress_text(p)),
            Some(None) => "Looking up the latest llama.cpp release…".into(),
            None => String::new(),
        })
    })
    .style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.text_muted)
            .apply_if(installing.with(|i| i.is_none()), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let start = move || {
        message.set(String::new());
        let settings = Settings::load().local_ai;
        LOCAL_AI.start_in_background(settings);
    };
    // Only offer Start when it can work: a binary and a downloaded model.
    let can_start = move || {
        disk_gen.get();
        selected.get();
        let model_ready = phazeai_core::local_ai::server::selected_model(
            &Settings::load().local_ai,
            &local_ai::models_dir(),
        )
        .is_ok_and(|e| download::is_downloaded(e, &local_ai::models_dir()));
        model_ready
            && binary.with(|b| matches!(b, Ok(Some(_))))
            && !matches!(
                status(),
                ServerStatus::Running { .. } | ServerStatus::Starting { .. }
            )
    };
    let install_tx = tx.clone();
    let cancel_flag = install_cancel.clone();
    let server_buttons = stack((
        shown_button("Start", ButtonVariant::Primary, theme, can_start, start),
        shown_button(
            "Stop",
            ButtonVariant::Secondary,
            theme,
            move || {
                matches!(
                    status(),
                    ServerStatus::Running { .. } | ServerStatus::Starting { .. }
                )
            },
            || LOCAL_AI.stop(),
        ),
        shown_button(
            "Install llama.cpp",
            ButtonVariant::Secondary,
            theme,
            move || binary.with(|b| matches!(b, Ok(None))) && installing.with(|i| i.is_none()),
            move || {
                installing.set(Some(None));
                message.set(String::new());
                cancel_flag.store(false, Ordering::Relaxed);
                let tx = install_tx.clone();
                let cancel = cancel_flag.clone();
                std::thread::spawn(move || {
                    let result = run_blocking(async {
                        let progress_tx = tx.clone();
                        install::install_latest(&local_ai::llama_cpp_dir(), cancel, move |p| {
                            let _ = progress_tx.try_send(Event::InstallProgress(p));
                        })
                        .await
                        .map(|f| f.describe())
                        .map_err(|e| e.to_string())
                    });
                    let _ = tx.send(Event::InstallDone(result));
                });
            },
        ),
        shown_button(
            "Cancel install",
            ButtonVariant::Ghost,
            theme,
            move || installing.with(|i| i.is_some()),
            move || install_cancel.store(true, Ordering::Relaxed),
        ),
    ))
    .style(|s| {
        s.flex_row()
            .flex_wrap(floem::style::FlexWrap::Wrap)
            .gap(space::SM)
            .margin_top(space::SM)
    });

    let message_line = label(move || message.get()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(font::CAPTION)
            .color(if message_is_error.get() {
                p.error
            } else {
                p.text_muted
            })
            .line_height(1.35)
            .margin_top(space::SM)
            .apply_if(message.with(|m| m.is_empty()), |s| {
                s.display(floem::style::Display::None)
            })
    });
    let config_hint = caption(theme, || {
        format!(
            "Models: {}. Advanced: [local_ai] server_path, ctx_size and auto_start in config.toml.",
            local_ai::models_dir().display()
        )
    });

    let section = stack((
        header,
        intro,
        hw_line,
        cards,
        container(empty()).style(|s| s.height(space::MD)),
        server_line,
        binary_line,
        install_line,
        server_buttons,
        message_line,
        config_hint,
    ))
    .style(|s| s.flex_col().width_full().gap(space::XS));

    // "Set up AI →" on the first-run card scrolls here.
    let id = section.id();
    let focus = state.workbench.local_ai_focus_nonce;
    create_effect(move |prev: Option<u64>| {
        let n = focus.get();
        if prev.is_some_and(|p| p != n) {
            floem::action::exec_after(Duration::from_millis(80), move |_| id.scroll_to(None));
        }
        n
    });

    section
}

#[derive(Clone)]
struct CardState {
    state: IdeState,
    hardware: RwSignal<Option<HardwareInfo>>,
    active: RwSignal<Option<ActiveDownload>>,
    message: RwSignal<String>,
    message_is_error: RwSignal<bool>,
    disk_gen: RwSignal<u64>,
    selected: RwSignal<Option<String>>,
    tx: SyncSender<Event>,
}

fn run_blocking<T>(fut: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("could not start a runtime: {e}"))?
        .block_on(fut)
}

fn model_card(
    entry: &'static CatalogEntry,
    cs: CardState,
    recommended: impl Fn() -> Option<&'static str> + Copy + 'static,
) -> impl IntoView {
    let theme = cs.state.workbench.theme;
    let CardState {
        state,
        hardware,
        active,
        message,
        message_is_error,
        disk_gen,
        selected,
        tx,
    } = cs;
    let ai_provider = state.ai.provider;
    let ai_model = state.ai.model;
    let models_dir = local_ai::models_dir();

    // Copy closures (no captured PathBuf) so every view can share them.
    let downloaded = move || {
        disk_gen.get();
        download::is_downloaded(entry, &local_ai::models_dir())
    };
    let partial = move || {
        disk_gen.get();
        download::partial_bytes(entry, &local_ai::models_dir())
    };
    let is_active = move || active.with(|a| a.as_ref().is_some_and(|a| a.id == entry.id));
    let busy_other = move || active.with(|a| a.as_ref().is_some_and(|a| a.id != entry.id));
    let is_recommended = move || recommended() == Some(entry.id);
    let in_use = move || {
        ai_provider.get() == ProviderId::BuiltIn.name()
            && selected.with(|s| s.as_deref() == Some(entry.id))
    };
    let report =
        move || hardware.with(|h| h.as_ref().map(|h| catalog::fit(entry, h, entry.ctx_size)));

    let title = stack((
        label(move || entry.display_name).style(move |s| {
            s.font_size(font::UI)
                .font_weight(floem::text::Weight::BOLD)
                .color(theme.get().palette.text_primary)
        }),
        badge("Recommended", theme, is_recommended),
        badge("In use", theme, in_use),
    ))
    .style(|s| {
        s.flex_row()
            .items_center()
            .gap(space::SM)
            .flex_wrap(floem::style::FlexWrap::Wrap)
    });

    let meta = label(move || {
        let fit = report().map(|r| r.fit.label()).unwrap_or("Checking fit…");
        format!(
            "{} download · {} · {}k context · {fit}",
            human_bytes(entry.size),
            entry.license,
            entry.ctx_size / 1024
        )
    })
    .style(move |s| {
        let p = theme.get().palette;
        let c = match report().map(|r| r.fit) {
            Some(Fit::Fits) => p.success,
            Some(Fit::Tight) => p.warning,
            Some(Fit::TooBig) => p.error,
            None => p.text_muted,
        };
        s.font_size(font::CAPTION).color(c)
    });

    let good_at = caption(theme, move || entry.good_at.to_string());
    let runtime_note = label(move || entry.runtime_note.unwrap_or_default()).style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.warning)
            .line_height(1.35)
            .apply_if(entry.runtime_note.is_none(), |s| {
                s.display(floem::style::Display::None)
            })
    });
    let fit_reason = caption(theme, move || {
        let Some(r) = report() else {
            return String::new();
        };
        let mut s = r.reason.clone();
        if r.disk_ok == Some(false) && !downloaded() {
            s.push_str(&format!(
                " Not enough free disk space for the {} download.",
                human_bytes(entry.size)
            ));
        }
        s
    });

    // Progress bar + text while downloading.
    let fraction = move || {
        active.with(|a| {
            a.as_ref()
                .filter(|a| a.id == entry.id)
                .and_then(|a| a.progress)
                .map(|p| p.fraction())
                .unwrap_or(0.0)
        })
    };
    let bar = container(container(empty()).style(move |s| {
        s.height_full()
            .width_pct(fraction() * 100.0)
            .border_radius(radius::PILL)
            .background(theme.get().palette.accent)
    }))
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full()
            .height(6.0)
            .border_radius(radius::PILL)
            .background(p.bg_elevated)
            .margin_top(space::XS)
            .apply_if(!is_active(), |s| s.display(floem::style::Display::None))
    });
    let progress_line = caption(theme, move || {
        active.with(|a| match a.as_ref().filter(|a| a.id == entry.id) {
            Some(ActiveDownload {
                progress: Some(p), ..
            }) => progress_text(p),
            Some(_) => "Connecting…".into(),
            None => match partial() {
                Some(n) if !downloaded() => format!(
                    "Paused at {} of {} — Download resumes it.",
                    human_bytes(n),
                    human_bytes(entry.size)
                ),
                _ => String::new(),
            },
        })
    });

    // Buttons.
    let download_btn = {
        let tx = tx.clone();
        let dir = models_dir.clone();
        shown_button(
            "Download",
            ButtonVariant::Primary,
            theme,
            move || {
                !downloaded()
                    && !is_active()
                    && !busy_other()
                    && report().map(|r| r.fit) != Some(Fit::TooBig)
            },
            move || {
                let cancel = Arc::new(AtomicBool::new(false));
                active.set(Some(ActiveDownload {
                    id: entry.id,
                    progress: None,
                    cancel: cancel.clone(),
                }));
                message.set(String::new());
                let tx = tx.clone();
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let progress_tx = tx.clone();
                    let result = run_blocking(async {
                        download::download_model(entry, &dir, cancel, move |p| {
                            let _ = progress_tx.try_send(Event::Progress(entry.id, p));
                        })
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                    });
                    let _ = tx.send(Event::DownloadDone(entry.id, result));
                });
            },
        )
    };
    let cancel_btn = shown_button(
        "Cancel",
        ButtonVariant::Ghost,
        theme,
        is_active,
        move || {
            if let Some(a) = active.get_untracked() {
                a.cancel.store(true, Ordering::Relaxed);
            }
        },
    );
    let use_btn = shown_button(
        "Use this",
        ButtonVariant::Primary,
        theme,
        move || downloaded() && !in_use(),
        move || {
            let local = save_local_ai(|l| l.model = Some(entry.id.to_string()));
            selected.set(Some(entry.id.to_string()));
            // The provider/model effect in IdeState persists these to [llm].
            ai_provider.set(ProviderId::BuiltIn.name().to_string());
            ai_model.set(entry.id.to_string());
            let running_this = matches!(
                LOCAL_AI.status(),
                ServerStatus::Running { ref model_id, .. } | ServerStatus::Starting { ref model_id }
                    if model_id == entry.id
            );
            if !running_this {
                LOCAL_AI.start_in_background(local);
            }
            message_is_error.set(false);
            message.set(format!(
                "AI provider set to {} with {}.",
                ProviderId::BuiltIn.name(),
                entry.display_name
            ));
        },
    );
    let delete_btn = {
        let dir = models_dir.clone();
        shown_button(
            "Delete",
            ButtonVariant::Danger,
            theme,
            move || (downloaded() || partial().is_some()) && !is_active(),
            move || {
                if matches!(
                    LOCAL_AI.status(),
                    ServerStatus::Running { ref model_id, .. } | ServerStatus::Starting { ref model_id }
                        if model_id == entry.id
                ) {
                    LOCAL_AI.stop();
                }
                match download::delete_model(entry, &dir) {
                    Ok(()) => {
                        message_is_error.set(false);
                        message.set(format!("Deleted {}.", entry.display_name));
                    }
                    Err(e) => {
                        message_is_error.set(true);
                        message.set(format!("Could not delete {}: {e}", entry.display_name));
                    }
                }
                disk_gen.update(|g| *g += 1);
            },
        )
    };
    let buttons = stack((download_btn, cancel_btn, use_btn, delete_btn)).style(|s| {
        s.flex_row()
            .flex_wrap(floem::style::FlexWrap::Wrap)
            .gap(space::SM)
            .margin_top(space::XS)
    });

    stack((
        title,
        meta,
        good_at,
        runtime_note,
        fit_reason,
        bar,
        progress_line,
        buttons,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        let highlight = is_recommended();
        s.flex_col()
            .width_full()
            .gap(space::XXS)
            .padding(space::LG)
            .border_radius(radius::LG)
            .border(if highlight {
                stroke::FOCUS
            } else {
                stroke::HAIRLINE
            })
            .border_color(if highlight { p.accent } else { p.border })
            .background(p.bg_surface)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_wording() {
        assert_eq!(eta_text(10.0), "under a minute left");
        assert_eq!(eta_text(360.0), "about 6 min left");
        assert_eq!(eta_text(5400.0), "about 1.5 h left");
    }

    #[test]
    fn progress_wording() {
        let p = DownloadProgress {
            downloaded: 1_200_000_000,
            total: 4_683_073_536,
            bytes_per_sec: 11_000_000.0,
        };
        assert_eq!(
            progress_text(&p),
            "1.2 GB of 4.7 GB · 11.0 MB/s · about 5 min left"
        );
        let idle = DownloadProgress {
            bytes_per_sec: 0.0,
            ..p
        };
        assert_eq!(progress_text(&idle), "1.2 GB of 4.7 GB");
    }
}
