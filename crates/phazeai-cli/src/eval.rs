//! `phazeai eval`: does the agent actually finish real coding tasks, and what
//! does each one cost? The same tasks run against Phaze with any provider
//! (Claude, Grok, a local model), against Claude Code, or against any other
//! agent that takes a prompt on its command line, so the numbers compare.
//!
//! A task is a folder:
//!
//! ```text
//! evals/<task>/
//!   task.toml   prompt = "…", check = "…", and optional time limits
//!   repo/       the code the agent starts from
//!   check/      hidden tests, copied in only after the agent has finished
//!   solution/   (optional) files that solve it, for `--verify`
//! ```
//!
//! Each run copies `repo/` to a fresh folder, makes it a git repository with
//! one commit, runs the agent there, then copies `check/` over it and runs
//! `check` in the shell. Exit 0 means solved. The agent never sees `check/`,
//! so it cannot write code that only satisfies the grader's tests.
//!
//! Token counts and cost come from the agent's JSON result, which Phaze's
//! `--output-format json` and Claude Code's share. An agent that prints none
//! is still graded; its tokens show as unknown.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long an agent gets on a task when `task.toml` doesn't say.
const DEFAULT_AGENT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long the check gets when `task.toml` doesn't say.
const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// The most of a check's output kept with a failed run, in bytes.
const MAX_CHECK_OUTPUT: usize = 4 * 1024;

/// `phazeai eval`'s arguments.
#[derive(clap::Args, Debug, Clone)]
pub struct EvalArgs {
    /// The folder of tasks.
    #[arg(default_value = "evals")]
    pub dir: PathBuf,

    /// The agent to run: `phaze` (this program), `claude-code`, or a shell
    /// command that reads the task from the `PHAZE_EVAL_PROMPT` environment
    /// variable, such as `my-agent --print "$PHAZE_EVAL_PROMPT"`.
    #[arg(long, default_value = "phaze")]
    pub agent: String,

    /// Provider for the `phaze` agent (as `phazeai --provider`).
    #[arg(long)]
    pub provider: Option<String>,

    /// Model for the `phaze` or `claude-code` agent.
    #[arg(short, long)]
    pub model: Option<String>,

    /// Only the tasks whose name contains this (repeatable).
    #[arg(long = "task", value_name = "TEXT")]
    pub tasks: Vec<String>,

    /// How many times to run each task.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub runs: u32,

    /// Append one JSON line per run to this file.
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,

    /// Keep each run's folder, to look at what the agent did.
    #[arg(long)]
    pub keep: bool,

    /// Run no agent: check that each task fails as given and passes with its
    /// `solution/`, so a broken task never counts against an agent.
    #[arg(long)]
    pub verify: bool,
}

/// `task.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    /// What the agent is asked, as a user would ask it.
    pub prompt: String,
    /// The shell command that grades the result: exit 0 is solved.
    pub check: String,
    /// One line on what the task tests, for the report.
    #[serde(default)]
    pub about: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub check_timeout_secs: Option<u64>,
}

/// A task folder, read and checked.
#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    pub dir: PathBuf,
    pub spec: TaskSpec,
}

impl Task {
    fn repo(&self) -> PathBuf {
        self.dir.join("repo")
    }
    fn agent_timeout(&self) -> Duration {
        self.spec
            .timeout_secs
            .map_or(DEFAULT_AGENT_TIMEOUT, |s| Duration::from_secs(s.max(1)))
    }
    fn check_timeout(&self) -> Duration {
        self.spec
            .check_timeout_secs
            .map_or(DEFAULT_CHECK_TIMEOUT, |s| Duration::from_secs(s.max(1)))
    }
}

