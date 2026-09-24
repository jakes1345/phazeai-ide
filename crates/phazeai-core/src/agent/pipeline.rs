//! Multi-agent pipeline: Planner → Coder → Verify (check/fix loop) → Reviewer.
//!
//! Each stage is a real [`Agent`] run, not a text-only prompt:
//!
//! * **Planner** explores the repo with read-only tools and writes a plan.
//! * **Coder** implements the plan with the full tool set, subject to the
//!   host's approval callback and diff hook — it edits files on disk.
//! * **Verify** runs the project's check command (cargo check, tsc, go build,
//!   ...) against those real edits and feeds failures back to the same Coder
//!   conversation, up to `max_fix_rounds` times.
//! * **Reviewer** reads the actual diff produced by this run (read-only tools)
//!   and returns a verdict. If it requests changes, the Coder gets one round to
//!   address them, followed by a final check.
//!
//! Each role can use a different model through the existing `[model_routes]`
//! settings: Planner = `reasoning`, Coder = `code_generation`, Reviewer =
//! `code_review` (see [`crate::Settings::build_llm_client_for`]).

use crate::agent::{Agent, AgentEvent, AgentResponse, ApprovalFn, DiffHookFn};
use crate::error::PhazeError;
use crate::llm::LlmClient;
use crate::tools::ToolRegistry;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// Longest a single check command may run.
const CHECK_TIMEOUT: Duration = Duration::from_secs(300);
/// Cap on check output / diff text handed to a model.
const MAX_CONTEXT_CHARS: usize = 12_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Plan,
    Code,
    Verify,
    Review,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Plan => "Planner",
            Stage::Code => "Coder",
            Stage::Verify => "Verify",
            Stage::Review => "Reviewer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approved,
    ChangesRequested,
    /// The reviewer's answer didn't contain a recognisable verdict.
    Unclear,
}

/// Progress events for a pipeline run.
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    StageStarted(Stage),
    /// An event from the agent running the given stage (text, tool calls, ...).
    Agent {
        stage: Stage,
        event: AgentEvent,
    },
    CheckResult {
        round: usize,
        command: String,
        passed: bool,
        output: String,
    },
    StageFinished {
        stage: Stage,
        summary: String,
    },
    Complete(PipelineOutcome),
}

#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    pub plan: String,
    pub review: String,
    pub verdict: ReviewVerdict,
    /// `None` when no check command applies to this project.
    pub check_passed: Option<bool>,
    pub fix_rounds: usize,
    /// Files changed by this run (git workspaces only).
    pub changed_files: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A command that verifies the project builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl CheckCommand {
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Pick a check command from the files present in `root`.
    pub fn detect(root: &Path) -> Option<Self> {
        let cmd = |p: &str, a: &[&str]| {
            Some(Self {
                program: p.to_string(),
                args: a.iter().map(|s| s.to_string()).collect(),
            })
        };
        if root.join("Cargo.toml").exists() {
            cmd(
                "cargo",
                &["check", "--all-targets", "--message-format=short"],
            )
        } else if root.join("tsconfig.json").exists() {
            cmd(
                "npx",
                &["--no-install", "tsc", "--noEmit", "--pretty", "false"],
            )
        } else if root.join("go.mod").exists() {
            cmd("go", &["build", "./..."])
        } else if root.join("pyproject.toml").exists()
            || root.join("setup.py").exists()
            || root.join("requirements.txt").exists()
        {
            cmd("python3", &["-m", "compileall", "-q", "."])
        } else {
            None
        }
    }
}

pub struct PipelineConfig {
    pub workspace_root: PathBuf,
    /// `None` = auto-detect with [`CheckCommand::detect`].
    pub check_command: Option<CheckCommand>,
    /// Skip the verify stage entirely.
    pub skip_check: bool,
    /// Build-fix rounds after the first implementation.
    pub max_fix_rounds: usize,
    /// Rounds in which the Coder addresses reviewer feedback.
    pub max_review_rounds: usize,
    /// Tool-call iterations allowed per agent run.
    pub max_agent_iterations: usize,
}

