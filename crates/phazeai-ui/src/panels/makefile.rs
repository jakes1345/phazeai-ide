//! Browse Makefile targets and run them in the integrated terminal.

use crate::app::{show_toast, Tab};
use crate::domain_state::IdeState;
use crate::util::{append_debug_console, safe_get, shell_quote_single};
use floem::{
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, v_stack, Decorators},
    IntoView,
};
use std::path::{Path, PathBuf};

fn find_makefile(workspace: &Path) -> Option<PathBuf> {
    for name in ["Makefile", "makefile", "GNUmakefile"] {
        let p = workspace.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn parse_targets(contents: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in contents.lines() {
        let line_no_comment = line.split('#').next().unwrap_or("").trim_end();
        if line_no_comment.starts_with('\t') {
            continue;
        }
        if let Some((head, tail)) = line_no_comment.split_once(':') {
            if tail.starts_with('=') {
                continue;
            }
            let name = head.trim();
            let Some(first) = name.chars().next() else {
                continue;
            };
            if (!first.is_ascii_alphabetic() && first != '_' && first != '%')
                || name.contains(' ')
                || name.contains('\t')
            {
                continue;
            }
            out.push(name.to_string());
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn makefile_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let root = state.project.workspace_root;
    let targets: RwSignal<Vec<String>> = create_rw_signal(Vec::new());
    let makefile_path: RwSignal<String> = create_rw_signal(String::new());
    let err: RwSignal<String> = create_rw_signal(String::new());

    create_effect(move |_| {
        let _ = root.get();
        err.set(String::new());
        let ws = root.get_untracked();
        if ws.as_os_str().is_empty() || !ws.is_dir() {
            makefile_path.set(String::new());
            targets.set(vec![]);
            err.set("No workspace folder open.".to_string());
            return;
        }
        let Some(path) = find_makefile(&ws) else {
            makefile_path.set(String::new());
            targets.set(vec![]);
            err.set("No Makefile, makefile, or GNUmakefile found in workspace root.".to_string());
            return;
        };
        makefile_path.set(path.display().to_string());
        match std::fs::read_to_string(&path) {
            Ok(text) => targets.set(parse_targets(&text)),
            Err(e) => {
                makefile_path.set(String::new());
                targets.set(vec![]);
                err.set(format!("Could not read Makefile: {}", e));
            }
        }
    });

    let header = container(
        h_stack((
            label(|| "MAKEFILE TARGETS".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0)
                    .font_weight(floem::text::Weight::BOLD)
                    .color(p.text_muted)
                    .flex_grow(1.0)
            }),
            container(label(|| "↻ Refresh".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(11.0)
                    .color(p.accent)
                    .cursor(floem::style::CursorStyle::Pointer)
            }))
            .on_click_stop({
                move |_| {
                    err.set(String::new());
                    let ws = root.get_untracked();
                    if ws.as_os_str().is_empty() || !ws.is_dir() {
                        makefile_path.set(String::new());
                        targets.set(vec![]);
                        err.set("No workspace folder open.".to_string());
                        return;
                    }
                    let Some(path) = find_makefile(&ws) else {
                        makefile_path.set(String::new());
                        targets.set(vec![]);
                        err.set(
                            "No Makefile, makefile, or GNUmakefile found in workspace root."
                                .to_string(),
                        );
                        return;
                    };
                    makefile_path.set(path.display().to_string());
                    match std::fs::read_to_string(&path) {
                        Ok(text) => targets.set(parse_targets(&text)),
                        Err(e) => {
                            makefile_path.set(String::new());
                            targets.set(vec![]);
                            err.set(format!("Could not read Makefile: {}", e));
                        }
                    }
                }
            }),
        ))
        .style(|s| s.items_center().width_full()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.padding_horiz(12.0)
            .padding_vert(8.0)
            .border_bottom(1.0)
            .border_color(p.border)
            .width_full()
    });

    let path_label = label(move || {
        let p_str = makefile_path.get();
        if p_str.is_empty() {
            String::new()
        } else {
            format!("📄 {}", p_str)
        }
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(10.5)
            .color(p.text_muted)
            .padding_horiz(12.0)
            .padding_bottom(6.0)
    });

    let err_label = label(move || err.get()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(11.0)
            .color(p.warning)
            .padding_horiz(12.0)
            .padding_bottom(6.0)
            .apply_if(err.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let rows = dyn_stack(
        move || targets.get(),
        |t| t.clone(),
        move |target| {
            let t_clone = target.clone();
            let st = state.clone();
            let target2 = target.clone();
            container(
                h_stack((
                    label(move || target2.clone()).style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(12.5).color(p.text_primary).flex_grow(1.0)
                    }),
                    container(label(|| "Run ▶".to_string()).style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(11.0)
                            .padding_horiz(8.0)
                            .padding_vert(3.0)
                            .border_radius(4.0)
                            .border(1.0)
                            .border_color(p.border)
                            .color(p.accent)
                            .cursor(floem::style::CursorStyle::Pointer)
                    }))
                    .on_click_stop(move |_| {
                        let ws = safe_get(st.project.workspace_root, PathBuf::new());
                        let cmd = format!(
                            "cd {} && make {}",
                            shell_quote_single(&ws.to_string_lossy()),
                            t_clone
                        );
                        append_debug_console(
                            st.workbench.debug_console_log,
                            format!("Makefile: {}", cmd),
                        );
                        st.workbench.run_in_terminal_text.set(Some(cmd));
                        st.workbench.show_bottom_panel.set(true);
                        st.workbench.bottom_panel_tab.set(Tab::Terminal);
                        show_toast(
                            st.workbench.status_toast,
                            format!("Running `make {}` in terminal", t_clone),
                        );
                    }),
                ))
                .style(|s| s.items_center().width_full().padding_vert(4.0)),
            )
            .style(move |s| {
                let p = theme.get().palette;
                s.width_full()
                    .padding_horiz(10.0)
                    .border_bottom(1.0)
                    .border_color(p.border.with_alpha(0.35))
            })
        },
    );

    let list = scroll(rows.style(|s| s.width_full()))
        .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    container(
        v_stack((header, path_label, err_label, list))
            .style(|s| s.width_full().height_full().flex_col()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full().height_full().background(p.glass_bg)
    })
}