/// Reads every task under `dir`, sorted by name, keeping those whose name
/// contains one of `filters` (all of them when there are none).
pub fn load_tasks(dir: &Path, filters: &[String]) -> Result<Vec<Task>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("couldn't read the task folder {}", dir.display()))?;
    let mut tasks = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let spec_path = path.join("task.toml");
        if !spec_path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        let text = std::fs::read_to_string(&spec_path)?;
        let spec: TaskSpec =
            toml::from_str(&text).with_context(|| format!("{}", spec_path.display()))?;
        if spec.prompt.trim().is_empty() || spec.check.trim().is_empty() {
            bail!(
                "{}: `prompt` and `check` can't be empty",
                spec_path.display()
            );
        }
        if !path.join("repo").is_dir() {
            bail!("task {name} has no repo/ folder");
        }
        tasks.push(Task {
            name,
            dir: path,
            spec,
        });
    }
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    if tasks.is_empty() {
        bail!(
            "no tasks in {}{}",
            dir.display(),
            if filters.is_empty() {
                ""
            } else {
                " matching --task"
            }
        );
    }
    Ok(tasks)
}

/// Which agent runs, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Agent {
    /// This program, with `-p --output-format json --allow all`.
    Phaze {
        exe: PathBuf,
        provider: Option<String>,
        model: Option<String>,
    },
    /// Anthropic's Claude Code, with `-p --output-format json`, allowed to
    /// edit files and run commands.
    ClaudeCode { model: Option<String> },
    /// Any command, run in the shell with the task in `PHAZE_EVAL_PROMPT`.
    Shell(String),
}

impl Agent {
    pub fn from_args(args: &EvalArgs) -> Result<Self> {
        Ok(match args.agent.as_str() {
            "phaze" => Agent::Phaze {
                exe: std::env::current_exe().context("couldn't find this program")?,
                provider: args.provider.clone(),
                model: args.model.clone(),
            },
            "claude-code" | "claude" => {
                if args.provider.is_some() {
                    bail!("--provider is for the phaze agent; Claude Code uses its own");
                }
                Agent::ClaudeCode {
                    model: args.model.clone(),
                }
            }
            command => {
                if args.provider.is_some() || args.model.is_some() {
                    bail!("--provider and --model are for the phaze and claude-code agents; put them in your command");
                }
                Agent::Shell(command.to_string())
            }
        })
    }

    /// A short name for the report.
    pub fn label(&self) -> String {
        match self {
            Agent::Phaze {
                provider, model, ..
            } => {
                let mut s = "phaze".to_string();
                if let Some(p) = provider {
                    let _ = write!(s, " --provider {p}");
                }
                if let Some(m) = model {
                    let _ = write!(s, " --model {m}");
                }
                s
            }
            Agent::ClaudeCode { model } => match model {
                Some(m) => format!("claude-code --model {m}"),
                None => "claude-code".into(),
            },
            Agent::Shell(cmd) => cmd.clone(),
        }
    }

    fn command(&self, prompt: &str) -> tokio::process::Command {
        let mut cmd = match self {
            Agent::Phaze {
                exe,
                provider,
                model,
            } => {
                let mut c = phazeai_core::process::tokio_command(exe);
                c.args(["-p", prompt, "--output-format", "json", "--allow", "all"]);
                c.arg("--no-color");
                if let Some(p) = provider {
                    c.args(["--provider", p]);
                }
                if let Some(m) = model {
                    c.args(["--model", m]);
                }
                c
            }
            Agent::ClaudeCode { model } => {
                let mut c = phazeai_core::process::tokio_command("claude");
                c.args(["-p", prompt, "--output-format", "json"]);
                // The same reach as Phaze's `--allow all`: edit files and run
                // any command, in the run's own throwaway folder.
                c.args(["--permission-mode", "acceptEdits", "--allowedTools", "Bash"]);
                if let Some(m) = model {
                    c.args(["--model", m]);
                }
                c
            }
            Agent::Shell(command) => {
                let mut c = shell(command);
                c.env("PHAZE_EVAL_PROMPT", prompt);
                c
            }
        };
        cmd.stdin(std::process::Stdio::null());
        cmd
    }
}

fn shell(command: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut c = phazeai_core::process::tokio_command("cmd");
        c.arg("/C").arg(command);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = phazeai_core::process::tokio_command("sh");
        c.arg("-c").arg(command);
        c
    }
}

