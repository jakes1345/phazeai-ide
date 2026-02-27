//! Git Source Control panel for PhazeAI IDE.
//!
//! Shows staged changes, unstaged changes, and untracked files.
//! Provides commit message input, commit button, and file click-to-open.

use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};

use crate::{
    app::IdeState,
    components::icon::{icons, phaze_icon},
    theme::PhazeTheme,
};

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Untracked,
    Renamed,
}

#[derive(Clone, Debug)]
pub struct GitFileEntry {
    pub status: GitFileStatus,
    pub path: String,
    pub staged: bool,
}

impl GitFileEntry {
    fn badge(&self) -> &'static str {
        match self.status {
            GitFileStatus::Modified  => "M",
            GitFileStatus::Added     => "A",
            GitFileStatus::Deleted   => "D",
            GitFileStatus::Untracked => "U",
            GitFileStatus::Renamed   => "R",
        }
    }

    fn badge_color(&self) -> floem::peniko::Color {
        match self.status {
            GitFileStatus::Modified  => floem::peniko::Color::from_rgb8(255, 200, 60),
            GitFileStatus::Added     => floem::peniko::Color::from_rgb8(72,  230, 150),
            GitFileStatus::Deleted   => floem::peniko::Color::from_rgb8(255, 80,  100),
            GitFileStatus::Untracked => floem::peniko::Color::from_rgb8(140, 160, 235),
            GitFileStatus::Renamed   => floem::peniko::Color::from_rgb8(255, 160, 60),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GitStatusData {
    pub staged:    Vec<GitFileEntry>,
    pub unstaged:  Vec<GitFileEntry>,
    pub untracked: Vec<GitFileEntry>,
}

// ── Git helpers ───────────────────────────────────────────────────────────────

fn parse_porcelain(output: &str) -> GitStatusData {
    let mut data = GitStatusData::default();
    for line in output.lines() {
        if line.len() < 3 {
            continue;
        }
        let staged_char   = line.chars().next().unwrap_or(' ');
        let unstaged_char = line.chars().nth(1).unwrap_or(' ');
        let path = line[3..].trim().to_string();

        if staged_char == '?' && unstaged_char == '?' {
            data.untracked.push(GitFileEntry {
                status: GitFileStatus::Untracked,
                path,
                staged: false,
            });
            continue;
        }

        let staged_status = char_to_status(staged_char);
        let unstaged_status = char_to_status(unstaged_char);

        if let Some(s) = staged_status {
            data.staged.push(GitFileEntry { status: s, path: path.clone(), staged: true });
        }
        if let Some(s) = unstaged_status {
            data.unstaged.push(GitFileEntry { status: s, path, staged: false });
        }
    }
    data
}

fn char_to_status(c: char) -> Option<GitFileStatus> {
    match c {
        'M' => Some(GitFileStatus::Modified),
        'A' => Some(GitFileStatus::Added),
        'D' => Some(GitFileStatus::Deleted),
        'R' => Some(GitFileStatus::Renamed),
        _   => None,
    }
}

fn run_git_status(root: &std::path::Path) -> GitStatusData {
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            parse_porcelain(&String::from_utf8_lossy(&o.stdout))
        }
        _ => GitStatusData::default(),
    }
}

fn run_git_commit(root: &std::path::Path, message: &str) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

// ── Panel root ────────────────────────────────────────────────────────────────

