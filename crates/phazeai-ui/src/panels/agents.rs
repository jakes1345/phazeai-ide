//! Agents panel: run coding agents side by side, each in its own git
//! worktree, then merge or drop what each one made; and have the AI review a
//! change (your uncommitted work, your branch, or an agent's worktree) with
//! findings that open at their line.
//!
//! The agents run through the terminal app (`phazeai worktree run`), one
//! process per task, because the agent's file sandbox is one per process: a
//! worktree agent needs its own process to be fenced into its own checkout.
//! Listing, merging, removing and reviewing happen here, with the same code
//! the terminal app uses ([`phazeai_core::worktree`] and
//! [`phazeai_core::code_review`]).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate, SignalWith},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::code_review::{self, ChangedFile, DiffSource, Finding, Severity};
use phazeai_core::worktree::{self, Worktree, WorktreeState};
use phazeai_core::Settings;

use crate::app::{show_toast, Tab};
use crate::components::button::{phaze_button, ButtonVariant};
use crate::domain_state::IdeState;
use crate::theme::PhazeTheme;
use crate::tokens::{font, radius, space, stroke};
use crate::util::shell_quote_single;

/// The terminal app's executable name.
const CLI_NAME: &str = if cfg!(windows) {
    "phazeai.exe"
} else {
    "phazeai"
};

/// The terminal app: next to this executable first (install.sh and `cargo
/// build` put both there), then on `path` (the `PATH` variable).
pub(crate) fn find_cli(exe_dir: Option<&Path>, path: Option<&OsStr>) -> Option<PathBuf> {
    let beside = exe_dir.map(|d| d.join(CLI_NAME));
    let on_path = path
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|d| d.join(CLI_NAME));
    beside.into_iter().chain(on_path).find(|p| p.is_file())
}

/// The directory of the running executable, links resolved (the IDE is often
/// started through the `phazeai-ide` link).
fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    exe.parent().map(Path::to_path_buf)
}

/// The shell line that runs one agent per task in `repo`. None without a
/// task.
pub(crate) fn run_command(repo: &Path, cli: &Path, tasks: &[String]) -> Option<String> {
    let tasks: Vec<&str> = tasks
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if tasks.is_empty() {
        return None;
    }
    let mut line = format!(
        "cd {} && {} worktree run",
        shell_quote_single(&repo.to_string_lossy()),
        shell_quote_single(&cli.to_string_lossy())
    );
    for t in tasks {
        line.push_str(" --task ");
        line.push_str(&shell_quote_single(t));
    }
    Some(line)
}

