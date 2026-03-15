use crate::app::IdeState;
use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, h_stack, label, scroll, v_stack, Decorators},
    IntoView,
};
use std::sync::Arc;

// ─── Data Structures ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct WorkflowRun {
    pub id: u64,
    pub name: String,
    pub head_branch: String,
    pub head_commit_message: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug)]
pub struct WorkflowJob {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub duration_secs: u64,
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn time_ago(iso: &str) -> String {
    if iso.len() >= 16 {
        iso[..16].replace('T', " ")
    } else {
        iso.to_string()
    }
}

fn status_icon(status: &str, conclusion: Option<&str>) -> &'static str {
    match (status, conclusion) {
        ("completed", Some("success")) => "✓",
        ("completed", Some("failure")) => "✗",
        ("completed", Some("cancelled")) => "○",
        ("in_progress", _) => "⏳",
        ("queued", _) => "·",
        _ => "?",
    }
}

fn status_color(status: &str, conclusion: Option<&str>) -> floem::peniko::Color {
    match (status, conclusion) {
        ("completed", Some("success")) => floem::peniko::Color::from_rgb8(80, 200, 120),
        ("completed", Some("failure")) => floem::peniko::Color::from_rgb8(220, 80, 80),
        ("in_progress", _) => floem::peniko::Color::from_rgb8(230, 200, 60),
        _ => floem::peniko::Color::from_rgb8(160, 160, 160),
    }
}

fn fetch_json(url: &str, token: Option<&str>) -> Result<serde_json::Value, String> {
    use std::process::Command;
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sf",
        "--max-time",
        "10",
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "X-GitHub-Api-Version: 2022-11-28",
    ]);
    if let Some(tok) = token {
        cmd.args(["-H", &format!("Authorization: Bearer {}", tok)]);
    }
    cmd.arg(url);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("HTTP error: {}", out.status));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

fn get_gh_token() -> Option<String> {
    std::env::var("GH_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok()
}

fn parse_owner_repo() -> Option<(String, String)> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    parse_remote_url(&url)
}

fn parse_remote_url(url: &str) -> Option<(String, String)> {
    // Handle https://github.com/owner/repo.git or https://github.com/owner/repo
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let rest = rest.trim_end_matches('/');
        let mut parts = rest.splitn(2, '/');
        let owner = parts.next()?.to_string();
        let repo = parts.next()?.to_string();
        return Some((owner, repo));
    }
    // Handle git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let rest = rest.trim_end_matches('/');
        let mut parts = rest.splitn(2, '/');
        let owner = parts.next()?.to_string();
        let repo = parts.next()?.to_string();
        return Some((owner, repo));
    }
    None
}

fn parse_runs(val: &serde_json::Value) -> Vec<WorkflowRun> {
    val["workflow_runs"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|r| WorkflowRun {
            id: r["id"].as_u64().unwrap_or(0),
            name: r["name"].as_str().unwrap_or("").to_string(),
            head_branch: r["head_branch"].as_str().unwrap_or("").to_string(),
            head_commit_message: r["head_commit"]["message"]
                .as_str()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(40)
                .collect(),
            status: r["status"].as_str().unwrap_or("").to_string(),
            conclusion: r["conclusion"].as_str().map(|s| s.to_string()),
            updated_at: r["updated_at"].as_str().unwrap_or("").to_string(),
        })
        .collect()
}

fn parse_jobs(val: &serde_json::Value) -> Vec<WorkflowJob> {
    val["jobs"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|j| {
            let started = j["started_at"].as_str().unwrap_or("");
            let completed = j["completed_at"].as_str().unwrap_or("");
            let duration_secs = compute_duration_secs(started, completed);
            WorkflowJob {
                name: j["name"].as_str().unwrap_or("").to_string(),
                status: j["status"].as_str().unwrap_or("").to_string(),
                conclusion: j["conclusion"].as_str().map(|s| s.to_string()),
                duration_secs,
            }
        })
        .collect()
}

/// Very simple duration parser — parses ISO 8601 "YYYY-MM-DDTHH:MM:SSZ" into
/// seconds since Unix epoch (ignoring leap seconds / sub-seconds).
fn iso_to_secs(s: &str) -> Option<u64> {
    // "2026-03-13T07:46:59Z"
    let s = s.strip_suffix('Z').unwrap_or(s);
    let (date, time) = s.split_once('T')?;
    let mut dp = date.splitn(3, '-');
    let y: u64 = dp.next()?.parse().ok()?;
    let m: u64 = dp.next()?.parse().ok()?;
    let d: u64 = dp.next()?.parse().ok()?;
    let mut tp = time.splitn(3, ':');
    let h: u64 = tp.next()?.parse().ok()?;
    let mi: u64 = tp.next()?.parse().ok()?;
    let se: u64 = tp.next()?.parse().ok()?;
    // Rough epoch calculation (good enough for duration deltas)
    let days = (y - 1970) * 365 + (y - 1969) / 4 + month_days(y, m) + d - 1;
    Some(days * 86400 + h * 3600 + mi * 60 + se)
}

