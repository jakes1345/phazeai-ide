//! The review bot: read a change the way a careful reviewer would and point
//! at the lines that are wrong.
//!
//! [`collect`] takes a git diff (a branch against its base, the uncommitted
//! changes, or the staged ones) and keeps what a reviewer needs: each file's
//! changed hunks with their new line numbers, minus lock files, generated
//! files, binaries and anything the privacy policy denies. [`review`] sends
//! those to the model in a few compact requests that ask only for real
//! problems as JSON, then checks every finding against the diff, so a
//! comment always lands on a line the change actually shows. Findings can be
//! printed, emitted as JSON, or posted to a GitHub pull request as one review
//! with inline comments ([`github_payload`], [`post_github_review`]).
//!
//! Token use is kept low on purpose: three lines of context instead of whole
//! files, no system prompt beyond the instructions, removed lines shown
//! without numbers, and nothing but the diff in the request.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::cost::Meter;
use crate::llm::{LlmClient, Message, ProviderId, StreamEvent, Usage};
use crate::process::std_command;

/// About how many characters of diff go in one request (roughly 6k tokens).
pub const BATCH_CHARS: usize = 24_000;
/// Diff lines kept per file; a longer file diff is cut with a note.
const MAX_FILE_LINES: usize = 600;
/// Requests sent at the same time.
const PARALLEL_REQUESTS: usize = 3;

/// Which change to review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSource {
    /// The commits on this branch that `base` does not have (`base...HEAD`),
    /// which is what a pull request shows.
    Branch { base: String },
    /// Everything not committed yet, staged or not (`git diff HEAD`).
    Uncommitted,
    /// Only what is staged (`git diff --cached`).
    Staged,
}

impl DiffSource {
    fn git_args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
            "-U3",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        match self {
            DiffSource::Branch { base } => args.push(format!("{base}...HEAD")),
            DiffSource::Uncommitted => args.push("HEAD".into()),
            DiffSource::Staged => args.push("--cached".into()),
        }
        args
    }

    pub fn describe(&self) -> String {
        match self {
            DiffSource::Branch { base } => format!("the commits not in {base}"),
            DiffSource::Uncommitted => "the uncommitted changes".into(),
            DiffSource::Staged => "the staged changes".into(),
        }
    }
}

/// One file of the change, as the reviewer sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedFile {
    /// The path after the change, relative to the repository root.
    pub path: String,
    /// New-side line numbers shown in the diff (added and context lines):
    /// the lines a comment may point at.
    pub lines: BTreeSet<u32>,
    /// New-side line numbers that were added or changed.
    pub added: BTreeSet<u32>,
    /// The hunks as the model reads them: added lines as `12+ code`, context
    /// as `12  code`, removed lines as `-   code`.
    pub text: String,
    /// Why the file is not reviewed, when it is not.
    pub skipped: Option<String>,
}

