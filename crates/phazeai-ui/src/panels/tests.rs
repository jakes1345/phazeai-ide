//! Test Runner panel — runs `cargo test` and shows pass/fail per test.

use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, Decorators},
    IntoView,
};

use crate::domain_state::IdeState;

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Failed,
    Ignored,
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub name: String,
    pub status: TestStatus,
}

#[derive(Clone, Debug, Default)]
pub struct TestRunOutput {
    pub results: Vec<TestResult>,
    pub passed: usize,
    pub failed: usize,
    pub ignored: usize,
    pub raw: String,
    pub running: bool,
    pub error: Option<String>,
}

// ── Parser ────────────────────────────────────────────────────────────────────

fn parse_test_output(raw: &str) -> Vec<TestResult> {
    let mut results = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("test ") {
            if let Some(name_end) = rest.rfind(" ... ") {
                let name = rest[..name_end].to_string();
                let status_str = &rest[name_end + 5..];
                let status = if status_str.starts_with("ok") {
                    TestStatus::Passed
                } else if status_str.starts_with("FAILED") {
                    TestStatus::Failed
                } else if status_str.starts_with("ignored") {
                    TestStatus::Ignored
                } else {
                    continue;
                };
                results.push(TestResult { name, status });
            }
        }
    }
    results
}

// ── Panel ─────────────────────────────────────────────────────────────────────