/// What an agent's JSON result said about its run.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AgentReport {
    pub is_error: Option<bool>,
    pub turns: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

/// Reads the result object from an agent's stdout: the whole output as one
/// JSON value (`--output-format json`), or else the last line that is a
/// `"type": "result"` object (`stream-json`). `None` when there is neither.
pub fn parse_report(stdout: &str) -> Option<AgentReport> {
    let result = serde_json::from_str::<Value>(stdout.trim())
        .ok()
        .filter(|v| v.get("type").and_then(Value::as_str) == Some("result"))
        .or_else(|| {
            stdout.lines().rev().find_map(|line| {
                serde_json::from_str::<Value>(line.trim())
                    .ok()
                    .filter(|v| v.get("type").and_then(Value::as_str) == Some("result"))
            })
        })?;
    let usage = result.get("usage");
    let count = |key: &str| usage.and_then(|u| u.get(key)).and_then(Value::as_u64);
    Some(AgentReport {
        is_error: result.get("is_error").and_then(Value::as_bool),
        turns: result.get("num_turns").and_then(Value::as_u64),
        input_tokens: count("input_tokens"),
        cache_read_tokens: count("cache_read_input_tokens"),
        cache_write_tokens: count("cache_creation_input_tokens"),
        output_tokens: count("output_tokens"),
        cost_usd: result.get("total_cost_usd").and_then(Value::as_f64),
    })
}

/// One run of one task.
#[derive(Debug, Clone, Serialize)]
pub struct RunRecord {
    pub task: String,
    pub agent: String,
    pub run: u32,
    pub solved: bool,
    /// The agent's exit code; `None` when it was stopped or killed.
    pub agent_exit: Option<i32>,
    pub agent_timed_out: bool,
    pub agent_seconds: f64,
    #[serde(flatten)]
    pub report: AgentReport,
    /// The end of the check's output, for a run that wasn't solved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_output: Option<String>,
    /// Why the run couldn't be graded, if it couldn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Copies a folder's contents into `to`, which may exist (files there are
/// replaced). Symbolic links are copied as the files they point to.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// A fresh folder for one run, removed on drop unless kept.
struct WorkDir {
    path: PathBuf,
    keep: bool,
}

impl WorkDir {
    fn new(task: &str, run: u32, keep: bool) -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "phaze-eval-{task}-{run}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path, keep })
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Makes `dir` a git repository with everything committed, as a project the
/// agent is handed would be. Agents use git to see and undo their changes.
async fn git_snapshot(dir: &Path) -> Result<()> {
    for args in [
        &["init", "-q"][..],
        &["add", "-A"],
        &[
            "-c",
            "user.name=phaze-eval",
            "-c",
            "user.email=eval@localhost",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "Starting point",
        ],
    ] {
        let mut cmd = phazeai_core::process::tokio_command("git");
        cmd.args(args).current_dir(dir);
        let out = phazeai_core::process::output_or_kill(cmd, Duration::from_secs(60))
            .await
            .context("couldn't run git, which the eval needs")?
            .context("git took over a minute")?;
        if !out.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }
    Ok(())
}

/// Runs a task's check in `dir`: Ok(true) for exit 0, with the output's end.
async fn run_check(task: &Task, dir: &Path) -> Result<(bool, String)> {
    let mut cmd = shell(&task.spec.check);
    cmd.current_dir(dir).stdin(std::process::Stdio::null());
    let out = phazeai_core::process::output_or_kill(cmd, task.check_timeout())
        .await
        .with_context(|| format!("couldn't start the check `{}`", task.spec.check))?;
    let Some(out) = out else {
        return Ok((
            false,
            format!(
                "the check ran over {} s and was stopped",
                task.check_timeout().as_secs()
            ),
        ));
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), tail(&text, MAX_CHECK_OUTPUT)))
}