impl PipelineConfig {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            check_command: None,
            skip_check: false,
            max_fix_rounds: 3,
            max_review_rounds: 1,
            max_agent_iterations: 25,
        }
    }
}

pub struct Pipeline {
    planner: Box<dyn LlmClient>,
    coder: Box<dyn LlmClient>,
    reviewer: Box<dyn LlmClient>,
    config: PipelineConfig,
    approval_fn: Option<ApprovalFn>,
    diff_hook: Option<DiffHookFn>,
    cancel: Arc<AtomicBool>,
    coder_tools: Option<ToolRegistry>,
}

impl Pipeline {
    pub fn new(
        planner: Box<dyn LlmClient>,
        coder: Box<dyn LlmClient>,
        reviewer: Box<dyn LlmClient>,
        config: PipelineConfig,
    ) -> Self {
        Self {
            planner,
            coder,
            reviewer,
            config,
            approval_fn: None,
            diff_hook: None,
            cancel: Arc::new(AtomicBool::new(false)),
            coder_tools: None,
        }
    }

    /// Approval callback for the Coder's tool calls (the read-only Planner and
    /// Reviewer never need approval).
    pub fn with_approval(mut self, f: ApprovalFn) -> Self {
        self.approval_fn = Some(f);
        self
    }

    pub fn with_diff_hook(mut self, f: DiffHookFn) -> Self {
        self.diff_hook = Some(f);
        self
    }

    pub fn with_cancel_token(mut self, token: Arc<AtomicBool>) -> Self {
        self.cancel = token;
        self
    }

    /// Override the Coder's tools (defaults to [`ToolRegistry::default`]).
    pub fn with_coder_tools(mut self, tools: ToolRegistry) -> Self {
        self.coder_tools = Some(tools);
        self
    }

    pub async fn run(
        self,
        request: &str,
        tx: UnboundedSender<PipelineEvent>,
    ) -> Result<PipelineOutcome, PhazeError> {
        let Pipeline {
            planner,
            coder,
            reviewer,
            config,
            approval_fn,
            diff_hook,
            cancel,
            coder_tools,
        } = self;
        let this = PipelineCtx {
            cancel,
            tx: tx.clone(),
            root: config.workspace_root.clone(),
        };
        let mut tokens = (0u64, 0u64);
        let baseline = GitBaseline::capture(&config.workspace_root).await;

        // ── 1. Plan ────────────────────────────────────────────────────
        this.check_cancelled()?;
        let planner = Agent::new(planner)
            .with_tools(ToolRegistry::read_only())
            .with_system_prompt(PLANNER_PROMPT)
            .with_max_iterations(config.max_agent_iterations)
            .with_cancel_token(this.cancel.clone());
        let plan = this
            .run_stage(Stage::Plan, &planner, request.to_string(), &mut tokens)
            .await?
            .content;
        this.finish(Stage::Plan, first_line(&plan));

        // ── 2. Code ────────────────────────────────────────────────────
        this.check_cancelled()?;
        let mut coder = Agent::new(coder)
            .with_tools(coder_tools.unwrap_or_default())
            .with_system_prompt(CODER_PROMPT)
            .with_max_iterations(config.max_agent_iterations)
            .with_cancel_token(this.cancel.clone());
        if let Some(f) = approval_fn {
            coder = coder.with_approval(f);
        }
        if let Some(h) = diff_hook {
            coder = coder.with_diff_hook(h);
        }
        let impl_prompt = format!(
            "## Task\n{request}\n\n## Plan (from the Planner)\n{plan}\n\n\
             Implement the plan now by editing files with your tools."
        );
        let summary = this
            .run_stage(Stage::Code, &coder, impl_prompt, &mut tokens)
            .await?
            .content;
        this.finish(Stage::Code, first_line(&summary));

        // ── 3. Verify (check → fix loop) ───────────────────────────────
        let check = if config.skip_check {
            None
        } else {
            config
                .check_command
                .clone()
                .or_else(|| CheckCommand::detect(&config.workspace_root))
        };
        let mut fix_rounds = 0;
        let mut check_passed = match &check {
            Some(cmd) => Some(
                this.verify_and_fix(
                    cmd,
                    &coder,
                    config.max_fix_rounds,
                    &mut fix_rounds,
                    &mut tokens,
                )
                .await?,
            ),
            None => None,
        };

        // ── 4. Review (+ optional fix round) ───────────────────────────
        let reviewer = Agent::new(reviewer)
            .with_tools(ToolRegistry::read_only())
            .with_system_prompt(REVIEWER_PROMPT)
            .with_max_iterations(config.max_agent_iterations)
            .with_cancel_token(this.cancel.clone());
        let mut review_rounds = 0;
        let (review, verdict, changed_files) = loop {
            this.check_cancelled()?;
            let changes = baseline.changes(&config.workspace_root).await;
            let review_prompt = format!(
                "## Task\n{request}\n\n## Plan\n{plan}\n\n## Check\n{}\n\n## Changes made in this run\n{}",
                match (&check, check_passed) {
                    (Some(cmd), Some(true)) => format!("`{}` passes.", cmd.display()),
                    (Some(cmd), _) => format!("`{}` still FAILS.", cmd.display()),
                    (None, _) => "No check command for this project.".to_string(),
                },
                changes.describe()
            );
            let review = this
                .run_stage(Stage::Review, &reviewer, review_prompt, &mut tokens)
                .await?
                .content;
            let verdict = parse_verdict(&review);
            this.finish(Stage::Review, first_line(&review));

            if verdict != ReviewVerdict::ChangesRequested
                || review_rounds >= config.max_review_rounds
            {
                break (review, verdict, changes.files);
            }
            review_rounds += 1;
            this.check_cancelled()?;
            let fix = format!(
                "The Reviewer requested changes:\n\n{review}\n\n\
                 Address every point that is a real problem, then stop."
            );
            this.run_stage(Stage::Code, &coder, fix, &mut tokens)
                .await?;
            if let Some(cmd) = &check {
                check_passed = Some(
                    this.verify_and_fix(cmd, &coder, 1, &mut fix_rounds, &mut tokens)
                        .await?,
                );
            }
        };

        let outcome = PipelineOutcome {
            plan,
            review,
            verdict,
            check_passed,
            fix_rounds,
            changed_files,
            input_tokens: tokens.0,
            output_tokens: tokens.1,
        };
        let _ = tx.send(PipelineEvent::Complete(outcome.clone()));
        Ok(outcome)
    }
}