pub fn git_panel(state: IdeState) -> impl IntoView {
    let theme      = state.theme;
    let git_data   = create_rw_signal(GitStatusData::default());
    let commit_msg = create_rw_signal(String::new());
    let status_msg = create_rw_signal(String::new());
    let is_loading = create_rw_signal(false);

    // Initial git status load
    refresh_git_status(state.workspace_root.get(), git_data, is_loading);

    // ── Header row ────────────────────────────────────────────────────────────
    let refresh_hov = create_rw_signal(false);
    let state_r     = state.clone();
    let header = stack((
        label(|| "SOURCE CONTROL")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.color(p.text_muted)
                 .font_size(11.0)
                 .font_weight(floem::text::Weight::BOLD)
                 .flex_grow(1.0)
            }),
        // Refresh button
        container(
            phaze_icon(icons::SPINNER, 13.0, move |p| {
                if refresh_hov.get() { p.accent } else { p.text_muted }
            }, theme)
        )
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.padding(4.0)
             .border_radius(4.0)
             .cursor(floem::style::CursorStyle::Pointer)
             .background(if refresh_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
        })
        .on_click_stop(move |_| {
            refresh_git_status(state_r.workspace_root.get(), git_data, is_loading);
        })
        .on_event_stop(floem::event::EventListener::PointerEnter, move |_| refresh_hov.set(true))
        .on_event_stop(floem::event::EventListener::PointerLeave, move |_| refresh_hov.set(false)),
    ))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(12.0)
         .padding_vert(8.0)
         .border_bottom(1.0)
         .border_color(p.border)
         .width_full()
         .items_center()
    });

    // ── Commit area ───────────────────────────────────────────────────────────
    let commit_input = text_input(commit_msg)
        .placeholder("Message (Ctrl+Enter to commit)")
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.flex_grow(1.0)
             .background(p.bg_elevated)
             .border(1.0)
             .border_color(p.border)
             .border_radius(4.0)
             .color(p.text_primary)
             .padding_horiz(8.0)
             .padding_vert(5.0)
             .font_size(12.0)
        });

    let commit_hov = create_rw_signal(false);
    let state_c    = state.clone();
    let commit_btn = container(
        label(|| "✓ Commit")
            .style(move |s| {
                let t = theme.get();
                s.font_size(11.0)
                 .color(t.palette.bg_base)
                 .font_weight(floem::text::Weight::BOLD)
            }),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(10.0)
         .padding_vert(5.0)
         .border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if commit_hov.get() { p.accent_hover } else { p.accent })
    })
    .on_click_stop(move |_| {
        let msg = commit_msg.get();
        if msg.trim().is_empty() {
            status_msg.set("Enter a commit message first.".to_string());
            return;
        }
        let root = state_c.workspace_root.get();
        let msg2 = msg.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        std::thread::spawn(move || {
            let _ = tx.send(run_git_commit(&root, &msg2));
        });
        let rx_sig = create_signal_from_channel(rx);
        let state_d = state_c.clone();
        create_effect(move |_| {
            if let Some(result) = rx_sig.get() {
                match result {
                    Ok(()) => {
                        commit_msg.set(String::new());
                        status_msg.set("Committed successfully!".to_string());
                        refresh_git_status(state_d.workspace_root.get(), git_data, is_loading);
                    }
                    Err(e) => {
                        let first = e.lines().next().unwrap_or("unknown error").to_string();
                        status_msg.set(format!("Error: {first}"));
                    }
                }
            }
        });
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| commit_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| commit_hov.set(false));

    let commit_area = stack((commit_input, commit_btn))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.padding(8.0)
             .gap(6.0)
             .width_full()
             .items_center()
             .border_bottom(1.0)
             .border_color(p.border)
        });

    // ── Status feedback ───────────────────────────────────────────────────────
    let status_bar_view = label(move || {
        if is_loading.get() {
            "Refreshing...".to_string()
        } else {
            status_msg.get()
        }
    })
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.font_size(11.0)
         .color(p.success)
         .padding_horiz(12.0)
         .padding_vert(3.0)
         .width_full()
    });

    // ── File sections ─────────────────────────────────────────────────────────
    let file_list = scroll(
        stack((
            git_section("STAGED CHANGES",  SectionKind::Staged,    git_data, state.clone(), theme),
            git_section("CHANGES",         SectionKind::Unstaged,  git_data, state.clone(), theme),
            git_section("UNTRACKED FILES", SectionKind::Untracked, git_data, state.clone(), theme),
        ))
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    stack((header, commit_area, status_bar_view, file_list))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.flex_col()
             .width_full()
             .height_full()
             .background(p.bg_panel)
        })
}

// ── Helper: trigger an async git status refresh ───────────────────────────────