/// The last `max` bytes of `text`, on a character boundary.
fn tail(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

/// Runs `agent` on `task` once and grades the result.
pub async fn run_task(agent: &Agent, task: &Task, run: u32, keep: bool) -> RunRecord {
    let mut record = RunRecord {
        task: task.name.clone(),
        agent: agent.label(),
        run,
        solved: false,
        agent_exit: None,
        agent_timed_out: false,
        agent_seconds: 0.0,
        report: AgentReport::default(),
        check_output: None,
        error: None,
    };
    let work = match WorkDir::new(&task.name, run, keep) {
        Ok(w) => w,
        Err(e) => {
            record.error = Some(format!("couldn't make a folder for the run: {e}"));
            return record;
        }
    };
    if let Err(e) = copy_tree(&task.repo(), &work.path) {
        record.error = Some(format!("couldn't copy repo/: {e}"));
        return record;
    }
    if let Err(e) = git_snapshot(&work.path).await {
        record.error = Some(format!("{e:#}"));
        return record;
    }

    let mut cmd = agent.command(task.spec.prompt.trim());
    cmd.current_dir(&work.path);
    let started = Instant::now();
    let output = phazeai_core::process::output_or_kill(cmd, task.agent_timeout()).await;
    record.agent_seconds = started.elapsed().as_secs_f64();
    match output {
        Err(e) => {
            record.error = Some(format!("couldn't start the agent: {e}"));
            return record;
        }
        Ok(None) => record.agent_timed_out = true,
        Ok(Some(out)) => {
            record.agent_exit = out.status.code();
            if let Some(report) = parse_report(&String::from_utf8_lossy(&out.stdout)) {
                record.report = report;
            }
        }
    }

    // Grade what the agent left, timed out or not: the hidden tests go in
    // now, over anything the agent wrote with the same names.
    let check_dir = task.dir.join("check");
    if check_dir.is_dir() {
        if let Err(e) = copy_tree(&check_dir, &work.path) {
            record.error = Some(format!("couldn't copy check/: {e}"));
            return record;
        }
    }
    match run_check(task, &work.path).await {
        Ok((solved, output)) => {
            record.solved = solved;
            if !solved {
                record.check_output = Some(output);
            }
        }
        Err(e) => record.error = Some(format!("{e:#}")),
    }
    if keep {
        eprintln!("  kept {}", work.path.display());
    }
    record
}

/// One task's verification, for `--verify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Fails as given and passes with `solution/`.
    Sound,
    /// Has no `solution/` to try; only the failing start was checked.
    FailsAsGiven,
    /// Something is wrong with the task itself.
    Broken(String),
}

/// Checks that `task` fails as given and, if it has a `solution/`, passes
/// with it.
pub async fn verify_task(task: &Task) -> Verdict {
    async fn check_with(task: &Task, solution: bool) -> Result<(bool, String)> {
        let work = WorkDir::new(&task.name, 0, false)?;
        copy_tree(&task.repo(), &work.path)?;
        if solution {
            copy_tree(&task.dir.join("solution"), &work.path)?;
        }
        let check_dir = task.dir.join("check");
        if check_dir.is_dir() {
            copy_tree(&check_dir, &work.path)?;
        }
        run_check(task, &work.path).await
    }
    match check_with(task, false).await {
        Err(e) => return Verdict::Broken(format!("{e:#}")),
        Ok((true, _)) => {
            return Verdict::Broken("the check already passes before any change".into())
        }
        Ok((false, _)) => {}
    }
    if !task.dir.join("solution").is_dir() {
        return Verdict::FailsAsGiven;
    }
    match check_with(task, true).await {
        Err(e) => Verdict::Broken(format!("{e:#}")),
        Ok((true, _)) => Verdict::Sound,
        Ok((false, output)) => {
            Verdict::Broken(format!("the check fails even with solution/:\n{output}"))
        }
    }
}

