//! `phazeai review`: the review bot from the terminal or CI.
//!
//! Reviews the commits on this branch (as a pull request would show them),
//! the uncommitted changes or the staged ones, prints the findings or writes
//! them as JSON, can fail a CI job at a chosen severity, and can post them to
//! a GitHub pull request as one review with inline comments.

use anyhow::{bail, Context, Result};
use phazeai_core::code_review::{self, DiffSource, Review, Severity};

use crate::setup;

#[derive(clap::Args, Debug)]
pub struct ReviewArgs {
    /// Review the commits not in BASE (`BASE...HEAD`, what a pull request
    /// shows). Defaults to the remote's default branch, else main or master.
    #[arg(long, value_name = "BASE", conflicts_with_all = ["uncommitted", "staged"])]
    pub base: Option<String>,
    /// Review everything not committed yet, staged or not (new files only
    /// once they are added with `git add`).
    #[arg(long, conflicts_with = "staged")]
    pub uncommitted: bool,
    /// Review only what is staged.
    #[arg(long)]
    pub staged: bool,
    /// Something to look at especially closely, e.g. "security" or "the
    /// cache invalidation".
    #[arg(long)]
    pub focus: Option<String>,
    /// LLM model to review with.
    #[arg(short, long)]
    pub model: Option<String>,
    /// LLM provider to review with.
    #[arg(long)]
    pub provider: Option<String>,
    /// text (default) or json.
    #[arg(long, value_enum, default_value_t = ReviewFormat::Text)]
    pub output_format: ReviewFormat,
    /// Exit with status 1 when there is a finding at this severity or
    /// worse (for CI): high, medium or low.
    #[arg(long, value_enum, value_name = "LEVEL")]
    pub fail_on: Option<Level>,
    /// Post the findings to this GitHub pull request as one review with
    /// inline comments. Needs GITHUB_TOKEN (or GH_TOKEN) with write access
    /// to pull requests; GITHUB_API_URL is honoured for GitHub Enterprise.
    #[arg(long, value_name = "N")]
    pub github_pr: Option<u64>,
    /// The pull request's repository, `owner/name`. Defaults to
    /// GITHUB_REPOSITORY (set in GitHub Actions), else the `origin` remote.
    #[arg(long, value_name = "OWNER/NAME")]
    pub github_repo: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ReviewFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Level {
    High,
    Medium,
    Low,
}

impl From<Level> for Severity {
    fn from(l: Level) -> Severity {
        match l {
            Level::High => Severity::High,
            Level::Medium => Severity::Medium,
            Level::Low => Severity::Low,
        }
    }
}

fn git(dir: &std::path::Path, args: &[&str]) -> Option<String> {
    phazeai_core::process::std_command("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// The review as one JSON object.
pub fn to_json(review: &Review, model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "findings": review.findings,
        "files_reviewed": review.files_reviewed,
        "skipped": review.skipped.iter()
            .map(|(p, why)| serde_json::json!({"path": p, "reason": why}))
            .collect::<Vec<_>>(),
        "errors": review.errors,
        "requests": review.requests,
        "usage": {
            "input_tokens": review.meter.input,
            "output_tokens": review.meter.output,
            "cache_read_input_tokens": review.meter.cache_read,
        },
        "cost_usd": (review.meter.unpriced_calls == 0).then_some(review.meter.cost),
    })
}

pub async fn run(args: ReviewArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let Some(root) = git(&cwd, &["rev-parse", "--show-toplevel"]) else {
        bail!("{} is not inside a git repository", cwd.display());
    };
    let root = std::path::PathBuf::from(root);
    // Files the project's privacy policy denies are never sent.
    phazeai_core::privacy::set_active_project(&root);
    if let Some(err) = phazeai_core::privacy::active().load_error() {
        bail!("the privacy policy is invalid ({err}); nothing was sent");
    }

    let explicit = args.base.is_some() || args.uncommitted || args.staged;
    let source = if args.uncommitted {
        DiffSource::Uncommitted
    } else if args.staged {
        DiffSource::Staged
    } else {
        let base = match args.base.clone() {
            Some(b) => b,
            None => code_review::default_base(&root).context(
                "couldn't find a base branch to compare with; pass --base, --uncommitted or --staged",
            )?,
        };
        DiffSource::Branch { base }
    };
    if args.github_pr.is_some() && !matches!(source, DiffSource::Branch { .. }) {
        bail!("--github-pr reviews commits, so it can't be used with --uncommitted or --staged");
    }
    let mut source = source;
    let mut files = code_review::collect(&root, &source)?;
    // Nothing on the branch yet but work in progress: review that instead.
    if !explicit
        && args.github_pr.is_none()
        && files.iter().all(|f| f.skipped.is_some())
        && git(&root, &["status", "--porcelain", "--untracked-files=no"])
            .is_some_and(|s| !s.is_empty())
    {
        source = DiffSource::Uncommitted;
        files = code_review::collect(&root, &source)?;
    }

    let mut settings = phazeai_core::Settings::load();
    if let Some(ref provider) = args.provider {
        let Some(p) = setup::parse_provider(provider) else {
            bail!(
                "unknown provider '{provider}'. Available: {}",
                setup::PROVIDER_NAMES.join(", ")
            );
        };
        if p != settings.llm.provider && args.model.is_none() {
            settings.llm.model = setup::default_model_for(&settings, &p);
        }
        settings.set_provider(p);
    }
    if let Some(ref model) = args.model {
        settings.llm.model = model.clone();
    }
    let model = settings.llm.model.clone();
    eprintln!("Reviewing {} with {model}…", source.describe());

    let review = if files.iter().any(|f| f.skipped.is_none()) {
        let client = settings.build_llm_client()?;
        let billed = settings.billed_model();
        code_review::review(
            client.as_ref(),
            billed.as_ref().map(|(p, m)| (p, m.as_str())),
            &files,
            args.focus.as_deref(),
        )
        .await
    } else {
        let mut r = Review::default();
        for f in &files {
            if let Some(why) = &f.skipped {
                r.skipped.push((f.path.clone(), why.clone()));
            }
        }
        r
    };

    match args.output_format {
        ReviewFormat::Text => print!("{}", code_review::render_text(&review)),
        ReviewFormat::Json => println!("{}", to_json(&review, &model)),
    }

    if let Some(pr) = args.github_pr {
        let token = std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("GH_TOKEN"))
            .context("posting to GitHub needs GITHUB_TOKEN or GH_TOKEN")?;
        let repo = match args.github_repo.clone() {
            Some(r) => r,
            None => std::env::var("GITHUB_REPOSITORY")
                .ok()
                .filter(|r| !r.is_empty())
                .or_else(|| {
                    git(&root, &["remote", "get-url", "origin"])
                        .and_then(|u| code_review::github_repo_from_remote(&u))
                })
                .context("couldn't tell which GitHub repository; pass --github-repo owner/name")?,
        };
        let api = std::env::var("GITHUB_API_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "https://api.github.com".into());
        let commit = git(&root, &["rev-parse", "HEAD"]).context("couldn't read HEAD")?;
        let payload = code_review::github_payload(&review, &commit, &model);
        match code_review::post_github_review(&api, &token, &repo, pr, &payload).await {
            Ok(Some(url)) => eprintln!("Posted the review: {url}"),
            Ok(None) => eprintln!("Posted the review to {repo}#{pr}."),
            Err(e) => bail!(e),
        }
    }

    if review.requests > 0 && review.errors.len() == review.requests {
        bail!("the review failed: {}", review.errors.join("; "));
    }
    if let Some(level) = args.fail_on {
        if review.has_at_least(level.into()) {
            bail!(
                "found problems at {} severity or worse",
                Severity::from(level).as_str()
            );
        }
    }
    Ok(())
}
