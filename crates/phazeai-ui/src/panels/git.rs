//! Git Source Control panel for PhazeAI IDE.
//!
//! Shows staged changes, unstaged changes, and untracked files.
//! Provides commit message input, commit button, and file click-to-open.
//! Also includes branch switching, pull/push, stash, and commit history.

use floem::{
    ext_event::{create_ext_action, create_signal_from_channel},
    reactive::{create_effect, create_rw_signal, RwSignal, Scope, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::{Agent, AgentEvent, Settings};

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

/// A single commit entry from `git log`.
#[derive(Clone, Debug)]
pub struct CommitEntry {
    pub hash:    String,
    pub message: String,
    pub author:  String,
    pub date:    String,
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

fn run_git_add(root: &std::path::Path, path: &str) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .args(["add", path])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).to_string()) }
}

fn run_git_reset(root: &std::path::Path, path: &str) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .args(["reset", "HEAD", "--", path])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).to_string()) }
}

fn run_git_discard(root: &std::path::Path, path: &str) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .args(["checkout", "--", path])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).to_string()) }
}

/// Returns (current_branch, all_branches).
fn run_git_branches(root: &std::path::Path) -> (String, Vec<String>) {
    // Current branch
    let current = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "main".to_string());

    // All local branches
    let branches_raw = std::process::Command::new("git")
        .args(["branch", "--list"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();

    let branches: Vec<String> = branches_raw
        .lines()
        .map(|l| l.trim_start_matches('*').trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    (current, branches)
}

fn run_git_checkout(root: &std::path::Path, branch: &str) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .args(["checkout", branch])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).to_string()) }
}

fn run_git_checkout_new(root: &std::path::Path, branch: &str) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .args(["checkout", "-b", branch])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).to_string()) }
}

