//! `phazeai worktree …` and `phazeai --worktree`: agents that each work in
//! their own git worktree, so several can run at once without touching each
//! other or your checkout.
//!
//! `phazeai worktree run --task "…" --task "…"` makes one worktree per task,
//! runs a `phazeai -p` agent in each at the same time, commits what each one
//! changed on its own `phaze/<name>` branch and prints what came back. You
//! then look at a branch (`git diff HEAD...phaze/<name>`) and merge it with
//! `phazeai worktree merge <name>`, or drop it with `phazeai worktree remove`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use phazeai_core::worktree::{self, Worktree};

use crate::oneshot::Allow;

#[derive(clap::Args, Debug)]
pub struct WorktreeArgs {
    #[command(subcommand)]
    pub action: WorktreeAction,
}

#[derive(clap::Subcommand, Debug)]
pub enum WorktreeAction {
    /// Show the agent worktrees of this repository and what each holds.
    List,
    /// Run one agent per task, each in its own worktree, all at once.
    Run(RunArgs),
    /// Merge a worktree's branch into the branch you are on.
    Merge {
        /// The worktree's name, as `phazeai worktree list` shows it.
        name: String,
        /// Keep the worktree after merging (it is removed by default).
        #[arg(long)]
        keep: bool,
    },
    /// Delete a worktree and its branch.
    Remove {
        name: String,
        /// Delete it even if it holds work that is not merged.
        #[arg(long)]
        force: bool,
    },
}

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    /// A task for one agent. Repeat for more agents: `--task "…" --task "…"`.
    #[arg(long = "task", required = true, value_name = "TASK")]
    pub tasks: Vec<String>,
    /// What each agent may do without asking (nobody is there to ask): read,
    /// edit (default; change files, read-only shell) or all.
    #[arg(long, value_enum, default_value_t = Allow::Edit)]
    pub allow: Allow,
    /// How many agents run at the same time.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u32).range(1..=32))]
    pub jobs: u32,
    /// LLM model for every agent.
    #[arg(short, long)]
    pub model: Option<String>,
    /// LLM provider for every agent.
    #[arg(long)]
    pub provider: Option<String>,
    /// The most model turns each agent may take.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub max_turns: Option<u32>,
}

/// How one agent's run ended.
#[derive(Debug)]
pub struct TaskOutcome {
    pub task: String,
    pub worktree: Worktree,
    pub success: bool,
    pub seconds: f64,
    pub changed_files: Vec<String>,
    pub log: PathBuf,
}

pub async fn run(args: WorktreeArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = worktree::repo_root(&cwd)?;
    match args.action {
        WorktreeAction::List => list(&repo),
        WorktreeAction::Merge { name, keep } => {
            let wt = worktree::find(&repo, &name)?;
            worktree::merge(&repo, &wt)?;
            println!("Merged {} into your branch.", wt.branch);
            if !keep {
                worktree::remove(&repo, &wt, false)?;
                println!("Removed the worktree {}.", wt.name);
            }
            Ok(())
        }
        WorktreeAction::Remove { name, force } => {
            let wt = worktree::find(&repo, &name)?;
            worktree::remove(&repo, &wt, force)?;
            println!("Removed the worktree {} and its branch.", wt.name);
            Ok(())
        }
        WorktreeAction::Run(run) => {
            let exe = std::env::current_exe().context("couldn't find the phazeai binary")?;
            let outcomes = run_tasks(&repo, &exe, run).await?;
            print!("{}", summary(&outcomes));
            if outcomes.iter().all(|o| o.success) {
                Ok(())
            } else {
                anyhow::bail!("some agents failed; their logs are listed above")
            }
        }
    }
}