/// Shared state for stage execution (split out so `run` can move the
/// per-role clients out of `Pipeline`).
struct PipelineCtx {
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<PipelineEvent>,
    root: PathBuf,
}

impl PipelineCtx {
    fn check_cancelled(&self) -> Result<(), PhazeError> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(PhazeError::Other("Pipeline cancelled".into()))
        } else {
            Ok(())
        }
    }

    fn finish(&self, stage: Stage, summary: String) {
        let _ = self
            .tx
            .send(PipelineEvent::StageFinished { stage, summary });
    }

    /// Run one agent turn, forwarding its events tagged with `stage`.
    async fn run_stage(
        &self,
        stage: Stage,
        agent: &Agent,
        prompt: String,
        tokens: &mut (u64, u64),
    ) -> Result<AgentResponse, PhazeError> {
        let _ = self.tx.send(PipelineEvent::StageStarted(stage));
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel();
        let outer = self.tx.clone();
        let forward = async move {
            while let Some(event) = inner_rx.recv().await {
                let _ = outer.send(PipelineEvent::Agent { stage, event });
            }
        };
        let (result, ()) = tokio::join!(agent.run_with_events(prompt, inner_tx), forward);
        let response = result?;
        tokens.0 += response.total_input_tokens;
        tokens.1 += response.total_output_tokens;
        Ok(response)
    }

    /// Run the check; on failure give the Coder up to `max_rounds` attempts to
    /// fix it. Returns whether the final check passed.
    async fn verify_and_fix(
        &self,
        cmd: &CheckCommand,
        coder: &Agent,
        max_rounds: usize,
        fix_rounds: &mut usize,
        tokens: &mut (u64, u64),
    ) -> Result<bool, PhazeError> {
        let mut round = 0;
        loop {
            self.check_cancelled()?;
            let _ = self.tx.send(PipelineEvent::StageStarted(Stage::Verify));
            let (passed, output) = run_check(cmd, &self.root).await;
            let _ = self.tx.send(PipelineEvent::CheckResult {
                round,
                command: cmd.display(),
                passed,
                output: output.clone(),
            });
            self.finish(
                Stage::Verify,
                if passed {
                    format!("`{}` passed", cmd.display())
                } else {
                    format!("`{}` failed", cmd.display())
                },
            );
            if passed || round >= max_rounds {
                return Ok(passed);
            }
            round += 1;
            *fix_rounds += 1;
            let fix = format!(
                "`{}` fails after your changes (fix attempt {round}/{max_rounds}):\n\n```\n{output}\n```\n\n\
                 Fix the errors caused by the change. Do not disable checks or delete tests to make it pass.",
                cmd.display()
            );
            self.run_stage(Stage::Code, coder, fix, tokens).await?;
        }
    }
}

