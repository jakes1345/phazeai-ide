//! Lists local Docker containers (requires Docker CLI).

use crate::app::{show_toast, Tab};
use crate::domain_state::IdeState;
use crate::util::append_debug_console;
use floem::{
    reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, v_stack, Decorators},
    IntoView,
};
use std::process::Command;

#[derive(Clone, Debug)]
struct ContainerRow {
    id: String,
    image: String,
    names: String,
    status: String,
}

fn docker_ps_parse() -> Result<Vec<ContainerRow>, String> {
    let output = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--format",
            "{{.ID}}\t{{.Image}}\t{{.Names}}\t{{.Status}}",
        ])
        .output()
        .map_err(|e| format!("Docker not available: {}", e))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(if err.trim().is_empty() {
            "docker ps failed (is the Docker daemon running?)".into()
        } else {
            err.trim().to_string()
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut rows = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.splitn(4, '\t').collect();
        if cols.len() == 4 {
            rows.push(ContainerRow {
                id: cols[0].to_string(),
                image: cols[1].to_string(),
                names: cols[2].to_string(),
                status: cols[3].to_string(),
            });
        }
    }
    Ok(rows)
}

pub fn containers_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let rows_sig: RwSignal<Vec<ContainerRow>> = create_rw_signal(Vec::new());
    let err: RwSignal<String> = create_rw_signal(String::new());

    let refresh = move |_: ()| {
        err.set(String::new());
        match docker_ps_parse() {
            Ok(rows) => rows_sig.set(rows),
            Err(e) => {
                rows_sig.set(vec![]);
                err.set(e);
            }
        }
    };

    refresh(());

    let header = container(
        h_stack((
            label(|| "DOCKER CONTAINERS".to_string()).style(move |s| {
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
            .on_click_stop(move |_| refresh(())),
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

    let hint =
        label(|| "`docker ps -a`. Use Logs / Shell actions to inspect a container.".to_string())
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
        move || rows_sig.get(),
        |r| format!("{}-{}", r.id, r.names),
        move |row| {
            let st = state.clone();
            let id_logs = row.id.clone();
            let id_shell = row.id.clone();
            let nm = row.names.clone();
            let id_disp = if row.id.len() > 12 {
                format!("{}…", &row.id[..12])
            } else {
                row.id.clone()
            };
            let line1 = format!("{} · {} [{}]", row.names, row.image, id_disp);
            let line2 = row.status.clone();
            container(
                v_stack((
                    label(move || line1.clone()).style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(12.5).color(p.text_primary).width_full()
                    }),
                    label(move || line2.clone()).style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(10.5).color(p.text_muted).width_full()
                    }),
                    h_stack((
                        container(label(|| "Logs".to_string()).style(move |s| {
                            let p = theme.get().palette;
                            s.font_size(11.0)
                                .padding_horiz(8.0)
                                .padding_vert(3.0)
                                .border_radius(4.0)
                                .border(1.0)
                                .border_color(p.border)
                                .cursor(floem::style::CursorStyle::Pointer)
                        }))
                        .on_click_stop(move |_| {
                            let cmd = format!("docker logs -f {}", id_logs);
                            append_debug_console(
                                st.workbench.debug_console_log,
                                format!("Containers: {}", cmd),
                            );
                            st.workbench.run_in_terminal_text.set(Some(cmd));
                            st.workbench.show_bottom_panel.set(true);
                            st.workbench.bottom_panel_tab.set(Tab::Terminal);
                            show_toast(st.workbench.status_toast, "Following container logs …");
                        }),
                        container(label(|| "Shell".to_string()).style(move |s| {
                            let p = theme.get().palette;
                            s.font_size(11.0)
                                .padding_horiz(8.0)
                                .padding_vert(3.0)
                                .border_radius(4.0)
                                .border(1.0)
                                .border_color(p.border)
                                .cursor(floem::style::CursorStyle::Pointer)
                        }))
                        .on_click_stop(move |_| {
                            let cmd = format!("docker exec -it {} sh", id_shell);
                            append_debug_console(
                                st.workbench.debug_console_log,
                                format!("Containers (shell): {}", cmd),
                            );
                            st.workbench.run_in_terminal_text.set(Some(cmd));
                            st.workbench.show_bottom_panel.set(true);
                            st.workbench.bottom_panel_tab.set(Tab::Terminal);
                            show_toast(st.workbench.status_toast, format!("Shell in {}", nm));
                        }),
                    ))
                    .style(|s| s.flex_row().gap(6.0).margin_top(4.0)),
                ))
                .style(|s| s.width_full()),
            )
            .style(move |s| {
                let p = theme.get().palette;
                s.width_full()
                    .padding_horiz(10.0)
                    .padding_vert(6.0)
                    .border_bottom(1.0)
                    .border_color(p.border.with_alpha(0.35))
            })
        },
    );

    let list = scroll(rows.style(|s| s.width_full()))
        .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    container(
        v_stack((header, hint, err_label, list)).style(|s| s.width_full().height_full().flex_col()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full().height_full().background(p.glass_bg)
    })
}