fn list(repo: &Path) -> Result<()> {
    let all = worktree::list(repo)?;
    if all.is_empty() {
        println!("No agent worktrees. Start some with: phazeai worktree run --task \"…\"");
        return Ok(());
    }
    for wt in all {
        let state = worktree::state(repo, &wt)?;
        let what = if state.is_empty() {
            "nothing new".to_string()
        } else {
            let mut parts = Vec::new();
            if state.commits_ahead > 0 {
                parts.push(format!(
                    "{} commit(s), {} file(s) changed",
                    state.commits_ahead,
                    state.changed_files.len()
                ));
            }
            if !state.uncommitted.is_empty() {
                parts.push(format!("{} uncommitted", state.uncommitted.len()));
            }
            parts.join(", ")
        };
        println!("{:<24} {what}\n{:<24} {}", wt.name, "", wt.path.display());
    }
    Ok(())
}

/// The arguments for one agent's `phazeai -p` run.
fn child_args(task: &str, name: &str, run: &RunArgs) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        task.to_string(),
        "--worktree".to_string(),
        name.to_string(),
        "--allow".to_string(),
        match run.allow {
            Allow::Read => "read",
            Allow::Edit => "edit",
            Allow::All => "all",
        }
        .to_string(),
        "--no-color".to_string(),
    ];
    if let Some(m) = &run.model {
        args.extend(["--model".into(), m.clone()]);
    }
    if let Some(p) = &run.provider {
        args.extend(["--provider".into(), p.clone()]);
    }
    if let Some(t) = run.max_turns {
        args.extend(["--max-turns".into(), t.to_string()]);
    }
    args
}

/// Make a worktree per task, run `exe -p <task> --worktree <name>` in each
/// (at most `jobs` at once), and report what each changed. Worktrees are made
/// one after another first, since git locks the repository while it adds one.
pub async fn run_tasks(repo: &Path, exe: &Path, run: RunArgs) -> Result<Vec<TaskOutcome>> {
    let mut taken: Vec<String> = worktree::list(repo)?.into_iter().map(|w| w.name).collect();
    let mut planned = Vec::new();
    for task in &run.tasks {
        let name = worktree::name_for_task(task, &taken);
        taken.push(name.clone());
        let wt = worktree::create(repo, &name)?;
        eprintln!("{}: started in {}", wt.name, wt.path.display());
        planned.push((task.clone(), wt));
    }

    let logs = worktree::worktrees_dir(repo).join("logs");
    std::fs::create_dir_all(&logs)?;
    let limit = Arc::new(tokio::sync::Semaphore::new(run.jobs as usize));
    let run = Arc::new(run);
    let mut handles = Vec::new();
    for (task, wt) in planned {
        let limit = limit.clone();
        let run = run.clone();
        let exe = exe.to_path_buf();
        let log = logs.join(format!("{}.log", wt.name));
        handles.push(tokio::spawn(async move {
            let _slot = limit.acquire_owned().await;
            let started = Instant::now();
            let success = run_one(&exe, &task, &wt, &run, &log).await;
            (task, wt, success, started.elapsed().as_secs_f64(), log)
        }));
    }

    let mut outcomes = Vec::new();
    for handle in handles {
        let (task, wt, success, seconds, log) = handle.await?;
        // The agent commits its own work; anything it left (a crash, a kill)
        // is committed here so nothing is lost when the worktree is merged.
        let _ = worktree::commit_all(&wt, &commit_message(&task));
        let changed_files = worktree::state(repo, &wt)
            .map(|s| s.changed_files)
            .unwrap_or_default();
        eprintln!(
            "{}: {} after {seconds:.0}s, {} file(s) changed",
            wt.name,
            if success { "finished" } else { "failed" },
            changed_files.len()
        );
        outcomes.push(TaskOutcome {
            task,
            worktree: wt,
            success,
            seconds,
            changed_files,
            log,
        });
    }
    Ok(outcomes)
}

async fn run_one(exe: &Path, task: &str, wt: &Worktree, run: &RunArgs, log: &Path) -> bool {
    let Ok(file) = std::fs::File::create(log) else {
        return false;
    };
    let Ok(err_file) = file.try_clone() else {
        return false;
    };
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(child_args(task, &wt.name, run))
        .current_dir(&wt.path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err_file))
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.status().await {
        Ok(status) => status.success(),
        Err(e) => {
            let _ = std::fs::write(log, format!("couldn't start the agent: {e}\n"));
            false
        }
    }
}