fn refresh_git_status(
    root: std::path::PathBuf,
    git_data: RwSignal<GitStatusData>,
    is_loading: RwSignal<bool>,
) {
    is_loading.set(true);
    let (tx, rx) = std::sync::mpsc::sync_channel::<GitStatusData>(1);
    std::thread::spawn(move || {
        let _ = tx.send(run_git_status(&root));
    });
    let rx_sig = create_signal_from_channel(rx);
    create_effect(move |_| {
        if let Some(data) = rx_sig.get() {
            git_data.set(data);
            is_loading.set(false);
        }
    });
}

// ── Section (Staged / Changes / Untracked) ────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Staged,
    Unstaged,
    Untracked,
}

fn git_section(
    title: &'static str,
    kind: SectionKind,
    git_data: RwSignal<GitStatusData>,
    state: IdeState,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    let expanded = create_rw_signal(true);
    let hov      = create_rw_signal(false);

    let header = container(
        stack((
            label(move || if expanded.get() { "▾ " } else { "▸ " })
                .style(move |s| {
                    s.font_size(10.0)
                     .color(theme.get().palette.text_muted)
                     .margin_right(2.0)
                }),
            label(move || {
                let data  = git_data.get();
                let count = match kind {
                    SectionKind::Staged    => data.staged.len(),
                    SectionKind::Unstaged  => data.unstaged.len(),
                    SectionKind::Untracked => data.untracked.len(),
                };
                format!("{title} ({count})")
            })
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(11.0)
                 .color(p.text_muted)
                 .font_weight(floem::text::Weight::BOLD)
            }),
        ))
        .style(|s| s.items_center()),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(10.0)
         .padding_vert(5.0)
         .width_full()
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| expanded.update(|v| *v = !*v))
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| hov.set(false));

    let rows = dyn_stack(
        move || {
            if !expanded.get() {
                return Vec::new();
            }
            let data = git_data.get();
            match kind {
                SectionKind::Staged    => data.staged,
                SectionKind::Unstaged  => data.unstaged,
                SectionKind::Untracked => data.untracked,
            }
        },
        |entry| entry.path.clone(),
        {
            let state = state.clone();
            move |entry: GitFileEntry| {
                let row_hov    = create_rw_signal(false);
                let badge      = entry.badge();
                let badge_col  = entry.badge_color();
                let rel_path   = entry.path.clone();
                let fname = std::path::Path::new(&rel_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| rel_path.clone());
                let parent = std::path::Path::new(&rel_path)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let abs_path = state.workspace_root.get().join(&rel_path);
                let state_r  = state.clone();

                container(
                    stack((
                        label(move || badge)
                            .style(move |s| {
                                s.font_size(10.0)
                                 .color(badge_col)
                                 .font_weight(floem::text::Weight::BOLD)
                                 .min_width(14.0)
                                 .margin_right(4.0)
                            }),
                        label(move || fname.clone())
                            .style(move |s| {
                                let t = theme.get();
                                s.font_size(12.0)
                                 .color(t.palette.text_primary)
                                 .flex_grow(1.0)
                                 .min_width(0.0)
                            }),
                        label(move || parent.clone())
                            .style(move |s| {
                                let t = theme.get();
                                s.font_size(10.0)
                                 .color(t.palette.text_muted)
                                 .margin_left(4.0)
                            }),
                    ))
                    .style(|s| s.items_center().width_full().min_width(0.0)),
                )
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.width_full()
                     .padding_horiz(16.0)
                     .padding_vert(3.0)
                     .border_radius(3.0)
                     .cursor(floem::style::CursorStyle::Pointer)
                     .background(if row_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                })
                .on_click_stop(move |_| {
                    state_r.open_file.set(Some(abs_path.clone()));
                })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| row_hov.set(true))
                .on_event_stop(floem::event::EventListener::PointerLeave, move |_| row_hov.set(false))
            }
        },
    )
    .style(|s: floem::style::Style| s.flex_col().width_full());

    stack((header, rows)).style(|s| s.flex_col().width_full())
}