/// Sums and per-task lines for the report.
pub fn summary(records: &[RunRecord]) -> String {
    let mut out = String::new();
    let mut names: Vec<&str> = records.iter().map(|r| r.task.as_str()).collect();
    names.dedup();
    let width = names.iter().map(|n| n.len()).max().unwrap_or(4).max(4);
    let _ = writeln!(
        out,
        "{:width$}  {:>6}  {:>5}  {:>9}  {:>10}  {:>8}  {:>9}  {:>6}",
        "task", "solved", "turns", "input", "cache read", "output", "cost", "time"
    );
    for name in &names {
        let runs: Vec<&RunRecord> = records.iter().filter(|r| r.task == *name).collect();
        let solved = runs.iter().filter(|r| r.solved).count();
        let mean_u = |f: &dyn Fn(&RunRecord) -> Option<u64>| -> String {
            let v: Vec<u64> = runs.iter().filter_map(|r| f(r)).collect();
            if v.is_empty() {
                "?".into()
            } else {
                thousands(v.iter().sum::<u64>() / v.len() as u64)
            }
        };
        let costs: Vec<f64> = runs.iter().filter_map(|r| r.report.cost_usd).collect();
        let cost = if costs.is_empty() {
            "?".into()
        } else {
            format!("${:.4}", costs.iter().sum::<f64>() / costs.len() as f64)
        };
        let secs = runs.iter().map(|r| r.agent_seconds).sum::<f64>() / runs.len() as f64;
        let _ = writeln!(
            out,
            "{name:width$}  {:>6}  {:>5}  {:>9}  {:>10}  {:>8}  {cost:>9}  {:>5.0}s",
            format!("{solved}/{}", runs.len()),
            mean_u(&|r| r.report.turns),
            mean_u(&|r| r.report.input_tokens),
            mean_u(&|r| r.report.cache_read_tokens),
            mean_u(&|r| r.report.output_tokens),
            secs,
        );
    }

    let total = records.len();
    let solved = records.iter().filter(|r| r.solved).count();
    let _ = write!(
        out,
        "\nSolved {solved} of {total} runs ({:.0}%).",
        if total == 0 {
            0.0
        } else {
            100.0 * solved as f64 / total as f64
        }
    );
    let priced: Vec<f64> = records.iter().filter_map(|r| r.report.cost_usd).collect();
    if !priced.is_empty() {
        let spent: f64 = priced.iter().sum();
        let _ = write!(out, " Spent ${spent:.4}");
        if priced.len() < total {
            let _ = write!(out, " on the {} runs with a known price", priced.len());
        }
        if solved > 0 && priced.len() == total {
            let _ = write!(out, ", ${:.4} per solved task", spent / solved as f64);
        }
        out.push('.');
    }
    let failed: Vec<&RunRecord> = records.iter().filter(|r| r.error.is_some()).collect();
    for r in failed {
        let _ = write!(
            out,
            "\n{} run {} wasn't graded: {}",
            r.task,
            r.run,
            r.error.as_deref().unwrap_or_default()
        );
    }
    out.push('\n');
    out
}

