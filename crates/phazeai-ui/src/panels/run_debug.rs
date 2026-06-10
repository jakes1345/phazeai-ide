//! Workspace run presets from `.vscode/launch.json` plus common Cargo shortcuts.

use crate::app::{show_toast, Tab};
use crate::debug_session::{detect_adapter, run_session, DebugCmd, SessionUpdate};
use crate::domain_state::project::DebugStatus;
use crate::domain_state::IdeState;
use crate::util::{append_debug_console, shell_join_args, shell_quote_single};
use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, text_input, v_stack, Decorators},
    IntoView,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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
        parts = parts
            .into_iter()
            .map(|p| sanitize_shell_fragment(&p))
            .collect();
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

/// Best-guess debug target: `target/debug/<workspace dir name>` if it exists.
fn default_program(workspace: &Path) -> String {
    let name = workspace
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let candidate = workspace.join("target").join("debug").join(&name);
    if candidate.is_file() {
        candidate.to_string_lossy().to_string()
    } else {
        String::new()
    }
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

    // ── DAP debug session wiring ─────────────────────────────────────────────
    // Session thread streams SessionUpdates over this channel; the effect
    // below turns them into signal writes on the UI thread.
    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<SessionUpdate>(256);
    let update_sig = create_signal_from_channel(update_rx);
    {
        let st = state.clone();
        create_effect(move |_| {
            if let Some(update) = update_sig.get() {
                match update {
                    SessionUpdate::Status(s2) => {
                        st.project.debug_status.set(s2);
                        if s2 == DebugStatus::Idle {
                            st.project.debug_cmd.set(None);
                        }
                    }
                    SessionUpdate::StoppedAt(at) => st.project.debug_stopped_at.set(at),
                    SessionUpdate::Frames(f) => st.project.debug_frames.set(f),
                    SessionUpdate::Vars(v) => st.project.debug_vars.set(v),
                    SessionUpdate::Output(o) => {
                        append_debug_console(st.workbench.debug_console_log, o.trim_end());
                    }
                }
            }
        });
    }

    // Forward breakpoint edits (F9 in the editor) to the live session.
    // Returns the per-file map so the next run can clear files that lost
    // their last breakpoint.
    {
        let st = state.clone();
        create_effect(move |prev: Option<HashMap<PathBuf, Vec<u64>>>| {
            let mut by_file: HashMap<PathBuf, Vec<u64>> = HashMap::new();
            for (path, line) in st.project.breakpoints.get() {
                by_file.entry(path).or_default().push(line);
            }
            if let Some(tx) = st.project.debug_cmd.get_untracked() {
                let mut files: HashSet<PathBuf> = by_file.keys().cloned().collect();
                if let Some(prev) = &prev {
                    files.extend(prev.keys().cloned());
                }
                for file in files {
                    let lines = by_file.get(&file).cloned().unwrap_or_default();
                    let _ = tx.send(DebugCmd::SetBreakpoints(file, lines));
                }
            }
            by_file
        });
    }

    let dbg_status = state.project.debug_status;
    let program_input = create_rw_signal(default_program(&root.get_untracked()));
    {
        // Refresh the suggested program path when the workspace changes.
        create_effect(move |_| {
            let ws = root.get();
            if program_input.get_untracked().is_empty() {
                program_input.set(default_program(&ws));
            }
        });
    }

    // Start (or stop) the debug session.
    let start_stop = {
        let st = state.clone();
        move |_: &floem::event::Event| {
            if dbg_status.get_untracked() != DebugStatus::Idle {
                if let Some(tx) = st.project.debug_cmd.get_untracked() {
                    let _ = tx.send(DebugCmd::Stop);
                }
                return;
            }
            let program = program_input.get_untracked().trim().to_string();
            if program.is_empty() || !Path::new(&program).is_file() {
                show_toast(
                    st.workbench.status_toast,
                    "Debug: set a valid program path (build first?)",
                );
                return;
            }
            let Some((adapter_cmd, adapter_args)) = detect_adapter() else {
                show_toast(
                    st.workbench.status_toast,
                    "No DAP adapter found — install lldb-dap (LLVM) or gdb ≥ 14",
                );
                return;
            };
            let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<DebugCmd>();
            st.project.debug_cmd.set(Some(cmd_tx));
            st.project.debug_status.set(DebugStatus::Running);
            st.workbench.show_bottom_panel.set(true);
            st.workbench.bottom_panel_tab.set(Tab::DebugConsole);
            let bps = st.project.breakpoints.get_untracked();
            let cwd = root.get_untracked();
            let utx = update_tx.clone();
            std::thread::spawn(move || {
                run_session(adapter_cmd, adapter_args, program, cwd, bps, cmd_rx, utx);
            });
        }
    };

    // Small bordered action button used across the debug toolbar.
    let dbg_btn = move |text: &'static str, cmd: DebugCmd, st: IdeState| {
        container(label(move || text.to_string()).style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .padding_horiz(7.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .border(1.0)
                .border_color(p.border)
                .color(p.accent)
                .cursor(floem::style::CursorStyle::Pointer)
        }))
        .on_click_stop(move |_| {
            if let Some(tx) = st.project.debug_cmd.get_untracked() {
                let _ = tx.send(cmd.clone());
            }
        })
    };

    let debug_section = {
        let st = state.clone();
        let section_header = label(|| "DEBUG".to_string()).style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .font_weight(floem::text::Weight::BOLD)
                .color(p.text_muted)
                .padding_horiz(12.0)
                .padding_top(8.0)
        });

        let program_row = h_stack((
            text_input(program_input)
                .placeholder("Path to debug target (e.g. target/debug/app)")
                .style(move |s| {
                    let p = theme.get().palette;
                    s.flex_grow(1.0)
                        .min_width(0.0)
                        .font_size(11.0)
                        .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                        .color(p.text_primary)
                        .background(p.bg_elevated)
                        .border(1.0)
                        .border_color(p.border)
                        .border_radius(4.0)
                        .padding_horiz(6.0)
                        .padding_vert(3.0)
                }),
            container(
                label(move || {
                    if dbg_status.get() == DebugStatus::Idle {
                        "Debug ▶".to_string()
                    } else {
                        "Stop ⏹".to_string()
                    }
                })
                .style(move |s| {
                    let p = theme.get().palette;
                    let active = dbg_status.get() != DebugStatus::Idle;
                    s.font_size(11.0)
                        .padding_horiz(8.0)
                        .padding_vert(3.0)
                        .border_radius(4.0)
                        .border(1.0)
                        .border_color(p.border)
                        .color(if active { p.git_deleted } else { p.accent })
                        .cursor(floem::style::CursorStyle::Pointer)
                }),
            )
            .on_click_stop(move |e| start_stop(e)),
        ))
        .style(|s| {
            s.items_center()
                .gap(6.0)
                .padding_horiz(12.0)
                .padding_vert(4.0)
                .width_full()
        });

        let toolbar = h_stack((
            dbg_btn("Continue F5", DebugCmd::Continue, st.clone()),
            dbg_btn("Over F10", DebugCmd::StepOver, st.clone()),
            dbg_btn("Into F11", DebugCmd::StepIn, st.clone()),
            dbg_btn("Out ⇧F11", DebugCmd::StepOut, st.clone()),
        ))
        .style(move |s| {
            s.gap(4.0)
                .padding_horiz(12.0)
                .padding_vert(2.0)
                .apply_if(dbg_status.get() != DebugStatus::Stopped, |s| {
                    s.display(floem::style::Display::None)
                })
        });

        let frames_sig = state.project.debug_frames;
        let st_frames = state.clone();
        let stack_list = v_stack((
            label(|| "CALL STACK".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(10.0)
                    .font_weight(floem::text::Weight::BOLD)
                    .color(p.text_muted)
                    .padding_horiz(12.0)
                    .padding_top(6.0)
            }),
            scroll(
                dyn_stack(
                    move || frames_sig.get(),
                    |f| f.0,
                    move |(_id, name, file, line)| {
                        let short = Path::new(&file)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let text = if short.is_empty() {
                            name.clone()
                        } else {
                            format!("{name} — {short}:{line}")
                        };
                        let open_path = PathBuf::from(&file);
                        let st2 = st_frames.clone();
                        label(move || text.clone())
                            .style(move |s| {
                                let p = theme.get().palette;
                                s.font_size(11.0)
                                    .color(p.text_primary)
                                    .padding_horiz(16.0)
                                    .padding_vert(2.0)
                                    .width_full()
                                    .cursor(floem::style::CursorStyle::Pointer)
                                    .hover(|s| s.background(p.bg_elevated))
                            })
                            .on_click_stop(move |_| {
                                if open_path.is_file() {
                                    st2.editor.open_file.set(Some(open_path.clone()));
                                    st2.editor.goto_line.set(line as u32);
                                }
                            })
                    },
                )
                .style(|s| s.flex_col().width_full()),
            )
            .style(|s| s.max_height(140.0).width_full()),
        ))
        .style(move |s| {
            s.width_full()
                .apply_if(dbg_status.get() != DebugStatus::Stopped, |s| {
                    s.display(floem::style::Display::None)
                })
        });

        let vars_sig = state.project.debug_vars;
        let vars_list = v_stack((
            label(|| "VARIABLES".to_string()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(10.0)
                    .font_weight(floem::text::Weight::BOLD)
                    .color(p.text_muted)
                    .padding_horiz(12.0)
                    .padding_top(6.0)
            }),
            scroll(
                dyn_stack(
                    move || vars_sig.get(),
                    |v| v.clone(),
                    move |(name, value, _ty)| {
                        label(move || format!("{name} = {value}")).style(move |s| {
                            let p = theme.get().palette;
                            s.font_size(10.5)
                                .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                                .color(p.text_primary)
                                .padding_horiz(16.0)
                                .padding_vert(1.0)
                                .width_full()
                        })
                    },
                )
                .style(|s| s.flex_col().width_full()),
            )
            .style(|s| s.max_height(180.0).width_full()),
        ))
        .style(move |s| {
            s.width_full()
                .apply_if(dbg_status.get() != DebugStatus::Stopped, |s| {
                    s.display(floem::style::Display::None)
                })
        });

        v_stack((section_header, program_row, toolbar, stack_list, vars_list)).style(move |s| {
            let p = theme.get().palette;
            s.width_full()
                .flex_col()
                .border_bottom(1.0)
                .border_color(p.border.with_alpha(0.5))
                .padding_bottom(6.0)
        })
    };

    let explain = label(|| {
        "F9 toggles a breakpoint on the cursor line. Set the program path, hit Debug ▶ (needs lldb-dap or gdb 14+), then F5/F10/F11 to continue/step.".to_string()
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

    container(
        v_stack((header, debug_section, explain, list))
            .style(|s| s.width_full().height_full().flex_col()),
    )
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
