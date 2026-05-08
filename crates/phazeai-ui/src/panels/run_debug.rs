//! Workspace run presets from `.vscode/launch.json` plus common Cargo shortcuts.

use crate::app::{show_toast, Tab};
use crate::domain_state::IdeState;
use crate::util::{append_debug_console, shell_join_args, shell_quote_single};
use floem::{
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, v_stack, Decorators},
    IntoView,
};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Debug)]
struct RunPreset {
    name: String,
    shell_command: String,
}

fn strip_line_comments(raw: &str) -> String {
    // Remove `// ...` and `/* ... */` comments while preserving quoted strings.
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while let Some(ch) = chars.next() {
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
                out.push('\n');
            }
            continue;
        }
        if in_block_comment {
            if ch == '*' && matches!(chars.peek(), Some('/')) {
                let _ = chars.next();
                in_block_comment = false;
            } else if ch == '\n' {
                // Preserve line structure for easier debugging and parsing errors.
                out.push('\n');
            }
            continue;
        }

        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == '/' && matches!(chars.peek(), Some('/')) {
            let _ = chars.next();
            in_line_comment = true;
            continue;
        }
        if ch == '/' && matches!(chars.peek(), Some('*')) {
            let _ = chars.next();
            in_block_comment = true;
            continue;
        }
        out.push(ch);
    }

    out
}

fn sanitize_shell_fragment(raw: &str) -> String {
    raw.replace(['\r', '\n'], " ").trim().to_string()
}

fn expand_vars(s: &str, workspace: &Path) -> String {
    let r = workspace.to_string_lossy();
    s.replace("${workspaceFolder}", &r)
        .replace("${workspaceRoot}", &r)
}

fn value_array_strings(v: Option<&Value>) -> Vec<String> {
    let Some(arr) = v.and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| e.as_str().map(|s| s.to_string()))
        .collect()
}

fn launch_config_to_command(cfg: &Value, workspace: &Path) -> Option<String> {
    let cwd_raw = cfg.get("cwd").and_then(|x| x.as_str()).unwrap_or("");
    let cwd = if cwd_raw.is_empty() {
        workspace.to_string_lossy().to_string()
    } else {
        expand_vars(cwd_raw, workspace)
    };

    if let Some(cmd) = cfg.get("command").and_then(|x| x.as_str()) {
        let c = sanitize_shell_fragment(&expand_vars(cmd, workspace));
        if c.is_empty() {
            return None;
        }
        return Some(format!("cd {} && {}", shell_quote_single(&cwd), c));
    }

    let mut parts: Vec<String> = Vec::new();
    if let Some(re) = cfg.get("runtimeExecutable").and_then(|x| x.as_str()) {
        parts.push(expand_vars(re, workspace));
    }
    parts.extend(value_array_strings(cfg.get("runtimeArgs")));
    if let Some(prog) = cfg.get("program").and_then(|x| x.as_str()) {
        parts.push(expand_vars(prog, workspace));
    }
    parts.extend(value_array_strings(cfg.get("args")));

    if !parts.is_empty() {
        parts = parts.into_iter().map(|p| sanitize_shell_fragment(&p)).collect();
        parts.retain(|p| !p.is_empty());
        if parts.is_empty() {
            return None;
        }
        let inner = shell_join_args(&parts);
        return Some(format!("cd {} && {}", shell_quote_single(&cwd), inner));
    }

    if let Some(prog) = cfg.get("program").and_then(|x| x.as_str()) {
        let p = sanitize_shell_fragment(&expand_vars(prog, workspace));
        if p.is_empty() {
            return None;
        }
        let inner = shell_join_args(&[p]);
        return Some(format!("cd {} && {}", shell_quote_single(&cwd), inner));
    }

    None
}