/// What a worktree holds, in a few words.
pub(crate) fn describe_state(s: &WorktreeState) -> String {
    if s.is_empty() {
        return "Nothing new yet".into();
    }
    let mut parts = Vec::new();
    if s.commits_ahead > 0 {
        parts.push(format!(
            "{} commit{} to merge",
            s.commits_ahead,
            plural(s.commits_ahead)
        ));
    }
    if !s.changed_files.is_empty() {
        parts.push(format!(
            "{} file{} changed",
            s.changed_files.len(),
            plural(s.changed_files.len())
        ));
    }
    if !s.uncommitted.is_empty() {
        parts.push(format!("{} not committed", s.uncommitted.len()));
    }
    parts.join(" · ")
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// A worktree's checkout is outside the project, where the project's privacy
/// policy can't see it. Judge each file by its place in the project instead,
/// so a file the policy denies is never sent from a worktree either.
pub(crate) fn skip_private(files: &mut [ChangedFile], repo: &Path, denied: impl Fn(&Path) -> bool) {
    for f in files {
        if f.skipped.is_none() && denied(&repo.join(&f.path)) {
            f.skipped = Some("private (privacy policy)".into());
        }
    }
}

/// One finished review, ready to show.
#[derive(Clone, Debug)]
pub(crate) struct ReviewOutcome {
    /// Where the reviewed files are; a finding's file is relative to it.
    pub root: PathBuf,
    pub findings: Vec<Finding>,
    pub summary: String,
}

/// Review `source` in the checkout at `root` with the model in `settings`.
/// `repo` is the project the checkout belongs to (the same as `root` unless
/// it is an agent's worktree); `what` names the change for the summary.
pub(crate) async fn review_change(
    settings: &Settings,
    repo: &Path,
    root: &Path,
    source: DiffSource,
    what: &str,
) -> Result<ReviewOutcome, String> {
    let mut files = code_review::collect(root, &source).map_err(|e| e.to_string())?;
    if root != repo {
        let policy = phazeai_core::privacy::active();
        skip_private(&mut files, repo, |p| policy.is_denied(p));
    }
    let skipped = files.iter().filter(|f| f.skipped.is_some()).count();
    if files.iter().all(|f| f.skipped.is_some()) {
        let mut summary = format!("Nothing to review in {what}.");
        if skipped > 0 {
            summary.push_str(&format!(
                " {skipped} file{} skipped (generated, lock or private).",
                plural(skipped)
            ));
        }
        return Ok(ReviewOutcome {
            root: root.to_path_buf(),
            findings: Vec::new(),
            summary,
        });
    }
    let client = settings
        .build_llm_client()
        .map_err(|e| format!("AI provider isn't ready: {e}. Open Settings to set one up."))?;
    let billed = settings.billed_model();
    let review = code_review::review(
        client.as_ref(),
        billed.as_ref().map(|(p, m)| (p, m.as_str())),
        &files,
        None,
    )
    .await;
    let n = review.findings.len();
    let mut summary = if n == 0 {
        format!("No problems found in {what}")
    } else {
        format!("{n} finding{} in {what}", plural(n))
    };
    summary.push_str(&format!(
        " ({} file{} reviewed, {}).",
        review.files_reviewed.len(),
        plural(review.files_reviewed.len()),
        review.meter.summary()
    ));
    if skipped > 0 {
        summary.push_str(&format!(" {skipped} skipped."));
    }
    for e in &review.errors {
        summary.push_str(&format!(" Error: {e}"));
    }
    Ok(ReviewOutcome {
        root: root.to_path_buf(),
        findings: review.findings,
        summary,
    })
}

/// The commit the main checkout is on.
fn head_commit(repo: &Path) -> Result<String, String> {
    let out = phazeai_core::process::std_command("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// One worktree as listed.
#[derive(Clone, Debug)]
struct AgentRow {
    wt: Worktree,
    state: Result<WorktreeState, String>,
}

fn list_rows(repo: &Path) -> Result<Vec<AgentRow>, String> {
    let wts = worktree::list(repo).map_err(|e| e.to_string())?;
    Ok(wts
        .into_iter()
        .map(|wt| AgentRow {
            state: worktree::state(repo, &wt).map_err(|e| e.to_string()),
            wt,
        })
        .collect())
}

/// What the review thread sends back.
#[derive(Clone, Debug)]
enum ReviewUpdate {
    Done(ReviewOutcome),
    Failed(String),
}

fn spawn_review(
    repo: PathBuf,
    root: PathBuf,
    source: DiffSource,
    what: String,
    tx: std::sync::mpsc::SyncSender<ReviewUpdate>,
) {
    std::thread::spawn(move || {
        let update = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => {
                let settings = Settings::load();
                match rt.block_on(review_change(&settings, &repo, &root, source, &what)) {
                    Ok(outcome) => ReviewUpdate::Done(outcome),
                    Err(e) => ReviewUpdate::Failed(e),
                }
            }
            Err(e) => ReviewUpdate::Failed(e.to_string()),
        };
        let _ = tx.send(update);
    });
}

fn section_title(text: &'static str, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    label(move || text.to_string()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(10.5)
            .font_weight(floem::text::Weight::BOLD)
            .color(p.text_muted)
            .padding_horiz(space::LG)
            .padding_top(space::LG)
            .padding_bottom(space::SM)
    })
}

fn note(text: impl Fn() -> String + 'static, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    label(text).style(move |s| {
        let p = theme.get().palette;
        s.font_size(font::CAPTION)
            .color(p.text_muted)
            .padding_horiz(space::LG)
            .padding_bottom(space::SM)
            .width_full()
    })
}