/// A commit message from a task: its first line, trimmed to fit.
pub fn commit_message(task: &str) -> String {
    let first = task.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let first = first.trim();
    let mut msg: String = first.chars().take(72).collect();
    if first.chars().count() > 72 {
        msg.push('…');
    }
    if msg.is_empty() {
        "Agent changes".into()
    } else {
        msg
    }
}

/// What `phazeai worktree run` prints at the end.
pub fn summary(outcomes: &[TaskOutcome]) -> String {
    let mut out = String::new();
    for o in outcomes {
        let status = if o.success { "done" } else { "FAILED" };
        out.push_str(&format!(
            "\n{} [{status}, {:.0}s] {}\n",
            o.worktree.name,
            o.seconds,
            commit_message(&o.task)
        ));
        if o.changed_files.is_empty() {
            out.push_str("  no files changed\n");
        } else {
            for f in o.changed_files.iter().take(10) {
                out.push_str(&format!("  {f}\n"));
            }
            if o.changed_files.len() > 10 {
                out.push_str(&format!("  … and {} more\n", o.changed_files.len() - 10));
            }
        }
        out.push_str(&format!("  log: {}\n", o.log.display()));
    }
    let with_changes: Vec<&TaskOutcome> = outcomes
        .iter()
        .filter(|o| !o.changed_files.is_empty())
        .collect();
    if let Some(first) = with_changes.first() {
        out.push_str(&format!(
            "\nReview a branch with `git diff HEAD...{}`, then `phazeai worktree merge {}` \
             (or `phazeai worktree remove {} --force` to drop it).\n",
            first.worktree.branch, first.worktree.name, first.worktree.name
        ));
    }
    out
}

/// For `phazeai --worktree [NAME]`: open (or make) the worktree, and return
/// it so the caller can move into it before anything else reads the cwd.
pub fn enter(name: Option<&str>, prompt: Option<&str>) -> Result<Worktree> {
    let cwd = std::env::current_dir()?;
    let repo = worktree::repo_root(&cwd)?;
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => {
            let taken: Vec<String> = worktree::list(&repo)?.into_iter().map(|w| w.name).collect();
            worktree::name_for_task(prompt.unwrap_or("session"), &taken)
        }
    };
    let wt = worktree::open_or_create(&repo, &name)?;
    std::env::set_current_dir(&wt.path)
        .with_context(|| format!("couldn't enter {}", wt.path.display()))?;
    Ok(wt)
}

/// After a `--worktree` run: commit what the agent changed and say where it is.
pub fn leave(wt: &Worktree, prompt: Option<&str>) {
    let message = commit_message(prompt.unwrap_or("PhazeAI session"));
    match worktree::commit_all(wt, &message) {
        Ok(true) => eprintln!(
            "Committed the changes on {}. Review: git diff HEAD...{}  Merge: phazeai worktree merge {}",
            wt.branch, wt.branch, wt.name
        ),
        Ok(false) => {}
        Err(e) => eprintln!("couldn't commit the changes in {}: {e}", wt.path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_messages_are_one_short_line() {
        assert_eq!(
            commit_message("  Fix the login bug\nmore detail"),
            "Fix the login bug"
        );
        assert_eq!(commit_message("\n\n"), "Agent changes");
        let long = "x".repeat(100);
        assert_eq!(commit_message(&long).chars().count(), 73);
    }

    #[test]
    fn each_agent_gets_its_task_its_worktree_and_the_shared_flags() {
        let run = RunArgs {
            tasks: vec![],
            allow: Allow::Edit,
            jobs: 2,
            model: Some("m".into()),
            provider: None,
            max_turns: Some(30),
        };
        assert_eq!(
            child_args("Fix it", "fix-it", &run),
            [
                "-p",
                "Fix it",
                "--worktree",
                "fix-it",
                "--allow",
                "edit",
                "--no-color",
                "--model",
                "m",
                "--max-turns",
                "30"
            ]
        );
    }
}