async fn run_check(cmd: &CheckCommand, root: &Path) -> (bool, String) {
    let run = tokio::process::Command::new(&cmd.program)
        .args(&cmd.args)
        .current_dir(root)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(CHECK_TIMEOUT, run).await {
        Ok(Ok(out)) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            (out.status.success(), tail_chars(&text, MAX_CONTEXT_CHARS))
        }
        Ok(Err(e)) => (false, format!("could not run `{}`: {e}", cmd.display())),
        Err(_) => (
            false,
            format!(
                "`{}` timed out after {}s",
                cmd.display(),
                CHECK_TIMEOUT.as_secs()
            ),
        ),
    }
}

// ── Git-based change tracking ─────────────────────────────────────────────

/// Working-tree snapshot taken before the Coder runs, so the Reviewer sees
/// only this run's changes — not edits the user already had uncommitted.
struct GitBaseline {
    /// Commit-ish capturing the pre-run tree (`git stash create`, or HEAD).
    base: Option<String>,
    untracked_before: Vec<String>,
}

impl GitBaseline {
    async fn capture(root: &Path) -> Self {
        let base = match git(root, &["stash", "create"]).await {
            Some(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => git(root, &["rev-parse", "--verify", "HEAD"])
                .await
                .map(|s| s.trim().to_string()),
        };
        Self {
            base,
            untracked_before: untracked(root).await,
        }
    }

    async fn changes(&self, root: &Path) -> Changes {
        let Some(base) = &self.base else {
            return Changes::default();
        };
        let diff = git(root, &["diff", base]).await.unwrap_or_default();
        let mut files: Vec<String> = git(root, &["diff", "--name-only", base])
            .await
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        let new_files: Vec<String> = untracked(root)
            .await
            .into_iter()
            .filter(|f| !self.untracked_before.contains(f))
            .collect();
        files.extend(new_files.iter().cloned());
        Changes {
            tracked: true,
            diff,
            files,
            new_files,
        }
    }
}

#[derive(Default)]
struct Changes {
    tracked: bool,
    diff: String,
    files: Vec<String>,
    new_files: Vec<String>,
}

impl Changes {
    fn describe(&self) -> String {
        if !self.tracked {
            return "Not a git repository, so no diff is available. Use your tools to \
                    read the files the plan says were changed."
                .to_string();
        }
        if self.files.is_empty() {
            return "No files were changed.".to_string();
        }
        let mut out = format!("Files: {}\n", self.files.join(", "));
        if !self.new_files.is_empty() {
            out.push_str(&format!(
                "New (untracked) files — read them with your tools: {}\n",
                self.new_files.join(", ")
            ));
        }
        out.push_str(&format!(
            "\n```diff\n{}\n```",
            head_chars(&self.diff, MAX_CONTEXT_CHARS)
        ));
        out
    }
}

async fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn untracked(root: &Path) -> Vec<String> {
    git(root, &["ls-files", "--others", "--exclude-standard"])
        .await
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn parse_verdict(review: &str) -> ReviewVerdict {
    let upper = review.to_uppercase();
    if upper.contains("CHANGES REQUESTED") || upper.contains("CHANGES_REQUESTED") {
        ReviewVerdict::ChangesRequested
    } else if upper.contains("APPROVED") {
        ReviewVerdict::Approved
    } else {
        ReviewVerdict::Unclear
    }
}

fn first_line(s: &str) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    head_chars(line, 160)
}

fn head_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}\n… [truncated]", &s[..s.floor_char_boundary(max)])
    }
}