/// A small text button.
fn small_button(
    text: impl Fn() -> String + 'static,
    theme: RwSignal<PhazeTheme>,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    container(label(text).style(move |s| {
        let p = theme.get().palette;
        s.font_size(font::CAPTION)
            .padding_horiz(space::MD)
            .padding_vert(3.0)
            .border_radius(radius::SM)
            .border(stroke::HAIRLINE)
            .border_color(p.border)
            .color(p.text_primary)
            .cursor(floem::style::CursorStyle::Pointer)
    }))
    .on_click_stop(move |_| on_click())
}

pub fn agents_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let root_sig = state.project.workspace_root;
    let toast = state.workbench.status_toast;

    let rows: RwSignal<Vec<AgentRow>> = create_rw_signal(Vec::new());
    let list_error = create_rw_signal(String::new());
    let tasks: RwSignal<Vec<String>> = create_rw_signal(Vec::new());
    let new_task = create_rw_signal(String::new());
    let run_note = create_rw_signal(String::new());
    let confirm_remove = create_rw_signal(None::<String>);
    let reviewing = create_rw_signal(None::<String>);
    let review_status = create_rw_signal(String::new());
    let findings: RwSignal<Vec<(PathBuf, Finding)>> = create_rw_signal(Vec::new());

    let repo = move || worktree::repo_root(&root_sig.get_untracked()).ok();

    let refresh = move || {
        confirm_remove.set(None);
        match worktree::repo_root(&root_sig.get_untracked()) {
            Ok(repo) => match list_rows(&repo) {
                Ok(r) => {
                    list_error.set(String::new());
                    rows.set(r);
                }
                Err(e) => {
                    rows.set(Vec::new());
                    list_error.set(e);
                }
            },
            Err(e) => {
                rows.set(Vec::new());
                list_error.set(format!("Agents need a git repository: {e}"));
            }
        }
    };
    // List again when the project changes and whenever the panel is opened,
    // since agents started from the terminal finish on their own time.
    {
        let tab = state.workbench.left_panel_tab;
        create_effect(move |_| {
            let _ = root_sig.get();
            if tab.get() == Tab::Agents {
                refresh();
            }
        });
    }

    // ── Reviews ─────────────────────────────────────────────────────────────
    let (tx, rx) = std::sync::mpsc::sync_channel::<ReviewUpdate>(4);
    let updates = create_signal_from_channel(rx);
    create_effect(move |_| {
        let Some(update) = updates.get() else {
            return;
        };
        reviewing.set(None);
        match update {
            ReviewUpdate::Done(outcome) => {
                review_status.set(outcome.summary.clone());
                findings.set(
                    outcome
                        .findings
                        .into_iter()
                        .map(|f| (outcome.root.clone(), f))
                        .collect(),
                );
            }
            ReviewUpdate::Failed(e) => review_status.set(format!("Review failed: {e}")),
        }
    });
    let start_review = {
        let tx = tx.clone();
        move |root: PathBuf, source: DiffSource, what: String| {
            if reviewing.get_untracked().is_some() {
                return;
            }
            let Some(repo) = repo() else {
                review_status.set("Reviews need a git repository.".into());
                return;
            };
            findings.set(Vec::new());
            review_status.set(format!("Reviewing {what}…"));
            reviewing.set(Some(what.clone()));
            spawn_review(repo, root, source, what, tx.clone());
        }
    };

    // ── Header ──────────────────────────────────────────────────────────────
    let header = stack((
        label(|| "AGENTS".to_string()).style(move |s| {
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
        .on_click_stop(move |_| refresh()),
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .width_full()
            .padding_horiz(space::LG)
            .padding_vert(space::MD)
            .border_bottom(stroke::HAIRLINE)
            .border_color(p.border)
    });

    // ── Run agents ──────────────────────────────────────────────────────────
    let add_task = move || {
        let t = new_task.get_untracked().trim().to_string();
        if t.is_empty() {
            return;
        }
        tasks.update(|v| v.push(t));
        new_task.set(String::new());
    };
    let task_input = text_input(new_task)
        .placeholder("A task for one agent, e.g. Add tests for the parser")
        .on_event_stop(floem::event::EventListener::KeyDown, move |e| {
            if let floem::event::Event::KeyDown(k) = e {
                if k.key.logical_key
                    == floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter)
                {
                    add_task();
                }
            }
        })
        .style(move |s| {
            let p = theme.get().palette;
            s.flex_grow(1.0)
                .min_width(0.0)
                .padding(space::MD)
                .font_size(font::BODY)
                .background(p.bg_surface)
                .color(p.text_primary)
                .border(stroke::HAIRLINE)
                .border_color(p.border)
                .border_radius(radius::MD)
        });
    let input_row = stack((
        task_input,
        phaze_button("Add", ButtonVariant::Secondary, theme, add_task),
    ))
    .style(|s| {
        s.flex_row()
            .gap(space::SM)
            .items_center()
            .width_full()
            .padding_horiz(space::LG)
    });

    let task_list = dyn_stack(
        move || tasks.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, t)| format!("{i}:{t}"),
        move |(i, t)| {
            stack((
                label(move || format!("{}. {t}", i + 1)).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(font::UI)
                        .color(p.text_primary)
                        .flex_grow(1.0)
                        .min_width(0.0)
                }),
                container(label(|| "×".to_string()).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(font::UI)
                        .color(p.text_muted)
                        .padding_horiz(space::SM)
                        .cursor(floem::style::CursorStyle::Pointer)
                }))
                .on_click_stop(move |_| {
                    tasks.update(|v| {
                        if i < v.len() {
                            v.remove(i);
                        }
                    })
                }),
            ))
            .style(|s| {
                s.flex_row()
                    .items_center()
                    .width_full()
                    .padding_horiz(space::LG)
                    .padding_vert(2.0)
            })
        },
    )
    .style(|s| s.flex_col().width_full().padding_vert(space::SM));

    let run_state = state.clone();
    let run_agents = move || {
        let list = tasks.get_untracked();
        let Some(repo) = repo() else {
            run_note.set("Agents need a git repository with at least one commit.".into());
            return;
        };
        let Some(cli) = find_cli(exe_dir().as_deref(), std::env::var_os("PATH").as_deref()) else {
            run_note.set(
                "The phazeai terminal app isn't installed next to the IDE or on your PATH. \
                 install.sh, or the phazeai-cli package, installs it."
                    .into(),
            );
            return;
        };
        let Some(line) = run_command(&repo, &cli, &list) else {
            run_note.set("Add a task first.".into());
            return;
        };
        let st = &run_state;
        st.workbench.run_in_terminal_text.set(Some(line));
        st.workbench.show_bottom_panel.set(true);
        st.workbench.bottom_panel_tab.set(Tab::Terminal);
        run_note.set(format!(
            "Started {} agent{} in the terminal. Each works in its own worktree; \
             press Refresh when they finish.",
            list.len(),
            plural(list.len())
        ));
        tasks.set(Vec::new());
        show_toast(toast, "Agents started in the terminal");
    };
    let run_button = container(phaze_button(
        "Run agents",
        ButtonVariant::Primary,
        theme,
        run_agents,
    ))
    .style(move |s| {
        s.padding_horiz(space::LG)
            .apply_if(tasks.with(|t| t.is_empty()), |s| {
                s.display(floem::style::Display::None)
            })
    });

    // ── Worktrees ───────────────────────────────────────────────────────────
    let list_err = label(move || list_error.get()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(font::CAPTION)
            .color(p.warning)
            .padding_horiz(space::LG)
            .padding_bottom(space::SM)
            .apply_if(list_error.with(|e| e.is_empty()), |s| {
                s.display(floem::style::Display::None)
            })
    });
    let empty = note(
        || "No agent worktrees yet. Add tasks above and run them.".to_string(),
        theme,
    )
    .style(move |s| {
        s.apply_if(
            !rows.with(|r| r.is_empty()) || !list_error.with(|e| e.is_empty()),
            |s| s.display(floem::style::Display::None),
        )
    });

    let row_state = state.clone();
    let start_review_wt = start_review.clone();
    let worktree_rows = dyn_stack(
        move || rows.get(),
        |r| format!("{}:{:?}", r.wt.name, r.state),
        move |row| {
            let st = row_state.clone();
            let name = row.wt.name.clone();
            let has_work = row.state.as_ref().map(|s| !s.is_empty()).unwrap_or(true);
            let can_merge = row
                .state
                .as_ref()
                .map(|s| s.commits_ahead > 0)
                .unwrap_or(false);
            let line2 = match &row.state {
                Ok(s) => describe_state(s),
                Err(e) => format!("Couldn't read it: {e}"),
            };
            let files = row
                .state
                .as_ref()
                .map(|s| {
                    let mut f = s.changed_files.iter().take(4).cloned().collect::<Vec<_>>();
                    if s.changed_files.len() > 4 {
                        f.push(format!("+{} more", s.changed_files.len() - 4));
                    }
                    f.join(", ")
                })
                .unwrap_or_default();
            let files_empty = files.is_empty();
            let title = format!("{} · {}", row.wt.name, row.wt.branch);

            let wt_merge = row.wt.clone();
            let merge = small_button(
                || "Merge".to_string(),
                theme,
                move || {
                    let Some(repo) = repo() else { return };
                    match worktree::merge(&repo, &wt_merge) {
                        Ok(()) => {
                            let removed = worktree::remove(&repo, &wt_merge, false);
                            let msg = match removed {
                                Ok(()) => {
                                    format!("Merged {} and removed its worktree.", wt_merge.name)
                                }
                                Err(e) => {
                                    format!("Merged {}; the worktree stays: {e}", wt_merge.name)
                                }
                            };
                            show_toast(toast, msg);
                        }
                        Err(e) => show_toast(toast, format!("Not merged: {e}")),
                    }
                    refresh();
                },
            );

            let wt_review = row.wt.clone();
            let start_review = start_review_wt.clone();
            let review = small_button(
                || "Review".to_string(),
                theme,
                move || {
                    let Some(repo) = repo() else { return };
                    match head_commit(&repo) {
                        Ok(base) => start_review(
                            wt_review.path.clone(),
                            DiffSource::Branch { base },
                            format!("the agent worktree {}", wt_review.name),
                        ),
                        Err(e) => review_status.set(format!("Review failed: {e}")),
                    }
                },
            );

            let wt_open = row.wt.clone();
            let open = small_button(
                || "Terminal".to_string(),
                theme,
                move || {
                    let line =
                        format!("cd {}", shell_quote_single(&wt_open.path.to_string_lossy()));
                    st.workbench.run_in_terminal_text.set(Some(line));
                    st.workbench.show_bottom_panel.set(true);
                    st.workbench.bottom_panel_tab.set(Tab::Terminal);
                },
            );

            let wt_remove = row.wt.clone();
            let name_for_label = name.clone();
            let remove = small_button(
                move || {
                    if confirm_remove.with(|c| c.as_deref() == Some(name_for_label.as_str())) {
                        if has_work {
                            "Delete unmerged work?".to_string()
                        } else {
                            "Confirm remove".to_string()
                        }
                    } else {
                        "Remove".to_string()
                    }
                },
                theme,
                move || {
                    if confirm_remove.get_untracked().as_deref() != Some(wt_remove.name.as_str()) {
                        confirm_remove.set(Some(wt_remove.name.clone()));
                        return;
                    }
                    let Some(repo) = repo() else { return };
                    match worktree::remove(&repo, &wt_remove, has_work) {
                        Ok(()) => show_toast(toast, format!("Removed {}.", wt_remove.name)),
                        Err(e) => show_toast(toast, format!("Not removed: {e}")),
                    }
                    refresh();
                },
            );

            stack((
                label(move || title.clone()).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(font::UI).color(p.text_primary).width_full()
                }),
                label(move || line2.clone()).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(font::CAPTION).color(p.text_muted).width_full()
                }),
                label(move || files.clone()).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(font::CAPTION)
                        .color(p.text_muted)
                        .width_full()
                        .apply_if(files_empty, |s| s.display(floem::style::Display::None))
                }),
                stack((
                    container(merge).style(move |s| {
                        s.apply_if(!can_merge, |s| s.display(floem::style::Display::None))
                    }),
                    container(review).style(move |s| {
                        s.apply_if(!can_merge, |s| s.display(floem::style::Display::None))
                    }),
                    open,
                    remove,
                ))
                .style(|s| s.flex_row().gap(space::SM).margin_top(space::SM)),
            ))
            .style(move |s| {
                let p = theme.get().palette;
                s.flex_col()
                    .width_full()
                    .padding_horiz(space::LG)
                    .padding_vert(space::MD)
                    .border_bottom(stroke::HAIRLINE)
                    .border_color(p.border.with_alpha(0.35))
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    // ── Review ──────────────────────────────────────────────────────────────
    let review_buttons = stack((
        phaze_button("Review my changes", ButtonVariant::Secondary, theme, {
            let start_review = start_review.clone();
            move || {
                let Some(repo) = repo() else {
                    review_status.set("Reviews need a git repository.".into());
                    return;
                };
                start_review(
                    repo,
                    DiffSource::Uncommitted,
                    "your uncommitted changes".into(),
                )
            }
        }),
        phaze_button("Review this branch", ButtonVariant::Secondary, theme, {
            let start_review = start_review.clone();
            move || {
                let Some(repo) = repo() else {
                    review_status.set("Reviews need a git repository.".into());
                    return;
                };
                match code_review::default_base(&repo) {
                    Some(base) => {
                        let what = format!("this branch's commits not in {base}");
                        start_review(repo, DiffSource::Branch { base }, what)
                    }
                    None => review_status
                        .set("Couldn't find a main branch to compare this branch with.".into()),
                }
            }
        }),
    ))
    .style(move |s| {
        s.flex_row()
            .gap(space::SM)
            .padding_horiz(space::LG)
            .apply_if(reviewing.with(|r| r.is_some()), |s| {
                s.display(floem::style::Display::None)
            })
    });
    let review_line = note(move || review_status.get(), theme).style(move |s| {
        s.padding_top(space::SM)
            .apply_if(review_status.with(|r| r.is_empty()), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let finding_state = state.clone();
    let finding_rows = dyn_stack(
        move || findings.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, (_, f))| format!("{i}:{}:{}:{}", f.file, f.line, f.title),
        move |(_, (root, f))| {
            let st = finding_state.clone();
            let sev = f.severity;
            let head = format!("{} · {}", f.severity.as_str().to_uppercase(), f.title);
            let place = format!("{}:{}", f.file, f.line);
            let detail = f.detail.clone();
            let detail_empty = detail.is_empty();
            let fix = f
                .fix
                .clone()
                .map(|x| format!("Fix: {x}"))
                .unwrap_or_default();
            let fix_empty = fix.is_empty();
            let path = root.join(&f.file);
            let line = f.line;
            stack((
                label(move || head.clone()).style(move |s| {
                    let p = theme.get().palette;
                    let c = match sev {
                        Severity::High => p.error,
                        Severity::Medium => p.warning,
                        Severity::Low => p.text_muted,
                    };
                    s.font_size(font::UI).color(c).width_full()
                }),
                label(move || place.clone()).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(font::CAPTION).color(p.accent).width_full()
                }),
                label(move || detail.clone()).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(font::CAPTION)
                        .color(p.text_primary)
                        .width_full()
                        .apply_if(detail_empty, |s| s.display(floem::style::Display::None))
                }),
                label(move || fix.clone()).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(font::CAPTION)
                        .color(p.text_muted)
                        .width_full()
                        .apply_if(fix_empty, |s| s.display(floem::style::Display::None))
                }),
            ))
            .on_click_stop(move |_| {
                st.editor.open_file.set(Some(path.clone()));
                st.editor.goto_line.set(line);
            })
            .style(move |s| {
                let p = theme.get().palette;
                s.flex_col()
                    .width_full()
                    .padding_horiz(space::LG)
                    .padding_vert(space::MD)
                    .border_bottom(stroke::HAIRLINE)
                    .border_color(p.border.with_alpha(0.35))
                    .cursor(floem::style::CursorStyle::Pointer)
                    .hover(|s| s.background(p.bg_elevated))
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    let body = stack((
        section_title("RUN AGENTS SIDE BY SIDE", theme),
        note(
            || {
                "Each task gets its own agent in its own git worktree, so they can't step on \
                 each other or on your files. Merge what you like."
                    .to_string()
            },
            theme,
        ),
        input_row,
        task_list,
        run_button,
        note(move || run_note.get(), theme).style(move |s| {
            s.padding_top(space::SM)
                .apply_if(run_note.with(|n| n.is_empty()), |s| {
                    s.display(floem::style::Display::None)
                })
        }),
        section_title("WORKTREES", theme),
        list_err,
        empty,
        worktree_rows,
        section_title("AI REVIEW", theme),
        review_buttons,
        review_line,
        finding_rows,
    ))
    .style(|s| s.flex_col().width_full().padding_bottom(space::LG));

    stack((
        header,
        scroll(body).style(|s| s.flex_grow(1.0).min_height(0.0).width_full()),
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width_full()
            .height_full()
            .background(p.glass_bg)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use phazeai_core::config::LlmProvider;
    use std::io::{Read, Write};
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repository with one committed file.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(
            dir.path().join("lib.rs"),
            "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn the_cli_beside_the_ide_wins_over_the_path() {
        let beside = tempfile::tempdir().unwrap();
        let on_path = tempfile::tempdir().unwrap();
        std::fs::write(on_path.path().join(CLI_NAME), "").unwrap();
        let path = std::env::join_paths([on_path.path()]).unwrap();
        assert_eq!(
            find_cli(Some(beside.path()), Some(&path)),
            Some(on_path.path().join(CLI_NAME))
        );
        std::fs::write(beside.path().join(CLI_NAME), "").unwrap();
        assert_eq!(
            find_cli(Some(beside.path()), Some(&path)),
            Some(beside.path().join(CLI_NAME))
        );
        assert_eq!(find_cli(None, None), None);
    }

    #[test]
    fn each_task_is_one_quoted_argument() {
        let line = run_command(
            Path::new("/work/my repo"),
            Path::new("/opt/bin/phazeai"),
            &[
                "Fix the parser's bug".to_string(),
                "  ".to_string(),
                "Add tests; then $(rm -rf /)".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(
            line,
            "cd '/work/my repo' && '/opt/bin/phazeai' worktree run \
             --task 'Fix the parser'\\''s bug' --task 'Add tests; then $(rm -rf /)'"
        );
        assert_eq!(
            run_command(Path::new("/r"), Path::new("/p"), &[" ".into()]),
            None
        );
    }

    /// The command line really runs as one agent per task under a POSIX
    /// shell: every task arrives as one argument, quotes and all.
    #[cfg(unix)]
    #[test]
    fn the_shell_passes_each_task_whole() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("fake cli");
        let args_file = dir.path().join("args");
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'\npwd >> '{}'\n",
                args_file.display(),
                args_file.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let work = dir.path().join("my repo");
        std::fs::create_dir(&work).unwrap();
        let line = run_command(
            &work,
            &cli,
            &["it's done".into(), "a && b; $(echo no)".into()],
        )
        .unwrap();
        let status = Command::new("sh").arg("-c").arg(&line).status().unwrap();
        assert!(status.success());
        let got = std::fs::read_to_string(&args_file).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(
            &lines[..6],
            [
                "worktree",
                "run",
                "--task",
                "it's done",
                "--task",
                "a && b; $(echo no)"
            ]
        );
        assert_eq!(
            Path::new(lines[6]).canonicalize().unwrap(),
            work.canonicalize().unwrap()
        );
    }

    #[test]
    fn a_worktree_state_reads_as_a_short_line() {
        assert_eq!(describe_state(&WorktreeState::default()), "Nothing new yet");
        let s = WorktreeState {
            uncommitted: vec![" M a.rs".into()],
            commits_ahead: 1,
            changed_files: vec!["a.rs".into(), "b.rs".into()],
        };
        assert_eq!(
            describe_state(&s),
            "1 commit to merge · 2 files changed · 1 not committed"
        );
    }

    #[test]
    fn a_worktree_file_is_judged_by_its_place_in_the_project() {
        let mut files = code_review::parse_unified(
            "diff --git a/secrets/key.rs b/secrets/key.rs\n--- a/secrets/key.rs\n+++ b/secrets/key.rs\n@@ -1 +1 @@\n-a\n+b\n\
             diff --git a/src/ok.rs b/src/ok.rs\n--- a/src/ok.rs\n+++ b/src/ok.rs\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert_eq!(files.len(), 2);
        let repo = Path::new("/project");
        skip_private(&mut files, repo, |p| p.starts_with("/project/secrets"));
        assert_eq!(
            files[0].skipped.as_deref(),
            Some("private (privacy policy)")
        );
        assert_eq!(files[1].skipped, None);
    }

    /// A one-request Ollama server on 127.0.0.1 that answers `reply` and
    /// hands back the request body it got.
    fn ollama(reply: String) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let body = loop {
                let n = sock.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if buf.len() >= end + 4 + len {
                    break String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).into_owned();
                }
            };
            let line = serde_json::json!({
                "model": "m",
                "created_at": "2026-01-01T00:00:00Z",
                "message": {"role": "assistant", "content": reply},
                "done": true,
            })
            .to_string()
                + "\n";
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{line}",
                line.len()
            );
            body
        });
        (address, handle)
    }

    #[tokio::test]
    async fn a_review_finds_problems_in_uncommitted_work_at_their_line() {
        let dir = repo();
        std::fs::write(
            dir.path().join("lib.rs"),
            "fn add(a: i32, b: i32) -> i32 {\n    a - b\n}\n",
        )
        .unwrap();
        let reply = serde_json::json!({"findings": [{
            "file": "lib.rs", "line": 2, "severity": "high",
            "title": "add subtracts", "detail": "a - b returns the difference",
            "fix": "a + b"
        }]})
        .to_string();
        let (server, request) = ollama(reply);
        let mut settings = Settings::default();
        settings.llm.provider = LlmProvider::Ollama;
        settings.llm.model = "qwen2.5-coder:7b".into();
        settings.llm.base_url = Some(server);

        let outcome = review_change(
            &settings,
            dir.path(),
            dir.path(),
            DiffSource::Uncommitted,
            "your uncommitted changes",
        )
        .await
        .expect("the review runs");

        let sent = request.join().unwrap();
        assert!(sent.contains("a - b"), "the diff went to the model: {sent}");
        assert_eq!(outcome.findings.len(), 1);
        let f = &outcome.findings[0];
        assert_eq!((f.file.as_str(), f.line), ("lib.rs", 2));
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.fix.as_deref(), Some("a + b"));
        assert_eq!(outcome.root, dir.path());
        assert!(
            outcome
                .summary
                .starts_with("1 finding in your uncommitted changes (1 file reviewed"),
            "{}",
            outcome.summary
        );
    }

    #[tokio::test]
    async fn nothing_to_review_asks_no_model() {
        let dir = repo();
        let mut settings = Settings::default();
        settings.llm.provider = LlmProvider::Ollama;
        // Nothing listens here; a request would fail the review.
        settings.llm.base_url = Some("http://127.0.0.1:9".into());
        let outcome = review_change(
            &settings,
            dir.path(),
            dir.path(),
            DiffSource::Uncommitted,
            "your uncommitted changes",
        )
        .await
        .unwrap();
        assert!(outcome.findings.is_empty());
        assert_eq!(
            outcome.summary,
            "Nothing to review in your uncommitted changes."
        );
    }

    #[test]
    fn an_agent_worktree_lists_with_what_it_made() {
        let dir = repo();
        let wts = tempfile::tempdir().unwrap();
        std::env::set_var("PHAZEAI_WORKTREES_DIR", wts.path());
        let wt = worktree::create(dir.path(), "fix-add").unwrap();
        std::fs::write(wt.path.join("lib.rs"), "fn add() {}\n").unwrap();
        worktree::commit_all(&wt, "change add").unwrap();
        let rows = list_rows(dir.path()).unwrap();
        std::env::remove_var("PHAZEAI_WORKTREES_DIR");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].wt.name, "fix-add");
        let state = rows[0].state.as_ref().unwrap();
        assert_eq!(state.commits_ahead, 1);
        assert_eq!(state.changed_files, ["lib.rs"]);
        assert_eq!(describe_state(state), "1 commit to merge · 1 file changed");
    }
}
