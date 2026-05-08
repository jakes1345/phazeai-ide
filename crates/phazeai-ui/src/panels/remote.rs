//! Quick SSH connections from `~/.ssh/config` Host entries.

use crate::app::{show_toast, Tab};
use crate::domain_state::IdeState;
use crate::util::{append_debug_console, shell_join_args};
use floem::{
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, v_stack, Decorators},
    IntoView,
};
use std::path::PathBuf;

fn ssh_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ssh")
        .join("config")
}

fn parse_ssh_hosts(text: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        let mut parts = t.split_whitespace();
        let Some(keyword) = parts.next() else {
            continue;
        };
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        for h in parts {
            if h == "*"
                || h.starts_with('!')
                || h.contains('*')
                || h.contains('?')
                || !h
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
            {
                continue;
            }
            hosts.push(h.to_string());
        }
    }
    hosts.sort_unstable();
    hosts.dedup();
    hosts
}

pub fn remote_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let hosts_sig: RwSignal<Vec<String>> = create_rw_signal(Vec::new());
    let err: RwSignal<String> = create_rw_signal(String::new());
    let config_path = ssh_config_path();

    create_effect({
        let path = config_path.clone();
        move |_| {
            err.set(String::new());
            if !path.is_file() {
                hosts_sig.set(vec![]);
                err.set(format!(
                    "No SSH config at {}. Create Host entries to list shortcuts here.",
                    path.display()
                ));
                return;
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => hosts_sig.set(parse_ssh_hosts(&text)),
                Err(e) => {
                    hosts_sig.set(vec![]);
                    err.set(format!("Could not read SSH config: {}", e));
                }
            }
        }
    });

    let header = container(
        h_stack((
            label(|| "SSH HOSTS".to_string()).style(move |s| {
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
                let path = config_path.clone();
                move |_| {
                    err.set(String::new());
                    if !path.is_file() {
                        hosts_sig.set(vec![]);
                        err.set("SSH config not found.".to_string());
                        return;
                    }
                    match std::fs::read_to_string(&path) {
                        Ok(text) => hosts_sig.set(parse_ssh_hosts(&text)),
                        Err(e) => err.set(format!("Read error: {}", e)),
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

    let hint = label(move || {
        format!(
            "Reads Host entries from {}. Opens an interactive `ssh` session in the integrated terminal.",
            config_path.display()
        )
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
        move || hosts_sig.get(),
        |h| h.clone(),
        move |host| {
            let h = host.clone();
            let st = state.clone();
            let show = host.clone();
            container(
                h_stack((
                    label(move || show.clone()).style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(12.5).color(p.text_primary).flex_grow(1.0)
                    }),
                    container(label(|| "Connect ▶".to_string()).style(move |s| {
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
                        let parts = vec!["ssh".to_string(), h.clone()];
                        let cmd = shell_join_args(&parts);
                        append_debug_console(
                            st.workbench.debug_console_log,
                            format!("Remote SSH: {}", cmd),
                        );
                        st.workbench.run_in_terminal_text.set(Some(cmd));
                        st.workbench.show_bottom_panel.set(true);
                        st.workbench.bottom_panel_tab.set(Tab::Terminal);
                        show_toast(
                            st.workbench.status_toast,
                            format!("Starting ssh session to {}", h),
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
        v_stack((header, hint, err_label, list)).style(|s| s.width_full().height_full().flex_col()),
    )
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full().height_full().background(p.glass_bg)
    })
}

#[cfg(test)]
mod tests {
    use super::parse_ssh_hosts;

    #[test]
    fn parse_hosts_supports_case_insensitive_keyword() {
        let src = "Host app-prod\nHOST app-dev\n";
        let out = parse_ssh_hosts(src);
        assert!(out.contains(&"app-prod".to_string()));
        assert!(out.contains(&"app-dev".to_string()));
    }

    #[test]
    fn parse_hosts_rejects_wildcards_and_negations() {
        let src = "Host * !bad ok-host host?.example.com\n";
        let out = parse_ssh_hosts(src);
        assert_eq!(out, vec!["ok-host".to_string()]);
    }
}