/// Parse `git diff` output into files.
pub fn parse_unified(diff: &str) -> Vec<ChangedFile> {
    let mut files: Vec<ChangedFile> = Vec::new();
    let mut new_line: u32 = 0;
    let mut shown: usize = 0;
    let mut in_hunk = false;
    let mut minus_path: Option<String> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            in_hunk = false;
            shown = 0;
            minus_path = None;
            // `a/x b/x`: the b side, used until a `+++` line names it.
            let path = rest
                .rsplit_once(" b/")
                .map(|(_, b)| b.to_string())
                .unwrap_or_default();
            files.push(ChangedFile {
                path,
                ..Default::default()
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if !in_hunk {
            if let Some(p) = line.strip_prefix("--- ") {
                minus_path = p.strip_prefix("a/").map(str::to_string);
            } else if let Some(p) = line.strip_prefix("+++ ") {
                match p.strip_prefix("b/") {
                    Some(b) => file.path = b.to_string(),
                    None => {
                        if let Some(m) = minus_path.take() {
                            file.path = m;
                        }
                        file.skipped = Some("deleted".into());
                    }
                }
            } else if line.starts_with("Binary files ") {
                file.skipped = Some("binary".into());
            }
        }
        if let Some(header) = line.strip_prefix("@@ ") {
            in_hunk = true;
            // `@@ -a,b +c,d @@ context`
            new_line = header
                .split_whitespace()
                .find_map(|t| t.strip_prefix('+'))
                .and_then(|t| t.split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            if shown < MAX_FILE_LINES {
                file.text.push_str(line);
                file.text.push('\n');
            }
            continue;
        }
        if !in_hunk {
            continue;
        }
        let keep = shown < MAX_FILE_LINES;
        if let Some(code) = line.strip_prefix('+') {
            file.lines.insert(new_line);
            file.added.insert(new_line);
            if keep {
                file.text.push_str(&format!("{new_line}+ {code}\n"));
            }
            new_line += 1;
        } else if let Some(code) = line.strip_prefix('-') {
            if keep {
                file.text.push_str(&format!("-   {code}\n"));
            }
        } else if let Some(code) = line.strip_prefix(' ') {
            file.lines.insert(new_line);
            if keep {
                file.text.push_str(&format!("{new_line}  {code}\n"));
            }
            new_line += 1;
        } else if line.is_empty() {
            // An empty context line whose leading space was trimmed.
            file.lines.insert(new_line);
            if keep {
                file.text.push_str(&format!("{new_line}\n"));
            }
            new_line += 1;
        } else {
            // `\ No newline at end of file`
            continue;
        }
        shown += 1;
        if shown == MAX_FILE_LINES {
            file.text
                .push_str("(the rest of this file's diff is left out to keep the request small)\n");
        }
    }
    for f in &mut files {
        if f.skipped.is_none() && f.added.is_empty() && f.text.is_empty() {
            f.skipped = Some(if f.lines.is_empty() {
                "no text change".into()
            } else {
                "only deletions".into()
            });
        } else if f.skipped.is_none() && f.added.is_empty() {
            f.skipped = Some("only deletions".into());
        }
    }
    files
}

/// Files a reviewer does not read: lock files and generated or minified code.
pub fn generated(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    const LOCKS: &[&str] = &[
        "Cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lockb",
        "go.sum",
        "poetry.lock",
        "Pipfile.lock",
        "composer.lock",
        "Gemfile.lock",
        "flake.lock",
        "uv.lock",
    ];
    LOCKS.contains(&name)
        || name.ends_with(".min.js")
        || name.ends_with(".min.css")
        || name.ends_with(".map")
        || name.ends_with(".snap")
        || name.ends_with(".pb.go")
        || name.ends_with("_pb2.py")
        || path.starts_with("vendor/")
        || path.contains("/vendor/")
        || path.starts_with("node_modules/")
        || path.contains("/node_modules/")
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("{0} is not inside a git repository")]
    NotARepo(String),
    #[error("git diff failed: {0}")]
    Git(String),
    #[error("could not run git: {0}")]
    Io(#[from] std::io::Error),
}

/// The files of `source` in the repository at `repo`, with lock files,
/// generated files and files the privacy policy denies marked as skipped.
pub fn collect(repo: &Path, source: &DiffSource) -> Result<Vec<ChangedFile>, ReviewError> {
    let top = std_command("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(repo)
        .output()?;
    if !top.status.success() {
        return Err(ReviewError::NotARepo(repo.display().to_string()));
    }
    let root = std::path::PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
    let out = std_command("git")
        .args(source.git_args())
        .current_dir(&root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    if !out.status.success() {
        return Err(ReviewError::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let mut files = parse_unified(&String::from_utf8_lossy(&out.stdout));
    let policy = crate::privacy::active();
    for f in &mut files {
        if f.skipped.is_some() {
            continue;
        }
        if generated(&f.path) {
            f.skipped = Some("generated or lock file".into());
        } else if policy.is_denied(&root.join(&f.path)) {
            f.skipped = Some("private (privacy policy)".into());
        }
    }
    Ok(files)
}

/// The base a branch review compares against when none is given: the
/// remote's default branch, else `main`, else `master`.
pub fn default_base(repo: &Path) -> Option<String> {
    let git = |args: &[&str]| {
        std_command("git")
            .args(args)
            .current_dir(repo)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    if let Some(head) = git(&["rev-parse", "--abbrev-ref", "origin/HEAD"]) {
        if !head.is_empty() && head != "origin/HEAD" {
            return Some(head);
        }
    }
    ["origin/main", "main", "origin/master", "master"]
        .into_iter()
        .find(|b| git(&["rev-parse", "--verify", "-q", b]).is_some())
        .map(str::to_string)
}

/// How bad a finding is.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    #[default]
    Medium,
    High,
}

impl Severity {
    pub fn parse(s: &str) -> Option<Severity> {
        match s.trim().to_ascii_lowercase().as_str() {
            "high" | "critical" | "blocker" | "error" => Some(Severity::High),
            "medium" | "major" | "warning" => Some(Severity::Medium),
            "low" | "minor" | "info" | "nit" => Some(Severity::Low),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        }
    }
}

/// One problem the reviewer found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub file: String,
    pub line: u32,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// A whole review.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Review {
    pub findings: Vec<Finding>,
    pub files_reviewed: Vec<String>,
    /// (path, why it was not reviewed).
    pub skipped: Vec<(String, String)>,
    /// Requests that failed; their files were not reviewed.
    pub errors: Vec<String>,
    pub requests: usize,
    #[serde(skip)]
    pub meter: Meter,
}

impl Review {
    /// Whether any finding is at or above `level`.
    pub fn has_at_least(&self, level: Severity) -> bool {
        self.findings.iter().any(|f| f.severity >= level)
    }
}

/// Group the reviewable files into requests of about `budget` characters.
/// A file is never split; one larger than the budget gets a request alone.
pub fn batches(files: &[ChangedFile], budget: usize) -> Vec<Vec<&ChangedFile>> {
    let mut out: Vec<Vec<&ChangedFile>> = Vec::new();
    let mut size = 0;
    for f in files.iter().filter(|f| f.skipped.is_none()) {
        let len = f.text.len() + f.path.len() + 8;
        if out.is_empty() || size + len > budget {
            out.push(Vec::new());
            size = 0;
        }
        out.last_mut().expect("pushed above").push(f);
        size += len;
    }
    out
}

/// The request for one batch.
pub fn prompt(files: &[&ChangedFile], focus: Option<&str>) -> String {
    let mut p = String::from(
        "Review this code change like a careful senior engineer. Report only real problems \
         you would block a merge for or clearly flag: bugs, crashes, wrong logic, off-by-one \
         errors, security holes, data loss, races, leaks, broken error handling and clear \
         performance traps. Skip style, naming, formatting, missing docs and missing tests \
         unless they hide a bug. Leave out anything you are not sure of.\n\
         Lines are shown as `N+ code` (added), `N  code` (unchanged) and `-   code` (removed). \
         Point each finding at an added or unchanged line number N of its file.\n",
    );
    if let Some(focus) = focus.map(str::trim).filter(|f| !f.is_empty()) {
        p.push_str(&format!("Pay particular attention to: {focus}\n"));
    }
    p.push_str(
        "Reply with JSON only, no prose: {\"findings\":[{\"file\":\"path\",\"line\":N,\
         \"severity\":\"high|medium|low\",\"title\":\"a few words\",\"detail\":\"what is wrong \
         and what happens because of it\",\"fix\":\"the change to make (optional)\"}]}\n\
         If nothing is worth flagging, reply {\"findings\":[]}.\n",
    );
    for f in files {
        p.push_str(&format!("\n### {}\n{}", f.path, f.text));
    }
    p
}

#[derive(Deserialize)]
struct RawReply {
    #[serde(default)]
    findings: Vec<RawFinding>,
}

#[derive(Deserialize)]
struct RawFinding {
    #[serde(default)]
    file: String,
    #[serde(default)]
    line: serde_json::Value,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    fix: Option<String>,
}

/// The findings in a model reply, checked against the diff. A finding on a
/// file the batch did not hold is dropped; one on a line the diff does not
/// show moves to the nearest shown line within 3 lines, or is dropped.
/// `Err` when the reply holds no JSON object at all.
pub fn parse_reply(reply: &str, files: &[&ChangedFile]) -> Result<Vec<Finding>, String> {
    let start = reply.find('{');
    let end = reply.rfind('}');
    let (Some(start), Some(end)) = (start, end) else {
        return Err("the model's reply held no JSON".into());
    };
    if end < start {
        return Err("the model's reply held no JSON".into());
    }
    let raw: RawReply = serde_json::from_str(&reply[start..=end])
        .map_err(|e| format!("the model's reply was not the JSON asked for: {e}"))?;
    let by_path: BTreeMap<&str, &ChangedFile> =
        files.iter().map(|f| (f.path.as_str(), *f)).collect();
    let mut out = Vec::new();
    for r in raw.findings {
        let path = r.file.trim().trim_start_matches("./");
        let path = path
            .strip_prefix("b/")
            .filter(|p| by_path.contains_key(p))
            .unwrap_or(path);
        let Some(file) = by_path.get(path) else {
            continue;
        };
        let line = match &r.line {
            serde_json::Value::Number(n) => n.as_u64(),
            serde_json::Value::String(s) => s.trim().parse().ok(),
            _ => None,
        };
        let Some(line) = line.and_then(|l| u32::try_from(l).ok()) else {
            continue;
        };
        let line = if file.lines.contains(&line) {
            line
        } else {
            match file
                .lines
                .iter()
                .copied()
                .filter(|l| l.abs_diff(line) <= 3)
                .min_by_key(|l| l.abs_diff(line))
            {
                Some(l) => l,
                None => continue,
            }
        };
        let title = r.title.trim().to_string();
        let detail = r.detail.trim().to_string();
        if title.is_empty() && detail.is_empty() {
            continue;
        }
        out.push(Finding {
            file: file.path.clone(),
            line,
            severity: Severity::parse(&r.severity).unwrap_or_default(),
            title: if title.is_empty() {
                detail.chars().take(60).collect()
            } else {
                title
            },
            detail,
            fix: r
                .fix
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty()),
        });
    }
    Ok(out)
}

/// One text-only request: the text streamed back and the usage reported.
async fn ask(client: &dyn LlmClient, prompt: &str) -> (String, Usage, Option<String>) {
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut stream = match client.chat_stream(&[Message::user(prompt)], &[]).await {
        Ok(s) => s,
        Err(e) => return (text, usage, Some(e.to_string())),
    };
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::TextDelta(d) => text.push_str(&d),
            StreamEvent::Usage(u) => {
                usage.input_tokens += u.input_tokens;
                usage.output_tokens += u.output_tokens;
                usage.cache_read_tokens += u.cache_read_tokens;
                usage.cache_write_tokens += u.cache_write_tokens;
            }
            StreamEvent::Done => break,
            StreamEvent::Error(e) => return (text, usage, Some(e)),
            // No tools are offered, so a tool call is dropped unrun.
            _ => {}
        }
    }
    (text, usage, None)
}

/// Review `files` with `client`. `billed` names the provider and model for
/// the cost meter. Requests run a few at a time; a failed request is
/// reported in [`Review::errors`] and the rest still count.
pub async fn review(
    client: &dyn LlmClient,
    billed: Option<(&ProviderId, &str)>,
    files: &[ChangedFile],
    focus: Option<&str>,
) -> Review {
    let mut result = Review::default();
    for f in files {
        match &f.skipped {
            Some(why) => result.skipped.push((f.path.clone(), why.clone())),
            None => result.files_reviewed.push(f.path.clone()),
        }
    }
    let groups = batches(files, BATCH_CHARS);
    result.requests = groups.len();
    let prompts: Vec<(String, Vec<&ChangedFile>)> =
        groups.into_iter().map(|g| (prompt(&g, focus), g)).collect();
    let replies: Vec<_> = futures::stream::iter(prompts.iter().map(|(p, g)| {
        let attached: Vec<std::path::PathBuf> = g
            .iter()
            .map(|f| std::path::PathBuf::from(&f.path))
            .collect();
        async move {
            let reply = crate::privacy::audit::scope(attached, ask(client, p)).await;
            (reply, g)
        }
    }))
    .buffered(PARALLEL_REQUESTS)
    .collect()
    .await;

    let mut seen = HashSet::new();
    for ((text, usage, error), group) in replies {
        result.meter.add(billed, &usage);
        if let Some(e) = error {
            let names: Vec<&str> = group.iter().map(|f| f.path.as_str()).collect();
            result
                .errors
                .push(format!("{e} (not reviewed: {})", names.join(", ")));
            continue;
        }
        match parse_reply(&text, group) {
            Ok(found) => {
                for f in found {
                    let key = (f.file.clone(), f.line, f.title.to_lowercase());
                    if seen.insert(key) {
                        result.findings.push(f);
                    }
                }
            }
            Err(e) => {
                let names: Vec<&str> = group.iter().map(|f| f.path.as_str()).collect();
                result
                    .errors
                    .push(format!("{e} (not reviewed: {})", names.join(", ")));
            }
        }
    }
    result.findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    result
}

/// The review as plain text for a terminal.
pub fn render_text(review: &Review) -> String {
    let mut out = String::new();
    if review.findings.is_empty() {
        out.push_str("No problems found");
        if review.files_reviewed.is_empty() {
            out.push_str(" (nothing to review)");
        }
        out.push_str(".\n");
    }
    for f in &review.findings {
        out.push_str(&format!(
            "{}:{} [{}] {}\n",
            f.file,
            f.line,
            f.severity.as_str(),
            f.title
        ));
        if !f.detail.is_empty() {
            out.push_str(&format!("  {}\n", f.detail));
        }
        if let Some(fix) = &f.fix {
            out.push_str(&format!("  Fix: {fix}\n"));
        }
    }
    out.push_str(&format!(
        "\nReviewed {} file(s) in {} request(s): {}.\n",
        review.files_reviewed.len(),
        review.requests,
        review.meter.summary()
    ));
    if !review.skipped.is_empty() {
        let list: Vec<String> = review
            .skipped
            .iter()
            .map(|(p, why)| format!("{p} ({why})"))
            .collect();
        out.push_str(&format!("Skipped: {}\n", list.join(", ")));
    }
    for e in &review.errors {
        out.push_str(&format!("Error: {e}\n"));
    }
    out
}

/// A GitHub "create a review" request body: one review with an inline
/// comment per finding, on the new side of the diff at `commit`.
pub fn github_payload(review: &Review, commit: &str, model: &str) -> serde_json::Value {
    let comments: Vec<serde_json::Value> = review
        .findings
        .iter()
        .map(|f| {
            let mut body = format!("**[{}] {}**", f.severity.as_str(), f.title);
            if !f.detail.is_empty() {
                body.push_str(&format!("\n\n{}", f.detail));
            }
            if let Some(fix) = &f.fix {
                body.push_str(&format!("\n\nSuggested fix: {fix}"));
            }
            serde_json::json!({"path": f.file, "line": f.line, "side": "RIGHT", "body": body})
        })
        .collect();
    let count = |s: Severity| review.findings.iter().filter(|f| f.severity == s).count();
    let summary = if review.findings.is_empty() {
        "No problems found.".to_string()
    } else {
        format!(
            "{} finding(s): {} high, {} medium, {} low.",
            review.findings.len(),
            count(Severity::High),
            count(Severity::Medium),
            count(Severity::Low)
        )
    };
    let mut body = format!(
        "**PhazeAI review** ({model}): {summary}\n\nReviewed {} file(s); {}.",
        review.files_reviewed.len(),
        review.meter.summary()
    );
    if !review.errors.is_empty() {
        body.push_str(&format!(
            "\n\n{} request(s) failed, so some files were not reviewed.",
            review.errors.len()
        ));
    }
    serde_json::json!({
        "commit_id": commit,
        "event": "COMMENT",
        "body": body,
        "comments": comments,
    })
}

/// `owner/name` from a GitHub remote URL (https or ssh).
pub fn github_repo_from_remote(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !name.is_empty())
        .then(|| format!("{owner}/{name}"))
}

/// Post `payload` as a review on pull request `pr` of `repo` (`owner/name`)
/// at `api` (e.g. `https://api.github.com`), with `token`. Returns the
/// review's web address when GitHub gives one.
pub async fn post_github_review(
    api: &str,
    token: &str,
    repo: &str,
    pr: u64,
    payload: &serde_json::Value,
) -> Result<Option<String>, String> {
    let url = format!(
        "{}/repos/{repo}/pulls/{pr}/reviews",
        api.trim_end_matches('/')
    );
    // A proxy can't reach a server on this machine (a GitHub Enterprise
    // tunnel, a test stand-in), so loopback addresses go direct.
    let loopback = reqwest::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .is_some_and(|h| {
            h == "localhost"
                || h.trim_matches(|c| c == '[' || c == ']')
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60));
    if loopback {
        builder = builder.no_proxy();
    }
    let client = builder
        .build()
        .map_err(|e| format!("couldn't set up the GitHub request: {e}"))?;
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "phazeai-review")
        .json(payload)
        .send()
        .await
        .map_err(|e| format!("couldn't reach GitHub: {e}"))?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("no message");
        return Err(format!("GitHub refused the review ({status}): {msg}"));
    }
    Ok(body
        .get("html_url")
        .and_then(|u| u.as_str())
        .map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,4 +10,5 @@ fn main() {
     let a = 1;
-    let b = 2;
+    let b = 3;
+    let c = a / 0;

     println!(\"{a}\");
diff --git a/Cargo.lock b/Cargo.lock
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1,1 +1,1 @@
-old
+new
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
--- a/gone.rs
+++ /dev/null
@@ -1,1 +0,0 @@
-bye
diff --git a/logo.png b/logo.png
Binary files a/logo.png and b/logo.png differ
";

    fn files() -> Vec<ChangedFile> {
        let mut f = parse_unified(DIFF);
        for x in &mut f {
            if x.skipped.is_none() && generated(&x.path) {
                x.skipped = Some("generated or lock file".into());
            }
        }
        f
    }

    #[test]
    fn a_diff_becomes_numbered_hunks_per_file() {
        let f = files();
        let names: Vec<&str> = f.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(names, ["src/lib.rs", "Cargo.lock", "gone.rs", "logo.png"]);
        let lib = &f[0];
        assert_eq!(lib.skipped, None);
        assert_eq!(lib.added.iter().copied().collect::<Vec<_>>(), [11, 12]);
        assert_eq!(
            lib.lines.iter().copied().collect::<Vec<_>>(),
            [10, 11, 12, 13, 14]
        );
        assert!(lib.text.contains("11+     let b = 3;\n"), "{}", lib.text);
        assert!(lib.text.contains("-       let b = 2;\n"), "{}", lib.text);
        assert!(lib.text.contains("10      let a = 1;\n"), "{}", lib.text);
        assert_eq!(f[1].skipped.as_deref(), Some("generated or lock file"));
        assert_eq!(f[2].skipped.as_deref(), Some("deleted"));
        assert_eq!(f[3].skipped.as_deref(), Some("binary"));
    }

    #[test]
    fn findings_are_checked_against_the_diff() {
        let f = files();
        let lib = vec![&f[0]];
        let reply = r#"Here you go:
```json
{"findings":[
 {"file":"src/lib.rs","line":12,"severity":"high","title":"Division by zero","detail":"a / 0 panics","fix":"divide by a non-zero value"},
 {"file":"b/src/lib.rs","line":"16","severity":"weird","title":"Near","detail":"moves to 14"},
 {"file":"src/lib.rs","line":40,"severity":"low","title":"Far away","detail":"dropped"},
 {"file":"other.rs","line":1,"severity":"high","title":"Not in batch","detail":"dropped"},
 {"file":"src/lib.rs","line":11,"title":"","detail":""}
]}
```"#;
        let found = parse_reply(reply, &lib).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].line, 12);
        assert_eq!(found[0].severity, Severity::High);
        assert_eq!(found[0].fix.as_deref(), Some("divide by a non-zero value"));
        assert_eq!(found[1].line, 14);
        assert_eq!(found[1].severity, Severity::Medium);
        assert!(parse_reply("no json here", &lib).is_err());
        assert!(parse_reply("{\"findings\": nope}", &lib).is_err());
        assert!(parse_reply("{\"findings\":[]}", &lib).unwrap().is_empty());
    }

    #[test]
    fn requests_hold_whole_files_within_a_budget() {
        let mk = |p: &str, n: usize| ChangedFile {
            path: p.into(),
            text: "x".repeat(n),
            ..Default::default()
        };
        let f = vec![mk("a", 50), mk("b", 50), mk("c", 500), mk("d", 10)];
        let groups = batches(&f, 120);
        let names: Vec<Vec<&str>> = groups
            .iter()
            .map(|g| g.iter().map(|x| x.path.as_str()).collect())
            .collect();
        assert_eq!(names, vec![vec!["a", "b"], vec!["c"], vec!["d"]]);
        let p = prompt(&groups[0], Some("SQL injection"));
        assert!(p.contains("### a\n") && p.contains("### b\n"));
        assert!(p.contains("Pay particular attention to: SQL injection"));
    }

    #[test]
    fn long_file_diffs_are_cut() {
        let mut d = String::from(
            "diff --git a/big.rs b/big.rs\n--- a/big.rs\n+++ b/big.rs\n@@ -0,0 +1,700 @@\n",
        );
        for i in 0..700 {
            d.push_str(&format!("+line {i}\n"));
        }
        let f = parse_unified(&d);
        assert_eq!(f[0].added.len(), 700);
        assert_eq!(
            f[0].text.lines().filter(|l| l.contains("+ line")).count(),
            MAX_FILE_LINES
        );
        assert!(f[0].text.contains("left out"));
    }

    #[test]
    fn severities_and_remotes_parse() {
        assert_eq!(Severity::parse("Critical"), Some(Severity::High));
        assert_eq!(Severity::parse("nit"), Some(Severity::Low));
        assert_eq!(Severity::parse("?"), None);
        assert!(Severity::High > Severity::Medium && Severity::Medium > Severity::Low);
        assert_eq!(
            github_repo_from_remote("git@github.com:jake/ide.git").as_deref(),
            Some("jake/ide")
        );
        assert_eq!(
            github_repo_from_remote("https://github.com/jake/ide").as_deref(),
            Some("jake/ide")
        );
        assert_eq!(github_repo_from_remote("https://gitlab.com/jake/ide"), None);
    }

    #[test]
    fn a_github_review_has_one_inline_comment_per_finding() {
        let review = Review {
            findings: vec![Finding {
                file: "src/lib.rs".into(),
                line: 12,
                severity: Severity::High,
                title: "Division by zero".into(),
                detail: "a / 0 panics".into(),
                fix: None,
            }],
            files_reviewed: vec!["src/lib.rs".into()],
            requests: 1,
            ..Default::default()
        };
        let p = github_payload(&review, "abc123", "test-model");
        assert_eq!(p["commit_id"], "abc123");
        assert_eq!(p["event"], "COMMENT");
        assert_eq!(p["comments"][0]["path"], "src/lib.rs");
        assert_eq!(p["comments"][0]["line"], 12);
        assert_eq!(p["comments"][0]["side"], "RIGHT");
        assert!(p["body"].as_str().unwrap().contains("1 high"));
        assert!(review.has_at_least(Severity::Medium));
        assert!(!Review::default().has_at_least(Severity::Low));
    }
}