fn month_days(year: u64, month: u64) -> u64 {
    let months = [0u64, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let base = if (1..=12).contains(&month) {
        months[(month - 1) as usize]
    } else {
        0
    };
    // Add leap day if month > 2 and it's a leap year
    let leap = if month > 2
        && year.is_multiple_of(4)
        && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    {
        1
    } else {
        0
    };
    base + leap
}

fn compute_duration_secs(started: &str, completed: &str) -> u64 {
    match (iso_to_secs(started), iso_to_secs(completed)) {
        (Some(s), Some(c)) if c >= s => c - s,
        _ => 0,
    }
}

fn format_duration(secs: u64) -> String {
    if secs == 0 {
        return String::new();
    }
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

// ─── Panel ──────────────────────────────────────────────────────────────────

pub fn github_actions_panel(state: IdeState) -> impl IntoView {
    let theme = state.theme;

    // Signals
    let repo_label: RwSignal<String> = create_rw_signal("Loading...".to_string());
    let runs: RwSignal<Vec<WorkflowRun>> = create_rw_signal(Vec::new());
    let error_msg: RwSignal<Option<String>> = create_rw_signal(None);
    // Map from run_id → expanded jobs list (None = not fetched, Some(vec) = loaded)
    let expanded: RwSignal<Vec<(u64, Option<Vec<WorkflowJob>>)>> = create_rw_signal(Vec::new());
    let loading: RwSignal<bool> = create_rw_signal(false);

    // Shared owner/repo
    let owner_repo: Arc<std::sync::Mutex<Option<(String, String)>>> =
        Arc::new(std::sync::Mutex::new(None));
    let owner_repo_fetch = Arc::clone(&owner_repo);

    // ── Shared channel for fetch results (initial + refresh + manual) ──
    let (fetch_tx, fetch_rx) = std::sync::mpsc::sync_channel::<(String, Result<Vec<WorkflowRun>, String>)>(1);
    let fetch_result = create_signal_from_channel(fetch_rx);
    {
        let runs_sig = runs;
        let repo_label_sig = repo_label;
        let error_sig = error_msg;
        let loading_sig = loading;
        let expanded_sig = expanded;
        create_effect(move |_| {
            if let Some((lbl, result)) = fetch_result.get() {
                loading_sig.set(false);
                repo_label_sig.set(lbl);
                match result {
                    Ok(r) => {
                        error_sig.set(None);
                        expanded_sig.update(|exp| {
                            for run in &r {
                                if !exp.iter().any(|(id, _)| *id == run.id) {
                                    exp.push((run.id, None));
                                }
                            }
                        });
                        runs_sig.set(r);
                    }
                    Err(e) => {
                        error_sig.set(Some(e));
                    }
                }
            }
        });
    }

    // ── Initial fetch ──
    {
        loading.set(true);
        let tx = fetch_tx.clone();
        std::thread::spawn(move || {
            let token = get_gh_token();
            let pair = parse_owner_repo();
            let label;
            let result = match pair {
                None => {
                    label = "No GitHub remote".to_string();
                    Err("Could not parse owner/repo from git remote".to_string())
                }
                Some((owner, repo)) => {
                    label = format!("{}/{}", owner, repo);
                    if let Ok(mut guard) = owner_repo_fetch.lock() {
                        *guard = Some((owner.clone(), repo.clone()));
                    }
                    let url = format!(
                        "https://api.github.com/repos/{}/{}/actions/runs?per_page=15",
                        owner, repo
                    );
                    match fetch_json(&url, token.as_deref()) {
                        Ok(v) => Ok(parse_runs(&v)),
                        Err(e) => Err(e),
                    }
                }
            };
            let _ = tx.send((label, result));
        });
    }

    // ── Auto-refresh every 30s when any run is in_progress ──
    // Use a standalone timer thread (NOT create_effect subscribing to runs_sig),
    // which would create a reactive loop: runs changes → effect fires → spawns thread
    // → thread updates runs → effect fires again → infinite loop freezing the UI.
    // Now uses shared fetch_tx channel — no Scope::new() needed, and auto-refresh
    // actually continues working (old code broke after first update).
    {
        let owner_repo_timer = Arc::clone(&owner_repo);
        let tx = fetch_tx.clone();
        std::thread::spawn(move || loop {
            // Sleep in 1s intervals so we can detect channel disconnect quickly
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            let pair = owner_repo_timer.lock().ok().and_then(|g| g.clone());
            let Some((owner, repo)) = pair else { continue };
            let token = get_gh_token();
            let url = format!(
                "https://api.github.com/repos/{}/{}/actions/runs?per_page=15",
                owner, repo
            );
            let label = format!("{}/{}", owner, repo);
            let result = match fetch_json(&url, token.as_deref()) {
                Ok(v) => Ok(parse_runs(&v)),
                Err(e) => Err(e),
            };
            if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) = tx.try_send((label, result)) {
                break;
            }
        });
    }

    // ── Manual Refresh — uses shared fetch_tx, no Scope::new() ──
    let refresh_fn = {
        let loading_sig = loading;
        let owner_repo_ref = Arc::clone(&owner_repo);
        let tx = fetch_tx.clone();

        move || {
            let owner_repo_arc = Arc::clone(&owner_repo_ref);
            loading_sig.set(true);
            let tx = tx.clone();
            std::thread::spawn(move || {
                let pair = owner_repo_arc.lock().ok().and_then(|g| g.clone());
                match pair {
                    None => {
                        let _ = tx.send(("No GitHub remote".to_string(), Err("No owner/repo available".to_string())));
                    }
                    Some((owner, repo)) => {
                        let label = format!("{}/{}", owner, repo);
                        let token = get_gh_token();
                        let url = format!(
                            "https://api.github.com/repos/{}/{}/actions/runs?per_page=15",
                            owner, repo
                        );
                        let result = match fetch_json(&url, token.as_deref()) {
                            Ok(v) => Ok(parse_runs(&v)),
                            Err(e) => Err(e),
                        };
                        let _ = tx.send((label, result));
                    }
                }
            });
        }
    };

    // ── Job expansion — channel created once, no Scope::new() per click ──
    let (jobs_tx, jobs_rx) =
        std::sync::mpsc::sync_channel::<(u64, Vec<WorkflowJob>)>(1);
    let jobs_result = create_signal_from_channel(jobs_rx);
    {
        let expanded_sig = expanded;
        create_effect(move |_| {
            if let Some((run_id, job_list)) = jobs_result.get() {
                expanded_sig.update(|exp| {
                    for (id, jobs) in exp.iter_mut() {
                        if *id == run_id {
                            *jobs = Some(job_list.clone());
                        }
                    }
                });
            }
        });
    }
    let expand_run = {
        let expanded_sig = expanded;
        let owner_repo_ref = Arc::clone(&owner_repo);

        move |run_id: u64| {
            // Check if already expanded — if so, collapse
            let already = expanded_sig
                .get()
                .iter()
                .any(|(id, jobs)| *id == run_id && jobs.is_some());
            if already {
                expanded_sig.update(|exp| {
                    for (id, jobs) in exp.iter_mut() {
                        if *id == run_id {
                            *jobs = None;
                        }
                    }
                });
                return;
            }

            let owner_repo_arc = Arc::clone(&owner_repo_ref);
            let tx = jobs_tx.clone();
            std::thread::spawn(move || {
                let pair = owner_repo_arc.lock().ok().and_then(|g| g.clone());
                match pair {
                    None => {} // no owner/repo, nothing to do
                    Some((owner, repo)) => {
                        let token = get_gh_token();
                        let url = format!(
                            "https://api.github.com/repos/{}/{}/actions/runs/{}/jobs",
                            owner, repo, run_id
                        );
                        if let Ok(v) = fetch_json(&url, token.as_deref()) {
                            let _ = tx.send((run_id, parse_jobs(&v)));
                        }
                    }
                }
            });
        }
    };

    // ── Build UI ──

    let header = h_stack((
        label(move || {
            let lbl = repo_label.get();
            let is_loading = loading.get();
            if is_loading {
                format!("⏳ {}", lbl)
            } else {
                lbl
            }
        })
        .style(move |s| {
            s.font_size(12.0)
                .color(theme.get().palette.text_muted)
                .flex_grow(1.0)
        }),
        container(label(|| "↻ Refresh").style(move |s| {
            s.font_size(11.0)
                .color(theme.get().palette.accent)
                .padding_horiz(8.0)
                .padding_vert(3.0)
                .cursor(floem::style::CursorStyle::Pointer)
        }))
        .on_click_stop({
            let refresh_fn = refresh_fn.clone();
            move |_| {
                refresh_fn();
            }
        }),
    ))
    .style(move |s| {
        s.width_full()
            .padding_horiz(10.0)
            .padding_vert(6.0)
            .align_items(floem::taffy::AlignItems::Center)
            .border_bottom(1.0)
            .border_color(theme.get().palette.border)
    });

    let error_view = container(label(move || error_msg.get().unwrap_or_default()).style(
        move |s| {
            let show = error_msg.get().is_some();
            s.font_size(11.0)
                .color(floem::peniko::Color::from_rgb8(220, 80, 80))
                .padding(8.0)
                .apply_if(!show, |s| s.display(floem::style::Display::None))
        },
    ));

    let runs_list = dyn_stack(
        move || runs.get(),
        |run| run.id,
        move |run| {
            let run_id = run.id;
            let icon = status_icon(&run.status, run.conclusion.as_deref());
            let icon_color = status_color(&run.status, run.conclusion.as_deref());
            let time_str = time_ago(&run.updated_at);
            let msg_short: String = run.head_commit_message.chars().take(40).collect();

            // Build run row display string
            let row_label = format!(
                "  {} · {} · \"{}\" · {}",
                run.name, run.head_branch, msg_short, time_str
            );

            let jobs_view = dyn_stack(
                move || {
                    expanded
                        .get()
                        .into_iter()
                        .find(|(id, _)| *id == run_id)
                        .and_then(|(_, jobs)| jobs)
                        .unwrap_or_default()
                },
                |job| job.name.clone(),
                move |job| {
                    let j_icon = status_icon(&job.status, job.conclusion.as_deref());
                    let j_color = status_color(&job.status, job.conclusion.as_deref());
                    let dur = format_duration(job.duration_secs);
                    let job_label = if dur.is_empty() {
                        format!("    {} {}", j_icon, job.name)
                    } else {
                        format!("    {} {} ({})", j_icon, job.name, dur)
                    };
                    h_stack((
                        label(move || "  ")
                            .style(move |s| s.color(j_color).font_size(12.0).width(16.0)),
                        label(move || job_label.clone()).style(move |s| {
                            s.font_size(11.0)
                                .color(theme.get().palette.text_muted)
                                .padding_vert(1.0)
                        }),
                    ))
                    .style(|s| s.width_full().padding_left(24.0))
                },
            )
            .style(move |s| {
                let is_expanded = expanded
                    .get()
                    .iter()
                    .any(|(id, jobs)| *id == run_id && jobs.is_some());
                s.width_full()
                    .flex_col()
                    .apply_if(!is_expanded, |s| s.display(floem::style::Display::None))
            });

            v_stack((
                h_stack((
                    label(move || icon)
                        .style(move |s| s.color(icon_color).font_size(13.0).width(18.0)),
                    label(move || row_label.clone()).style(move |s| {
                        s.font_size(11.0)
                            .color(theme.get().palette.text_primary)
                            .flex_grow(1.0)
                    }),
                ))
                .on_click_stop({
                    let expand_run = expand_run.clone();
                    move |_| expand_run(run_id)
                })
                .style(move |s| {
                    s.width_full()
                        .padding_vert(4.0)
                        .padding_horiz(8.0)
                        .align_items(floem::taffy::AlignItems::Center)
                        .cursor(floem::style::CursorStyle::Pointer)
                }),
                jobs_view,
            ))
            .style(move |s| {
                s.width_full()
                    .flex_col()
                    .border_bottom(1.0)
                    .border_color(theme.get().palette.border.with_alpha(0.3))
            })
        },
    )
    .style(|s| s.width_full().flex_col());

    let panel_header = h_stack((label(move || "GITHUB ACTIONS").style(move |s| {
        s.font_size(11.0)
            .color(theme.get().palette.text_muted)
            .font_weight(floem::text::Weight::BOLD)
            .flex_grow(1.0)
    }),))
    .style(move |s| {
        s.width_full()
            .padding_horiz(10.0)
            .padding_vert(8.0)
            .border_bottom(1.0)
            .border_color(theme.get().palette.border)
    });

    v_stack((
        panel_header,
        header,
        error_view,
        scroll(runs_list).style(|s| s.width_full().flex_grow(1.0)),
    ))
    .style(|s| s.width_full().height_full().flex_col())
}