pub fn tests_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let workspace_root = state.project.workspace_root;

    let output: RwSignal<TestRunOutput> = create_rw_signal(TestRunOutput::default());
    let is_running: RwSignal<bool> = create_rw_signal(false);

    // Channel for receiving test run results
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel::<TestRunOutput>(1);
    let result_sig = create_signal_from_channel(result_rx);

    create_effect(move |_| {
        if let Some(run_output) = result_sig.get() {
            is_running.set(false);
            output.set(run_output);
        }
    });

    // ── Run button ────────────────────────────────────────────────────────
    let run_btn = container(label(move || {
        if is_running.get() {
            "⏳ Running..."
        } else {
            "▶ Run Tests"
        }
    }))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let running = is_running.get();
        s.padding_horiz(14.0)
            .padding_vert(7.0)
            .border_radius(6.0)
            .cursor(floem::style::CursorStyle::Pointer)
            .background(if running { p.bg_elevated } else { p.accent })
            .color(if running { p.text_muted } else { p.bg_surface })
            .font_size(13.0)
            .hover(|s| s.background(p.accent_hover))
    })
    .on_click_stop(move |_| {
        if is_running.get_untracked() {
            return;
        }
        is_running.set(true);
        output.update(|o| {
            o.running = true;
            o.error = None;
        });

        let root = workspace_root.get_untracked();
        let tx = result_tx.clone();

        std::thread::spawn(move || {
            let result = std::process::Command::new("cargo")
                .args(["test", "--workspace", "--", "--test-output", "immediate"])
                .current_dir(&root)
                .output();

            match result {
                Ok(out) => {
                    let raw = format!(
                        "{}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );
                    let results = parse_test_output(&raw);
                    let passed = results
                        .iter()
                        .filter(|r| r.status == TestStatus::Passed)
                        .count();
                    let failed = results
                        .iter()
                        .filter(|r| r.status == TestStatus::Failed)
                        .count();
                    let ignored = results
                        .iter()
                        .filter(|r| r.status == TestStatus::Ignored)
                        .count();
                    let _ = tx.send(TestRunOutput {
                        results,
                        passed,
                        failed,
                        ignored,
                        raw,
                        running: false,
                        error: None,
                    });
                }
                Err(e) => {
                    let _ = tx.send(TestRunOutput {
                        error: Some(format!("Failed to run cargo test: {e}")),
                        running: false,
                        ..Default::default()
                    });
                }
            }
        });
    });

    // ── Summary bar ───────────────────────────────────────────────────────
    let summary = container(label(move || {
        let o = output.get();
        if o.results.is_empty() && o.error.is_none() {
            return "No tests run yet".to_string();
        }
        if let Some(ref err) = o.error {
            return format!("Error: {err}");
        }
        format!(
            "✓ {} passed  ✗ {} failed  ○ {} ignored",
            o.passed, o.failed, o.ignored
        )
    }))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let o = output.get();
        let color = if o.failed > 0 {
            p.error
        } else if o.passed > 0 {
            p.success
        } else {
            p.text_muted
        };
        s.padding_horiz(10.0)
            .padding_vert(6.0)
            .font_size(12.0)
            .color(color)
            .border_bottom(1.0)
            .border_color(p.border)
            .width_full()
    });

    // ── Test result rows ──────────────────────────────────────────────────
    let results_list = dyn_stack(
        move || {
            let o = output.get();
            o.results.into_iter().enumerate().collect::<Vec<_>>()
        },
        |(i, _)| *i,
        move |(_, result)| {
            let name = result.name.clone();
            let name_copy = result.name.clone();
            let status = result.status.clone();
            let is_hovered = create_rw_signal(false);
            let status_for_icon = status.clone();
            let status_for_color = status.clone();

            container(
                stack((
                    label(move || match status_for_icon {
                        TestStatus::Passed => "✓",
                        TestStatus::Failed => "✗",
                        TestStatus::Ignored => "○",
                    })
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        let color = match status_for_color {
                            TestStatus::Passed => p.success,
                            TestStatus::Failed => p.error,
                            TestStatus::Ignored => p.text_muted,
                        };
                        s.font_size(12.0).color(color).margin_right(8.0).width(14.0)
                    }),
                    label(move || name.clone()).style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        s.font_size(12.0).color(p.text_primary).flex_grow(1.0)
                    }),
                ))
                .style(|s| s.items_center()),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(10.0)
                    .padding_vert(4.0)
                    .width_full()
                    .cursor(floem::style::CursorStyle::Pointer)
                    .background(if is_hovered.get() {
                        p.bg_elevated
                    } else {
                        floem::peniko::Color::TRANSPARENT
                    })
            })
            .on_click_stop(move |_| {
                // Copy test name to clipboard on click
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(name_copy.clone());
                }
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                is_hovered.set(true);
            })
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
                is_hovered.set(false);
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    // ── Raw output (collapsed by default) ────────────────────────────────
    let show_raw = create_rw_signal(false);

    let raw_toggle = container(label(move || {
        if show_raw.get() {
            "▼ Hide raw output"
        } else {
            "▶ Show raw output"
        }
    }))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(10.0)
            .padding_vert(5.0)
            .font_size(11.0)
            .color(p.text_muted)
            .cursor(floem::style::CursorStyle::Pointer)
            .border_top(1.0)
            .border_color(p.border)
            .width_full()
            .hover(|s| s.background(p.bg_elevated))
    })
    .on_click_stop(move |_| show_raw.update(|v| *v = !*v));

    let raw_output = container(
        scroll(label(move || output.get().raw).style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.font_size(11.0)
                .color(p.text_muted)
                .font_family("Monospace".to_string())
                .padding(8.0)
        }))
        .style(|s| s.width_full().max_height(200.0)),
    )
    .style(move |s| {
        if show_raw.get() {
            s.width_full()
        } else {
            s.display(floem::style::Display::None)
        }
    });

    // ── Header ────────────────────────────────────────────────────────────
    let header = container(
        stack((
            label(|| "TESTS").style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.color(p.text_muted)
                    .font_size(11.0)
                    .font_weight(floem::text::Weight::BOLD)
                    .flex_grow(1.0)
            }),
            run_btn,
        ))
        .style(|s| s.items_center().gap(8.0)),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(10.0)
            .padding_vert(8.0)
            .border_bottom(1.0)
            .border_color(p.border)
            .width_full()
    });

    stack((
        header,
        summary,
        scroll(results_list).style(|s| s.flex_grow(1.0).min_height(0.0).width_full()),
        raw_toggle,
        raw_output,
    ))
    .style(|s| s.flex_col().width_full().height_full())
}