fn run_git_pull(root: &std::path::Path) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["pull"])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn run_git_push(root: &std::path::Path) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["push"])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok("Pushed successfully.".to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn run_git_stash(root: &std::path::Path) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["stash", "push", "-m", "WIP"])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn run_git_stash_pop(root: &std::path::Path) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["stash", "pop"])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Loads the 50 most recent commits via `git log`.
fn run_git_log(root: &std::path::Path) -> Vec<CommitEntry> {
    let out = std::process::Command::new("git")
        .args(["log", "--format=%h|%s|%an|%ar", "-50"])
        .current_dir(root)
        .output();
    let Ok(o) = out else { return vec![] };
    if !o.status.success() { return vec![]; }
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, '|').collect();
            if parts.len() == 4 {
                Some(CommitEntry {
                    hash:    parts[0].to_string(),
                    message: parts[1].to_string(),
                    author:  parts[2].to_string(),
                    date:    parts[3].to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

// ── Git Blame ─────────────────────────────────────────────────────────────────

/// One line of `git blame` output.
#[derive(Clone, Debug)]
pub struct BlameEntry {
    /// 1-based line number.
    pub line:    usize,
    /// Short commit hash (first 8 chars).
    pub hash:    String,
    /// Committer name.
    pub author:  String,
    /// Commit date `YYYY-MM-DD`.
    pub date:    String,
    /// The source line content.
    pub content: String,
}

fn parse_blame_line(line_no: usize, raw: &str) -> BlameEntry {
    // Format (from `git blame --date=short`):
    //   `^abc1234 (Author Name  2024-01-15  1) content`
    //   ` abc1234 (Author Name  2024-01-15 42) content`
    let hash: String = raw.trim_start_matches('^')
        .chars().take(8).collect();

    // Locate the date: first YYYY-MM-DD token.
    let date = raw.split_whitespace()
        .find(|t| {
            t.len() == 10
                && t.chars().nth(4) == Some('-')
                && t.chars().nth(7) == Some('-')
        })
        .unwrap_or("")
        .to_string();

    // Author: text between '(' and the date.
    let author = raw.find('(')
        .map(|open| {
            let rest = &raw[open + 1..];
            if date.is_empty() {
                rest.splitn(2, ' ').next().unwrap_or("?").trim().to_string()
            } else if let Some(dp) = rest.find(date.as_str()) {
                rest[..dp].trim().to_string()
            } else {
                rest.splitn(2, ' ').next().unwrap_or("?").trim().to_string()
            }
        })
        .unwrap_or_else(|| "?".to_string());

    // Content: text after ')'.
    let content = raw.find(')')
        .map(|close| raw[close + 1..].trim_start_matches(' ').to_string())
        .unwrap_or_else(|| raw.to_string());

    BlameEntry { line: line_no, hash, author, date, content }
}

/// Run `git blame --date=short <path>` and return per-line blame info.
fn run_git_blame(path: &std::path::Path) -> Vec<BlameEntry> {
    let dir = path.parent().unwrap_or(path);
    let out = match std::process::Command::new("git")
        .args(["blame", "--date=short", path.to_str().unwrap_or("")])
        .current_dir(dir)
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    if !out.status.success() || out.stdout.is_empty() { return vec![]; }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .enumerate()
        .map(|(i, line)| parse_blame_line(i + 1, line))
        .collect()
}

// ── Panel root ────────────────────────────────────────────────────────────────

pub fn git_panel(state: IdeState) -> impl IntoView {
    let theme      = state.theme;
    let git_data   = create_rw_signal(GitStatusData::default());
    let commit_msg = create_rw_signal(String::new());
    let status_msg = create_rw_signal(String::new());
    let is_loading = create_rw_signal(false);

    // Branch signals
    let current_branch    = create_rw_signal(String::from("main"));
    let branches          = create_rw_signal(Vec::<String>::new());
    let branch_picker_open = create_rw_signal(false);
    // "New branch" overlay
    let new_branch_open   = create_rw_signal(false);
    let new_branch_name   = create_rw_signal(String::new());

    // Commit history
    let commits = create_rw_signal(Vec::<CommitEntry>::new());

    // Git blame
    let blame_lines:    RwSignal<Vec<BlameEntry>> = create_rw_signal(vec![]);
    let blame_loading:  RwSignal<bool>            = create_rw_signal(false);
    let blame_expanded: RwSignal<bool>            = create_rw_signal(false);
    let blame_file:     RwSignal<String>          = create_rw_signal(String::new());

    // Helper: full refresh (status + branch + log)
    let full_refresh = {
        let root = state.workspace_root;
        move || {
            let r = root.get();
            refresh_git_status(r.clone(), git_data, is_loading);
            refresh_branches(r.clone(), current_branch, branches);
            refresh_commits(r, commits);
        }
    };

    // Initial load
    full_refresh();

    // ── Row 1: branch button + pull + push ────────────────────────────────────
    let branch_hov  = create_rw_signal(false);
    let pull_hov    = create_rw_signal(false);
    let push_hov    = create_rw_signal(false);

    let state_pull = state.clone();
    let state_push = state.clone();

    let pull_btn = container(
        label(|| "Pull")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(11.0)
                 .color(if pull_hov.get() { p.accent_hover } else { p.accent })
                 .font_weight(floem::text::Weight::BOLD)
            })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(6.0)
         .padding_vert(3.0)
         .border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if pull_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| {
        let root = state_pull.workspace_root.get();
        let scope = Scope::new();
        let send = create_ext_action(scope, move |result: Result<String, String>| {
            match result {
                Ok(msg) => {
                    let summary = if msg.is_empty() { "Already up to date.".to_string() } else { msg };
                    status_msg.set(summary);
                }
                Err(e) => {
                    let first = e.lines().next().unwrap_or("pull failed").to_string();
                    status_msg.set(format!("Pull error: {first}"));
                }
            }
            refresh_git_status(state_pull.workspace_root.get(), git_data, is_loading);
            refresh_commits(state_pull.workspace_root.get(), commits);
        });
        std::thread::spawn(move || { send(run_git_pull(&root)); });
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| pull_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| pull_hov.set(false));

    let push_btn = container(
        label(|| "Push")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(11.0)
                 .color(if push_hov.get() { p.accent_hover } else { p.accent })
                 .font_weight(floem::text::Weight::BOLD)
            })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(6.0)
         .padding_vert(3.0)
         .border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if push_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| {
        let root = state_push.workspace_root.get();
        let scope = Scope::new();
        let send = create_ext_action(scope, move |result: Result<String, String>| {
            match result {
                Ok(msg) => { status_msg.set(msg); }
                Err(e) => {
                    let first = e.lines().next().unwrap_or("push failed").to_string();
                    status_msg.set(format!("Push error: {first}"));
                }
            }
        });
        std::thread::spawn(move || { send(run_git_push(&root)); });
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| push_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| push_hov.set(false));

    // Branch button
    let state_br = state.clone();
    let branch_btn = container(
        label(move || {
            let b = current_branch.get();
            format!(" {b}")
        })
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.font_size(11.0)
             .color(if branch_hov.get() { p.accent_hover } else { p.accent })
             .font_weight(floem::text::Weight::BOLD)
        })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(6.0)
         .padding_vert(3.0)
         .border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if branch_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| {
        // Refresh branch list and toggle picker
        refresh_branches(state_br.workspace_root.get(), current_branch, branches);
        branch_picker_open.update(|v| *v = !*v);
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| branch_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| branch_hov.set(false));

    // ── Stash buttons ─────────────────────────────────────────────────────────
    let stash_hov     = create_rw_signal(false);
    let stash_pop_hov = create_rw_signal(false);
    let state_stash   = state.clone();
    let state_stashp  = state.clone();

    let stash_btn = container(
        label(|| "Stash")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(11.0)
                 .color(if stash_hov.get() { p.accent_hover } else { p.text_muted })
            })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(5.0)
         .padding_vert(3.0)
         .border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if stash_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| {
        let root = state_stash.workspace_root.get();
        let scope = Scope::new();
        let root2 = root.clone();
        let send = create_ext_action(scope, move |result: Result<String, String>| {
            match result {
                Ok(_)  => { status_msg.set("Stashed WIP.".to_string()); }
                Err(e) => { status_msg.set(format!("Stash error: {}", e.lines().next().unwrap_or("?"))); }
            }
            refresh_git_status(root2.clone(), git_data, is_loading);
        });
        std::thread::spawn(move || { send(run_git_stash(&root)); });
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| stash_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| stash_hov.set(false));

    let stash_pop_btn = container(
        label(|| "Pop")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(11.0)
                 .color(if stash_pop_hov.get() { p.accent_hover } else { p.text_muted })
            })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(5.0)
         .padding_vert(3.0)
         .border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if stash_pop_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| {
        let root = state_stashp.workspace_root.get();
        let scope = Scope::new();
        let root2 = root.clone();
        let send = create_ext_action(scope, move |result: Result<String, String>| {
            match result {
                Ok(_)  => { status_msg.set("Stash popped.".to_string()); }
                Err(e) => { status_msg.set(format!("Pop error: {}", e.lines().next().unwrap_or("?"))); }
            }
            refresh_git_status(root2.clone(), git_data, is_loading);
        });
        std::thread::spawn(move || { send(run_git_stash_pop(&root)); });
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| stash_pop_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| stash_pop_hov.set(false));

    // ── Stage All + Refresh buttons ───────────────────────────────────────────
    let refresh_hov    = create_rw_signal(false);
    let stage_all_hov  = create_rw_signal(false);
    let state_r        = state.clone();
    let state_sa       = state.clone();

    let stage_all_btn = container(
        label(|| "+A")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(10.0)
                 .font_weight(floem::text::Weight::BOLD)
                 .color(if stage_all_hov.get() { p.accent_hover } else { p.accent })
            })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(5.0)
         .padding_vert(3.0)
         .border_radius(4.0)
         .margin_right(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if stage_all_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| {
        let root = state_sa.workspace_root.get();
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        std::thread::spawn(move || {
            let _ = tx.send(run_git_add(&root, "-A"));
        });
        let rx_sig = create_signal_from_channel(rx);
        let root2 = state_sa.workspace_root.get();
        create_effect(move |_| {
            if rx_sig.get().is_some() {
                refresh_git_status(root2.clone(), git_data, is_loading);
            }
        });
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| stage_all_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| stage_all_hov.set(false));

    let refresh_btn = container(
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
        refresh_branches(state_r.workspace_root.get(), current_branch, branches);
        refresh_commits(state_r.workspace_root.get(), commits);
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| refresh_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| refresh_hov.set(false));

    // ── Header row ────────────────────────────────────────────────────────────
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
        branch_btn,
        pull_btn,
        push_btn,
        stash_btn,
        stash_pop_btn,
        stage_all_btn,
        refresh_btn,
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
         .flex_wrap(floem::style::FlexWrap::Wrap)
    });

    // ── Branch picker dropdown ─────────────────────────────────────────────────
    let state_checkout = state.clone();
    let _state_newbr   = state.clone();

    let new_branch_hov = create_rw_signal(false);
    let new_branch_btn = container(
        label(|| "+ New Branch")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(12.0)
                 .color(if new_branch_hov.get() { p.accent_hover } else { p.accent })
            })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(10.0)
         .padding_vert(5.0)
         .width_full()
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if new_branch_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| {
        branch_picker_open.set(false);
        new_branch_open.set(true);
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| new_branch_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| new_branch_hov.set(false));

    let branch_list = dyn_stack(
        move || branches.get(),
        |b| b.clone(),
        move |branch_name: String| {
            let row_hov = create_rw_signal(false);
            let bn = branch_name.clone();
            let bn2 = branch_name.clone();
            let root = state_checkout.workspace_root.get();
            container(
                label(move || bn.clone())
                    .style(move |s| {
                        let t = theme.get();
                        let p = &t.palette;
                        let is_current = current_branch.get() == bn2;
                        s.font_size(12.0)
                         .color(if is_current { p.accent } else { p.text_primary })
                         .font_weight(if is_current { floem::text::Weight::BOLD } else { floem::text::Weight::NORMAL })
                    })
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(10.0)
                 .padding_vert(5.0)
                 .width_full()
                 .cursor(floem::style::CursorStyle::Pointer)
                 .background(if row_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
            })
            .on_click_stop(move |_| {
                let b = branch_name.clone();
                let r = root.clone();
                let scope = Scope::new();
                let send = create_ext_action(scope, move |result: Result<(), String>| {
                    match result {
                        Ok(()) => {
                            refresh_branches(r.clone(), current_branch, branches);
                            refresh_git_status(r.clone(), git_data, is_loading);
                            refresh_commits(r.clone(), commits);
                            status_msg.set(format!("Switched to branch '{}'", current_branch.get()));
                        }
                        Err(e) => {
                            status_msg.set(format!("Checkout error: {}", e.lines().next().unwrap_or("?")));
                        }
                    }
                    branch_picker_open.set(false);
                });
                let b2 = b.clone();
                let r2 = root.clone();
                std::thread::spawn(move || { send(run_git_checkout(&r2, &b2)); });
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| row_hov.set(true))
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| row_hov.set(false))
        },
    )
    .style(|s| s.flex_col().width_full());

    let branch_dropdown = container(
        scroll(
            stack((new_branch_btn, branch_list)).style(|s| s.flex_col().width_full())
        )
        .style(|s| s.max_height(200.0).width_full())
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.width_full()
         .background(p.bg_panel)
         .border(1.0)
         .border_color(p.border)
         .border_radius(4.0)
         .z_index(50)
         .apply_if(!branch_picker_open.get(), |s| s.display(floem::style::Display::None))
    });

    // ── New branch overlay ────────────────────────────────────────────────────
    let confirm_new_branch_hov = create_rw_signal(false);
    let cancel_new_branch_hov  = create_rw_signal(false);
    let state_nb = state.clone();

    let new_branch_input = text_input(new_branch_name)
        .placeholder("New branch name")
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

    let create_branch_btn = container(
        label(|| "Create")
            .style(move |s| {
                let t = theme.get();
                s.font_size(11.0)
                 .color(t.palette.bg_base)
                 .font_weight(floem::text::Weight::BOLD)
            })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(10.0)
         .padding_vert(5.0)
         .border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if confirm_new_branch_hov.get() { p.accent_hover } else { p.accent })
    })
    .on_click_stop(move |_| {
        let name = new_branch_name.get();
        let name = name.trim().to_string();
        if name.is_empty() { return; }
        let root = state_nb.workspace_root.get();
        let name2 = name.clone();
        let scope = Scope::new();
        let root2 = root.clone();
        let send = create_ext_action(scope, move |result: Result<(), String>| {
            match result {
                Ok(()) => {
                    refresh_branches(root2.clone(), current_branch, branches);
                    refresh_git_status(root2.clone(), git_data, is_loading);
                    refresh_commits(root2.clone(), commits);
                    status_msg.set(format!("Created and switched to '{name2}'"));
                    new_branch_name.set(String::new());
                }
                Err(e) => {
                    status_msg.set(format!("Error: {}", e.lines().next().unwrap_or("?")));
                }
            }
            new_branch_open.set(false);
        });
        std::thread::spawn(move || { send(run_git_checkout_new(&root, &name)); });
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| confirm_new_branch_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| confirm_new_branch_hov.set(false));

    let cancel_branch_btn = container(
        label(|| "Cancel")
            .style(move |s| {
                let t = theme.get();
                s.font_size(11.0)
                 .color(t.palette.text_muted)
            })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(8.0)
         .padding_vert(5.0)
         .border_radius(4.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if cancel_new_branch_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| {
        new_branch_open.set(false);
        new_branch_name.set(String::new());
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| cancel_new_branch_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| cancel_new_branch_hov.set(false));

    let new_branch_overlay = container(
        stack((new_branch_input, create_branch_btn, cancel_branch_btn))
            .style(|s| s.gap(6.0).items_center().padding(8.0).width_full())
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.width_full()
         .background(p.bg_elevated)
         .border_bottom(1.0)
         .border_color(p.border)
         .apply_if(!new_branch_open.get(), |s| s.display(floem::style::Display::None))
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

    // ── AI commit message generator ───────────────────────────────────────────
    let ai_gen_active = create_rw_signal(false);
    let ai_gen_hov    = create_rw_signal(false);
    let state_ai      = state.clone();
    let ai_btn = container(
        label(move || if ai_gen_active.get() { "…".to_string() } else { "✨ AI".to_string() })
            .style(move |s| {
                let t = theme.get();
                s.font_size(11.0)
                 .color(if ai_gen_active.get() { t.palette.text_muted } else { t.palette.accent })
                 .font_weight(floem::text::Weight::BOLD)
            }),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(8.0)
         .padding_vert(5.0)
         .border_radius(4.0)
         .cursor(if ai_gen_active.get() {
             floem::style::CursorStyle::Default
         } else {
             floem::style::CursorStyle::Pointer
         })
         .background(if ai_gen_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
         .border(1.0)
         .border_color(if ai_gen_hov.get() { p.border } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| {
        if ai_gen_active.get() { return; }
        ai_gen_active.set(true);

        let root  = state_ai.workspace_root.get();
        let scope = Scope::new();
        let send  = create_ext_action(scope, move |result: String| {
            if !result.is_empty() {
                commit_msg.set(result);
            }
            ai_gen_active.set(false);
        });

        std::thread::spawn(move || {
            // Collect staged diff summary + full diff (capped at 8 kB)
            let stat = std::process::Command::new("git")
                .args(["diff", "--cached", "--stat"])
                .current_dir(&root)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

            if stat.is_empty() {
                send("No staged changes.".to_string());
                return;
            }

            let full_diff = std::process::Command::new("git")
                .args(["diff", "--cached"])
                .current_dir(&root)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();

            let snippet = if full_diff.len() > 8_000 {
                format!("{}…(truncated)", &full_diff[..8_000])
            } else {
                full_diff
            };

            let prompt = format!(
                "Write a concise git commit message for these changes.\n\
                 Rules: imperative mood, ≤50 chars subject line, no period at end.\n\
                 Reply with ONLY the commit message — no explanation.\n\n\
                 Stats:\n{stat}\n\nDiff:\n{snippet}"
            );

            let settings = Settings::load();
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => { send(String::new()); return; }
            };

            let result = rt.block_on(async move {
                let client = match settings.build_llm_client() {
                    Ok(c)  => c,
                    Err(_) => return String::new(),
                };
                let agent = Agent::new(client);
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
                let mut accumulated = String::new();
                let run_fut   = agent.run_with_events(&prompt, tx);
                let drain_fut = async {
                    while let Some(ev) = rx.recv().await {
                        match ev {
                            AgentEvent::TextDelta(t) => accumulated.push_str(&t),
                            AgentEvent::Complete { .. } | AgentEvent::Error(_) => break,
                            _ => {}
                        }
                    }
                };
                let _ = tokio::join!(run_fut, drain_fut);
                accumulated.trim().to_string()
            });

            send(result);
        });
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| ai_gen_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| ai_gen_hov.set(false));

    let commit_hov = create_rw_signal(false);
    let state_c    = state.clone();
    let commit_btn = container(
        label(|| "Commit")
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
                        refresh_commits(state_d.workspace_root.get(), commits);
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

    let commit_area = stack((commit_input, ai_btn, commit_btn))
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
    let file_sections = stack((
        git_section("STAGED CHANGES",  SectionKind::Staged,    git_data, is_loading, state.clone(), theme),
        git_section("CHANGES",         SectionKind::Unstaged,  git_data, is_loading, state.clone(), theme),
        git_section("UNTRACKED FILES", SectionKind::Untracked, git_data, is_loading, state.clone(), theme),
    ))
    .style(|s| s.flex_col().width_full());

    // ── Commit history ────────────────────────────────────────────────────────
    let history_expanded = create_rw_signal(true);
    let history_hov      = create_rw_signal(false);

    let history_header = container(
        stack((
            label(move || if history_expanded.get() { "▾ " } else { "▸ " })
                .style(move |s| s.font_size(10.0).color(theme.get().palette.text_muted).margin_right(2.0)),
            label(move || {
                let n = commits.get().len();
                format!("COMMIT HISTORY ({n})")
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
         .border_top(1.0)
         .border_color(p.border)
         .background(if history_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| history_expanded.update(|v| *v = !*v))
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| history_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| history_hov.set(false));

    let commit_rows = dyn_stack(
        move || {
            if !history_expanded.get() { return vec![]; }
            commits.get()
        },
        |c| c.hash.clone(),
        move |entry: CommitEntry| {
            let row_hov = create_rw_signal(false);
            let hash    = entry.hash.clone();
            let msg     = entry.message.clone();
            let author  = entry.author.clone();
            let date    = entry.date.clone();

            container(
                stack((
                    label(move || hash.clone())
                        .style(move |s| {
                            let t = theme.get();
                            s.font_size(10.0)
                             .color(t.palette.accent)
                             .min_width(50.0)
                             .font_family("monospace".to_string())
                        }),
                    label(move || msg.clone())
                        .style(move |s| {
                            let t = theme.get();
                            s.font_size(11.0)
                             .color(t.palette.text_primary)
                             .flex_grow(1.0)
                             .min_width(0.0)
                        }),
                    label(move || format!(" {author}"))
                        .style(move |s| {
                            let t = theme.get();
                            s.font_size(10.0).color(t.palette.text_muted)
                        }),
                    label(move || format!(" ({date})"))
                        .style(move |s| {
                            let t = theme.get();
                            s.font_size(10.0).color(t.palette.text_muted)
                        }),
                ))
                .style(|s| s.items_center().width_full().min_width(0.0)),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.width_full()
                 .padding_horiz(12.0)
                 .padding_vert(3.0)
                 .border_radius(3.0)
                 .cursor(floem::style::CursorStyle::Default)
                 .background(if row_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| row_hov.set(true))
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| row_hov.set(false))
        },
    )
    .style(|s: floem::style::Style| s.flex_col().width_full());

    let history_scroll = scroll(commit_rows)
        .style(move |s| {
            s.max_height(200.0)
             .width_full()
             .apply_if(!history_expanded.get(), |s| s.display(floem::style::Display::None))
        });

    let commit_history = stack((history_header, history_scroll))
        .style(|s| s.flex_col().width_full());

    // ── Git Blame section ─────────────────────────────────────────────────────
    let blame_hov     = create_rw_signal(false);
    let blame_btn_hov = create_rw_signal(false);
    let state_blame   = state.clone();

    // "Blame Current File" button
    let blame_btn = container(
        label(move || {
            if blame_loading.get() { "blaming…".to_string() }
            else { "Blame File".to_string() }
        })
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.font_size(10.0)
             .color(if blame_btn_hov.get() { p.accent_hover } else { p.accent })
        })
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(6.0).padding_vert(2.0).border_radius(3.0)
         .cursor(floem::style::CursorStyle::Pointer)
         .background(if blame_btn_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
         .apply_if(blame_loading.get(), |s| s.color(floem::peniko::Color::from_rgba8(255,255,255,128)))
    })
    .on_click_stop(move |_| {
        let Some((path, _, _)) = state_blame.active_cursor.get() else { return };
        blame_loading.set(true);
        blame_file.set(path.to_string_lossy().to_string());
        blame_expanded.set(true);
        let scope = Scope::new();
        let send = create_ext_action(scope, move |entries: Vec<BlameEntry>| {
            blame_lines.set(entries);
            blame_loading.set(false);
        });
        std::thread::spawn(move || { send(run_git_blame(&path)); });
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| blame_btn_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| blame_btn_hov.set(false));

    let blame_header = container(
        stack((
            label(move || if blame_expanded.get() { "▾ " } else { "▸ " })
                .style(move |s| s.font_size(10.0).color(theme.get().palette.text_muted).margin_right(2.0)),
            label(move || {
                let n = blame_lines.get().len();
                let f = blame_file.get();
                let fname = std::path::Path::new(&f)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if fname.is_empty() {
                    format!("GIT BLAME ({n} lines)")
                } else {
                    format!("GIT BLAME — {fname} ({n})")
                }
            })
            .style(move |s| {
                let t = theme.get();
                s.font_size(11.0).color(t.palette.text_muted).font_weight(floem::text::Weight::BOLD).flex_grow(1.0)
            }),
            blame_btn,
        ))
        .style(|s| s.items_center().width_full()),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(10.0).padding_vert(5.0).width_full()
         .cursor(floem::style::CursorStyle::Pointer)
         .border_top(1.0).border_color(p.border)
         .background(if blame_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
    })
    .on_click_stop(move |_| blame_expanded.update(|v| *v = !*v))
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| blame_hov.set(true))
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| blame_hov.set(false));

    let state_blame_rows = state.clone();
    let blame_rows = dyn_stack(
        move || {
            if !blame_expanded.get() { return vec![]; }
            blame_lines.get()
        },
        |e| e.line,
        move |entry: BlameEntry| {
            let row_hov  = create_rw_signal(false);
            let hash     = entry.hash.clone();
            let author   = entry.author.clone();
            let date     = entry.date.clone();
            let content  = if entry.content.len() > 60 {
                format!("{}…", &entry.content[..60])
            } else {
                entry.content.clone()
            };
            let line_no  = entry.line;
            let state_b  = state_blame_rows.clone();

            container(
                stack((
                    // Hash badge
                    label(move || hash.clone())
                        .style(move |s| {
                            let t = theme.get();
                            s.font_size(10.0).color(t.palette.accent)
                             .min_width(62.0).font_family("monospace".to_string())
                        }),
                    // Author + date
                    label(move || format!("{author}  {date}"))
                        .style(move |s| {
                            let t = theme.get();
                            s.font_size(10.0).color(t.palette.text_muted).min_width(120.0)
                        }),
                    // Line content preview
                    label(move || content.clone())
                        .style(move |s| {
                            let t = theme.get();
                            s.font_size(10.0).color(t.palette.text_primary).flex_grow(1.0).min_width(0.0)
                        }),
                ))
                .style(|s| s.items_center().width_full().min_width(0.0)),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.width_full().padding_horiz(12.0).padding_vert(2.0).border_radius(2.0)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .background(if row_hov.get() { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
            })
            .on_click_stop(move |_| {
                // Jump to the blamed line in the editor.
                if let Some((cur_path, _, _)) = state_b.active_cursor.get() {
                    state_b.open_file.set(Some(cur_path));
                }
                state_b.goto_line.set(line_no as u32);
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| row_hov.set(true))
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| row_hov.set(false))
        },
    )
    .style(|s: floem::style::Style| s.flex_col().width_full());

    let blame_scroll = scroll(blame_rows)
        .style(move |s| {
            s.max_height(250.0).width_full()
             .apply_if(!blame_expanded.get(), |s| s.display(floem::style::Display::None))
        });

    let blame_section = stack((blame_header, blame_scroll))
        .style(|s| s.flex_col().width_full());

    // ── Full scrollable body ──────────────────────────────────────────────────
    let body = scroll(
        stack((file_sections, commit_history, blame_section))
            .style(|s| s.flex_col().width_full())
    )
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    stack((
        header,
        branch_dropdown,
        new_branch_overlay,
        commit_area,
        status_bar_view,
        body,
    ))
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

fn refresh_branches(
    root: std::path::PathBuf,
    current_branch: RwSignal<String>,
    branches: RwSignal<Vec<String>>,
) {
    let scope = Scope::new();
    let send = create_ext_action(scope, move |(cur, list): (String, Vec<String>)| {
        current_branch.set(cur);
        branches.set(list);
    });
    std::thread::spawn(move || {
        send(run_git_branches(&root));
    });
}

fn refresh_commits(
    root: std::path::PathBuf,
    commits: RwSignal<Vec<CommitEntry>>,
) {
    let scope = Scope::new();
    let send = create_ext_action(scope, move |list: Vec<CommitEntry>| {
        commits.set(list);
    });
    std::thread::spawn(move || {
        send(run_git_log(&root));
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
    is_loading: RwSignal<bool>,
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
                let abs_path   = state.workspace_root.get().join(&rel_path);
                let state_r    = state.clone();
                let root       = state.workspace_root.get();

                // ── Action buttons (only visible on hover) ────────────────
                // Primary action: stage (+) for Unstaged/Untracked, unstage (−) for Staged
                let primary_hov = create_rw_signal(false);
                let rel_path1   = rel_path.clone();
                let root1       = root.clone();
                let primary_label = match kind {
                    SectionKind::Staged    => "−",
                    SectionKind::Unstaged  => "+",
                    SectionKind::Untracked => "+",
                };
                let primary_btn = container(
                    label(move || primary_label)
                        .style(move |s| {
                            let t = theme.get();
                            let p = &t.palette;
                            s.font_size(12.0)
                             .font_weight(floem::text::Weight::BOLD)
                             .color(if primary_hov.get() { p.accent_hover } else { p.accent })
                        })
                )
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.width(20.0)
                     .height(20.0)
                     .border_radius(3.0)
                     .items_center()
                     .justify_center()
                     .cursor(floem::style::CursorStyle::Pointer)
                     .background(p.bg_elevated)
                     .apply_if(!row_hov.get(), |s| s.display(floem::style::Display::None))
                })
                .on_click_stop(move |_| {
                    let path = rel_path1.clone();
                    let r    = root1.clone();
                    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
                    std::thread::spawn(move || {
                        let result = match kind {
                            SectionKind::Staged    => run_git_reset(&r, &path),
                            SectionKind::Unstaged  => run_git_add(&r, &path),
                            SectionKind::Untracked => run_git_add(&r, &path),
                        };
                        let _ = tx.send(result);
                    });
                    let rx_sig = create_signal_from_channel(rx);
                    let r2 = root1.clone();
                    create_effect(move |_| {
                        if rx_sig.get().is_some() {
                            refresh_git_status(r2.clone(), git_data, is_loading);
                        }
                    });
                })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| primary_hov.set(true))
                .on_event_stop(floem::event::EventListener::PointerLeave, move |_| primary_hov.set(false));

                // Discard button (↩) — only for Unstaged section
                let discard_hov = create_rw_signal(false);
                let rel_path2   = rel_path.clone();
                let root2       = root.clone();
                let discard_btn = container(
                    label(|| "↩")
                        .style(move |s| {
                            let t = theme.get();
                            let p = &t.palette;
                            s.font_size(12.0)
                             .color(if discard_hov.get() { p.accent_hover } else { p.warning })
                        })
                )
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.width(20.0)
                     .height(20.0)
                     .border_radius(3.0)
                     .items_center()
                     .justify_center()
                     .cursor(floem::style::CursorStyle::Pointer)
                     .background(p.bg_elevated)
                     .margin_left(2.0)
                     // Only show for Unstaged, and only on hover
                     .apply_if(kind != SectionKind::Unstaged || !row_hov.get(), |s| {
                         s.display(floem::style::Display::None)
                     })
                })
                .on_click_stop(move |_| {
                    let path = rel_path2.clone();
                    let r    = root2.clone();
                    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
                    std::thread::spawn(move || {
                        let _ = tx.send(run_git_discard(&r, &path));
                    });
                    let rx_sig = create_signal_from_channel(rx);
                    let r2 = root2.clone();
                    create_effect(move |_| {
                        if rx_sig.get().is_some() {
                            refresh_git_status(r2.clone(), git_data, is_loading);
                        }
                    });
                })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| discard_hov.set(true))
                .on_event_stop(floem::event::EventListener::PointerLeave, move |_| discard_hov.set(false));

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
                        primary_btn,
                        discard_btn,
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