/// Keep the end of long output — compiler errors summarise at the bottom.
fn tail_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut start = s.len() - max;
        while !s.is_char_boundary(start) {
            start += 1;
        }
        format!("[… earlier output truncated]\n{}", &s[start..])
    }
}

// ── Role prompts ──────────────────────────────────────────────────────────

const PLANNER_PROMPT: &str = "You are the PLANNER in PhazeAI's multi-agent pipeline.
Explore the repository with your read-only tools (read_file, grep, glob, list_files, find_path) until you understand what the task touches. Then write a plan for the Coder:

1. A one-sentence summary of the change.
2. Numbered steps, each naming the exact files and functions to create or modify.
3. Risks and edge cases the Coder must handle.
4. How to know it works (tests to add or run).

Do not write the implementation. Be concrete and brief.";

const CODER_PROMPT: &str = "You are the CODER in PhazeAI's multi-agent pipeline.
Implement the Planner's plan by editing files with your tools (edit_file, write_file, ...). Read files before editing them and match the surrounding style. Make every change the plan calls for; do not leave TODOs.
When you are asked to fix check failures or review feedback, fix the underlying problem - never silence warnings, skip tests, or delete code just to make a check pass.
When done, reply with a short summary of what you changed.";

const REVIEWER_PROMPT: &str = "You are the REVIEWER in PhazeAI's multi-agent pipeline.
You receive the task, the plan, the check result and the diff of this run's changes. Use your read-only tools to read surrounding code when the diff is not enough.
Look for: incorrect behaviour, missed requirements, bugs, security problems, and changes that break other callers. Ignore pure style preferences.
Start your reply with exactly one of these lines:
APPROVED
CHANGES REQUESTED
Then list each problem with file, line and a concrete fix (none if approved).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_parsing() {
        assert_eq!(
            parse_verdict("APPROVED\nlooks good"),
            ReviewVerdict::Approved
        );
        assert_eq!(
            parse_verdict("CHANGES REQUESTED\n- src/a.rs:3 off by one"),
            ReviewVerdict::ChangesRequested
        );
        assert_eq!(parse_verdict("hmm"), ReviewVerdict::Unclear);
    }

    #[test]
    fn detects_check_commands() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(CheckCommand::detect(dir.path()), None);
        std::fs::write(dir.path().join("go.mod"), "module x").unwrap();
        assert_eq!(CheckCommand::detect(dir.path()).unwrap().program, "go");
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(
            CheckCommand::detect(dir.path()).unwrap().display(),
            "cargo check --all-targets --message-format=short"
        );
    }

    #[test]
    fn truncation_is_char_safe() {
        let s = "é".repeat(10);
        assert!(head_chars(&s, 5).starts_with("éé"));
        assert!(tail_chars(&s, 5).ends_with("éé"));
    }

    #[tokio::test]
    async fn baseline_sees_only_this_runs_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sh = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap()
        };
        sh(&["init", "-q"]);
        sh(&["config", "user.email", "t@t"]);
        sh(&["config", "user.name", "t"]);
        sh(&["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join("b.txt"), "one\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-qm", "init"]);
        // The user's own uncommitted edit, made before the pipeline runs.
        std::fs::write(root.join("a.txt"), "user edit\n").unwrap();

        let baseline = GitBaseline::capture(root).await;
        // The pipeline's edits.
        std::fs::write(root.join("b.txt"), "pipeline edit\n").unwrap();
        std::fs::write(root.join("new.txt"), "created\n").unwrap();

        let changes = baseline.changes(root).await;
        assert!(changes.files.contains(&"b.txt".to_string()));
        assert!(changes.files.contains(&"new.txt".to_string()));
        assert!(
            !changes.files.contains(&"a.txt".to_string()),
            "{:?}",
            changes.files
        );
        // `git stash create` must not have touched the working tree.
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "user edit\n"
        );
    }
}