/// `12345` as `12,345`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `phazeai eval`.
pub async fn run(args: EvalArgs) -> Result<()> {
    let tasks = load_tasks(&args.dir, &args.tasks)?;

    if args.verify {
        let mut broken = 0;
        for task in &tasks {
            match verify_task(task).await {
                Verdict::Sound => println!("ok      {}", task.name),
                Verdict::FailsAsGiven => {
                    println!("ok      {} (no solution/ to try)", task.name)
                }
                Verdict::Broken(why) => {
                    broken += 1;
                    println!("BROKEN  {}: {why}", task.name);
                }
            }
        }
        if broken > 0 {
            bail!("{broken} of {} tasks are broken", tasks.len());
        }
        return Ok(());
    }

    let agent = Agent::from_args(&args)?;
    eprintln!(
        "Running {} task{} × {} with {}",
        tasks.len(),
        if tasks.len() == 1 { "" } else { "s" },
        args.runs,
        agent.label()
    );
    let mut out_file = match &args.out {
        Some(path) => Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("couldn't open {}", path.display()))?,
        ),
        None => None,
    };
    let mut records = Vec::new();
    for task in &tasks {
        for run in 1..=args.runs {
            eprint!("{} run {run}… ", task.name);
            let record = run_task(&agent, task, run, args.keep).await;
            eprintln!(
                "{}",
                if record.error.is_some() {
                    "not graded"
                } else if record.solved {
                    "solved"
                } else if record.agent_timed_out {
                    "not solved (timed out)"
                } else {
                    "not solved"
                }
            );
            if let Some(file) = out_file.as_mut() {
                use std::io::Write;
                writeln!(file, "{}", serde_json::to_string(&record)?)?;
            }
            records.push(record);
        }
    }
    print!("{}", summary(&records));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_result_from_json_and_from_a_stream() {
        let one = r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","num_turns":4,
            "usage":{"input_tokens":120,"cache_read_input_tokens":9000,"cache_creation_input_tokens":800,"output_tokens":300},
            "total_cost_usd":0.0213}"#;
        let r = parse_report(one).unwrap();
        assert_eq!(r.turns, Some(4));
        assert_eq!(r.input_tokens, Some(120));
        assert_eq!(r.cache_read_tokens, Some(9000));
        assert_eq!(r.cache_write_tokens, Some(800));
        assert_eq!(r.output_tokens, Some(300));
        assert_eq!(r.cost_usd, Some(0.0213));
        assert_eq!(r.is_error, Some(false));

        let stream =
            "{\"type\":\"system\",\"subtype\":\"init\"}\n{\"type\":\"text\",\"text\":\"hi\"}\n\
            {\"type\":\"result\",\"is_error\":true,\"num_turns\":1,\"total_cost_usd\":null}\n";
        let r = parse_report(stream).unwrap();
        assert_eq!(r.is_error, Some(true));
        assert_eq!(r.cost_usd, None);
        assert_eq!(r.input_tokens, None);

        assert!(parse_report("Done, all tests pass.").is_none());
    }

    fn record(task: &str, solved: bool, cost: Option<f64>, input: Option<u64>) -> RunRecord {
        RunRecord {
            task: task.into(),
            agent: "phaze".into(),
            run: 1,
            solved,
            agent_exit: Some(0),
            agent_timed_out: false,
            agent_seconds: 10.0,
            report: AgentReport {
                input_tokens: input,
                cost_usd: cost,
                turns: Some(3),
                ..Default::default()
            },
            check_output: None,
            error: None,
        }
    }

    #[test]
    fn the_summary_counts_solved_runs_and_cost_per_solve() {
        let records = [
            record("a", true, Some(0.02), Some(12_000)),
            record("a", false, Some(0.04), Some(14_000)),
            record("b", true, Some(0.03), Some(1_000)),
        ];
        let text = summary(&records);
        assert!(text.contains("1/2"), "{text}");
        assert!(text.contains("13,000"), "{text}");
        assert!(text.contains("Solved 2 of 3 runs (67%)."), "{text}");
        assert!(
            text.contains("Spent $0.0900, $0.0450 per solved task."),
            "{text}"
        );

        let unknown = summary(&[record("a", true, None, None)]);
        assert!(
            unknown.contains("Solved 1 of 1 runs (100%).\n"),
            "{unknown}"
        );
        assert!(!unknown.contains("Spent"), "{unknown}");
    }

    #[test]
    fn agents_from_the_command_line() {
        let args = |agent: &str, provider: Option<&str>| EvalArgs {
            dir: "evals".into(),
            agent: agent.into(),
            provider: provider.map(Into::into),
            model: None,
            tasks: vec![],
            runs: 1,
            out: None,
            keep: false,
            verify: false,
        };
        assert!(matches!(
            Agent::from_args(&args("phaze", Some("grok"))).unwrap(),
            Agent::Phaze { provider: Some(p), .. } if p == "grok"
        ));
        assert_eq!(
            Agent::from_args(&args("claude-code", None)).unwrap(),
            Agent::ClaudeCode { model: None }
        );
        assert!(Agent::from_args(&args("claude-code", Some("grok"))).is_err());
        assert_eq!(
            Agent::from_args(&args("aider --yes -m \"$PHAZE_EVAL_PROMPT\"", None)).unwrap(),
            Agent::Shell("aider --yes -m \"$PHAZE_EVAL_PROMPT\"".into())
        );
    }

    #[test]
    fn big_numbers_get_commas() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