fn load_launch_presets(workspace: &Path) -> Vec<RunPreset> {
    let path = workspace.join(".vscode").join("launch.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let cleaned = strip_line_comments(&raw);
    let Ok(parsed) = serde_json::from_str::<Value>(&cleaned) else {
        return Vec::new();
    };
    let Some(cfgs) = parsed.get("configurations").and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for c in cfgs {
        let name = c
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("(unnamed)")
            .to_string();
        let request = c.get("request").and_then(|x| x.as_str()).unwrap_or("");
        if request != "launch" && !request.is_empty() {
            continue;
        }
        if let Some(shell_command) = launch_config_to_command(c, workspace) {
            out.push(RunPreset {
                name: format!("{} (launch.json)", name),
                shell_command,
            });
        }
    }
    out
}

fn cargo_presets(workspace: &Path) -> Vec<RunPreset> {
    let root = workspace.to_string_lossy();
    let qc = shell_quote_single(&root);
    vec![
        RunPreset {
            name: "Cargo: run".into(),
            shell_command: format!("cd {} && cargo run", qc),
        },
        RunPreset {
            name: "Cargo: build".into(),
            shell_command: format!("cd {} && cargo build", qc),
        },
        RunPreset {
            name: "Cargo: test".into(),
            shell_command: format!("cd {} && cargo test", qc),
        },
        RunPreset {
            name: "Cargo: clippy".into(),
            shell_command: format!("cd {} && cargo clippy --workspace -- -D warnings", qc),
        },
    ]
}

pub fn run_debug_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let root = state.project.workspace_root;
    let presets: RwSignal<Vec<RunPreset>> = create_rw_signal(Vec::new());

    create_effect(move |_| {
        let ws = root.get();
        let mut list = cargo_presets(&ws);
        list.extend(load_launch_presets(&ws));
        presets.set(list);
    });

    let header = container(label(|| "RUN & DEBUG".to_string()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(11.0)
            .font_weight(floem::text::Weight::BOLD)
            .color(p.text_muted)
            .padding_horiz(12.0)
            .padding_vert(8.0)
    }))
    .style(move |s| {
        let p = theme.get().palette;
        s.width_full().border_bottom(1.0).border_color(p.border)
    });

    let explain = label(|| {
        "Runs commands in the integrated terminal. For breakpoints and stepping, attach an external debugger (lldb, gdb, debugpy) to your process — full DAP UI is not embedded yet.".to_string()
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(10.5)
            .color(p.text_muted)
            .padding_horiz(12.0)
            .padding_bottom(8.0)
    });

    let rows = dyn_stack(
        move || presets.get(),
        |p| p.name.clone(),
        move |preset| {
            let st = state.clone();
            let name = preset.name.clone();
            let name2 = preset.name.clone();
            let cmd_disp = preset.shell_command.clone();
            let cmd_run = preset.shell_command.clone();
            container(
                h_stack((
                    v_stack((
                        label(move || name2.clone()).style(move |s| {
                            let p = theme.get().palette;
                            s.font_size(12.5).color(p.text_primary)
                        }),
                        label(move || cmd_disp.clone()).style(move |s| {
                            let p = theme.get().palette;
                            s.font_size(10.0)
                                .color(p.text_muted)
                                .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                        }),
                    ))
                    .style(|s| s.flex_col().flex_grow(1.0).min_width(0.0)),
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
                        append_debug_console(
                            st.workbench.debug_console_log,
                            format!("Run: [{}] {}", name, cmd_run),
                        );
                        st.workbench.run_in_terminal_text.set(Some(cmd_run.clone()));
                        st.workbench.show_bottom_panel.set(true);
                        st.workbench.bottom_panel_tab.set(Tab::Terminal);
                        show_toast(st.workbench.status_toast, format!("Running: {}", name));
                    }),
                ))
                .style(|s| s.items_start().width_full().gap(6.0)),
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

    container(v_stack((header, explain, list)).style(|s| s.width_full().height_full().flex_col()))
        .style(move |s| {
            let p = theme.get().palette;
            s.width_full().height_full().background(p.glass_bg)
        })
}

#[cfg(test)]
mod tests {
    use super::{sanitize_shell_fragment, strip_line_comments};

    #[test]
    fn strip_comments_preserves_strings() {
        let src = r#"{
  // comment
  "url": "https://example.com/x//y",
  "cmd": "echo /*not-comment*/"
}"#;
        let cleaned = strip_line_comments(src);
        assert!(cleaned.contains(r#""url": "https://example.com/x//y""#));
        assert!(cleaned.contains(r#""cmd": "echo /*not-comment*/""#));
    }

    #[test]
    fn strip_comments_removes_block_and_line_comments() {
        let src = "{\n/* first */\n\"a\":1,\n// second\n\"b\":2\n}";
        let cleaned = strip_line_comments(src);
        assert!(!cleaned.contains("first"));
        assert!(!cleaned.contains("second"));
        assert!(cleaned.contains("\"a\":1"));
        assert!(cleaned.contains("\"b\":2"));
    }

    #[test]
    fn sanitize_shell_fragment_flattens_newlines() {
        assert_eq!(
            sanitize_shell_fragment("  cargo run\r\n--release  "),
            "cargo run  --release"
        );
    }
}
