//! Agent worktrees: a separate checkout of the repository per task, so several
//! agents can edit at once without touching each other or the user's own
//! working tree.
//!
//! A worktree is a `git worktree` on a branch `phaze/<name>`, started from the
//! commit the user's checkout is on. It lives outside the repository, under
//! the local data directory (`<data>/phazeai/worktrees/<repo>-<hash>/<name>`),
//! so it never shows up in the user's `git status` or in code search.
//!
//! The lifecycle is: [`create`] (or [`open_or_create`]), let an agent work in
//! [`Worktree::path`], [`commit_all`] what it changed, then [`merge`] the
//! branch into the user's checkout or [`remove`] it. Nothing here pushes,
//! rebases or touches the user's checkout except [`merge`], which stops and
//! puts everything back if the merge does not apply cleanly.

use crate::process::std_command;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Output;

/// Branches of agent worktrees start with this.
pub const BRANCH_PREFIX: &str = "phaze/";
/// Longest worktree name kept.
const MAX_NAME: usize = 48;

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("{0} is not inside a git repository")]
    NotARepo(PathBuf),
    #[error("the repository has no commit yet; make one before starting an agent worktree")]
    NoCommit,
    #[error("no agent worktree named {0:?}")]
    NotFound(String),
    #[error("an agent worktree named {0:?} already exists")]
    Exists(String),
    #[error("{0}")]
    Dirty(String),
    #[error("merging {branch} did not apply cleanly ({files}); nothing was changed")]
    Conflict { branch: String, files: String },
    #[error("git {args} failed: {stderr}")]
    Git { args: String, stderr: String },
    #[error("could not run git: {0}")]
    Io(#[from] std::io::Error),
}

/// One agent worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// The name the user gave or that was made up, e.g. `fix-login`.
    pub name: String,
    /// Its checkout.
    pub path: PathBuf,
    /// Its branch, `phaze/<name>`.
    pub branch: String,
}

/// What a worktree holds that the user's checkout does not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeState {
    /// Files changed and not committed yet, as `git status --porcelain` lines.
    pub uncommitted: Vec<String>,
    /// Commits on the worktree's branch that the user's branch does not have.
    pub commits_ahead: usize,
    /// Files that differ from the user's branch in those commits.
    pub changed_files: Vec<String>,
}

impl WorktreeState {
    pub fn is_empty(&self) -> bool {
        self.uncommitted.is_empty() && self.commits_ahead == 0
    }
}

fn git_in(dir: &Path, args: &[&str]) -> Result<Output, WorktreeError> {
    Ok(std_command("git")
        .args(args)
        .current_dir(dir)
        // Never wait for an editor or a password prompt.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .output()?)
}

fn git_ok(dir: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let out = git_in(dir, args)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(WorktreeError::Git {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
    }
}

/// The top of the repository `dir` is in. Inside an agent worktree this is
/// the main checkout, so worktrees are always made from and merged into it.
pub fn repo_root(dir: &Path) -> Result<PathBuf, WorktreeError> {
    // The common git dir is shared by every worktree; its parent is the main
    // checkout (for a non-bare repository).
    let common = git_ok(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map_err(|_| WorktreeError::NotARepo(dir.to_path_buf()))?;
    let common = PathBuf::from(common.trim());
    let root = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        None
    };
    match root {
        Some(r) => Ok(r),
        None => git_ok(dir, &["rev-parse", "--show-toplevel"])
            .map(|s| PathBuf::from(s.trim()))
            .map_err(|_| WorktreeError::NotARepo(dir.to_path_buf())),
    }
}

/// Where the worktrees of `repo` live.
pub fn worktrees_dir(repo: &Path) -> PathBuf {
    let base = std::env::var_os("PHAZEAI_WORKTREES_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("phazeai").join("worktrees")))
        .unwrap_or_else(|| std::env::temp_dir().join("phazeai-worktrees"));
    let canonical = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let hash = Sha256::digest(canonical.to_string_lossy().as_bytes());
    let hash: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
    let stem: String = slug(
        &canonical
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".into()),
    );
    base.join(format!("{stem}-{hash}"))
}

/// A name safe for a directory and a branch: lowercase letters, digits and
/// single dashes, at most [`MAX_NAME`] characters. Empty when nothing usable
/// is left.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= MAX_NAME {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// A short name from a task's first words, e.g. "Fix the login bug" →
/// `fix-the-login-bug`, made unique among `taken`.
pub fn name_for_task(task: &str, taken: &[String]) -> String {
    let words: Vec<&str> = task.split_whitespace().take(5).collect();
    let mut base = slug(&words.join(" "));
    if base.is_empty() {
        base = "task".into();
    }
    base.truncate(32);
    let base = base.trim_matches('-').to_string();
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c))
        .unwrap_or(base)
}

/// The agent worktrees of `repo`, by name.
pub fn list(repo: &Path) -> Result<Vec<Worktree>, WorktreeError> {
    let repo = repo_root(repo)?;
    let text = git_ok(&repo, &["worktree", "list", "--porcelain"])?;
    let mut out = Vec::new();
    let mut path: Option<PathBuf> = None;
    for line in text.lines().chain(std::iter::once("")) {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            if let (Some(p), Some(name)) = (&path, b.strip_prefix(BRANCH_PREFIX)) {
                out.push(Worktree {
                    name: name.to_string(),
                    path: p.clone(),
                    branch: b.to_string(),
                });
            }
        } else if line.is_empty() {
            path = None;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The agent worktree called `name`.
pub fn find(repo: &Path, name: &str) -> Result<Worktree, WorktreeError> {
    let want = slug(name);
    list(repo)?
        .into_iter()
        .find(|w| w.name == want)
        .ok_or_else(|| WorktreeError::NotFound(name.to_string()))
}

/// Make a worktree called `name` on a new branch `phaze/<name>`, starting
/// from the commit the repository's main checkout is on.
pub fn create(repo: &Path, name: &str) -> Result<Worktree, WorktreeError> {
    let repo = repo_root(repo)?;
    let name = slug(name);
    if name.is_empty() {
        return Err(WorktreeError::NotFound(String::new()));
    }
    if git_ok(&repo, &["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
        return Err(WorktreeError::NoCommit);
    }
    let branch = format!("{BRANCH_PREFIX}{name}");
    let exists = git_in(
        &repo,
        &[
            "show-ref",
            "--verify",
            "-q",
            &format!("refs/heads/{branch}"),
        ],
    )?
    .status
    .success();
    if exists || list(&repo)?.iter().any(|w| w.name == name) {
        return Err(WorktreeError::Exists(name));
    }
    let path = worktrees_dir(&repo).join(&name);
    if path.exists() {
        return Err(WorktreeError::Exists(name));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let path_s = path.to_string_lossy().into_owned();
    git_ok(
        &repo,
        &["worktree", "add", "-q", "-b", &branch, &path_s, "HEAD"],
    )?;
    Ok(Worktree { name, path, branch })
}

/// The worktree called `name`, made first if it does not exist.
pub fn open_or_create(repo: &Path, name: &str) -> Result<Worktree, WorktreeError> {
    match find(repo, name) {
        Ok(w) => Ok(w),
        Err(WorktreeError::NotFound(_)) => create(repo, name),
        Err(e) => Err(e),
    }
}

/// The branch the main checkout is on (`HEAD` when detached).
fn current_branch(repo: &Path) -> Result<String, WorktreeError> {
    Ok(git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string())
}

/// What `wt` holds beyond the main checkout's branch.
pub fn state(repo: &Path, wt: &Worktree) -> Result<WorktreeState, WorktreeError> {
    let repo = repo_root(repo)?;
    let uncommitted: Vec<String> = git_ok(&wt.path, &["status", "--porcelain"])?
        .lines()
        .map(str::to_string)
        .collect();
    let range = format!("HEAD..{}", wt.branch);
    let commits_ahead = git_ok(&repo, &["rev-list", "--count", &range])?
        .trim()
        .parse()
        .unwrap_or(0);
    let three_dots = format!("HEAD...{}", wt.branch);
    let changed_files = git_ok(&repo, &["diff", "--name-only", &three_dots])?
        .lines()
        .map(str::to_string)
        .collect();
    Ok(WorktreeState {
        uncommitted,
        commits_ahead,
        changed_files,
    })
}

/// Commit everything changed in `wt` (new files included) with `message`.
/// `Ok(false)` when there was nothing to commit. The commit is authored by
/// the repository's configured user, or "PhazeAI agent" when none is set.
pub fn commit_all(wt: &Worktree, message: &str) -> Result<bool, WorktreeError> {
    git_ok(&wt.path, &["add", "-A"])?;
    if git_in(&wt.path, &["diff", "--cached", "--quiet"])?
        .status
        .success()
    {
        return Ok(false);
    }
    let has_user = git_in(&wt.path, &["config", "user.email"])?
        .status
        .success();
    let message = if message.trim().is_empty() {
        "Agent changes"
    } else {
        message.trim()
    };
    let mut args: Vec<&str> = Vec::new();
    if !has_user {
        args.extend([
            "-c",
            "user.name=PhazeAI agent",
            "-c",
            "user.email=agent@phazeai.invalid",
        ]);
    }
    args.extend(["commit", "-q", "--no-verify", "-m", message]);
    git_ok(&wt.path, &args)?;
    Ok(true)
}

/// Merge the worktree's branch into the branch the main checkout is on, as a
/// merge commit. Refuses while the main checkout has uncommitted changes to
/// tracked files or the worktree has uncommitted changes, and stops (with the
/// checkout as it was) when the merge would conflict.
pub fn merge(repo: &Path, wt: &Worktree) -> Result<(), WorktreeError> {
    let repo = repo_root(repo)?;
    let mine = git_ok(&repo, &["status", "--porcelain", "--untracked-files=no"])?;
    if !mine.trim().is_empty() {
        return Err(WorktreeError::Dirty(
            "your checkout has uncommitted changes; commit or stash them before merging".into(),
        ));
    }
    let theirs = git_ok(&wt.path, &["status", "--porcelain"])?;
    if !theirs.trim().is_empty() {
        return Err(WorktreeError::Dirty(format!(
            "the worktree {} has uncommitted changes; commit them first",
            wt.name
        )));
    }
    let into = current_branch(&repo)?;
    let message = format!("Merge agent worktree {} into {into}", wt.name);
    let has_user = git_in(&repo, &["config", "user.email"])?.status.success();
    let mut args: Vec<&str> = Vec::new();
    if !has_user {
        args.extend([
            "-c",
            "user.name=PhazeAI agent",
            "-c",
            "user.email=agent@phazeai.invalid",
        ]);
    }
    args.extend(["merge", "--no-ff", "--no-edit", "-m", &message, &wt.branch]);
    let out = git_in(&repo, &args)?;
    if out.status.success() {
        return Ok(());
    }
    let conflicted = git_ok(&repo, &["diff", "--name-only", "--diff-filter=U"])
        .unwrap_or_default()
        .lines()
        .collect::<Vec<_>>()
        .join(", ");
    let merging = git_in(&repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])?
        .status
        .success();
    if merging {
        let _ = git_in(&repo, &["merge", "--abort"]);
    }
    if conflicted.is_empty() {
        return Err(WorktreeError::Git {
            args: "merge".into(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Err(WorktreeError::Conflict {
        branch: wt.branch.clone(),
        files: conflicted,
    })
}

/// Delete the worktree's checkout and its branch. Without `force`, refuses
/// while it holds work the main checkout's branch does not have.
pub fn remove(repo: &Path, wt: &Worktree, force: bool) -> Result<(), WorktreeError> {
    let repo = repo_root(repo)?;
    if !force {
        let s = state(&repo, wt)?;
        if !s.is_empty() {
            return Err(WorktreeError::Dirty(format!(
                "the worktree {} has {} uncommitted file(s) and {} commit(s) not merged; \
                 merge it or remove it with force",
                wt.name,
                s.uncommitted.len(),
                s.commits_ahead
            )));
        }
    }
    let path = wt.path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path);
    git_ok(&repo, &args)?;
    git_ok(&repo, &["branch", "-D", &wt.branch])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// `PHAZEAI_WORKTREES_DIR` is process-wide.
    static ENV: Mutex<()> = Mutex::new(());

    struct Repo {
        _dir: tempfile::TempDir,
        _wts: tempfile::TempDir,
        root: PathBuf,
        _guard: MutexGuard<'static, ()>,
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = git_in(dir, args).unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo() -> Repo {
        let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        std::env::set_var("PHAZEAI_WORKTREES_DIR", wts.path());
        let root = dir.path().canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "dev@example.com"]);
        git(&root, &["config", "user.name", "Dev"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "first"]);
        Repo {
            _dir: dir,
            _wts: wts,
            root,
            _guard: guard,
        }
    }

    #[test]
    fn names_are_safe_and_unique() {
        assert_eq!(slug("Fix the Login bug!"), "fix-the-login-bug");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("  "), "");
        assert_eq!(slug(&"x".repeat(100)).len(), MAX_NAME);
        let taken = vec!["fix-login".to_string(), "fix-login-2".to_string()];
        assert_eq!(name_for_task("fix login", &taken), "fix-login-3");
        assert_eq!(name_for_task("!!!", &[]), "task");
    }

    #[test]
    fn a_worktree_is_a_separate_checkout_outside_the_repo() {
        let r = repo();
        let wt = create(&r.root, "Fix login").unwrap();
        assert_eq!(wt.name, "fix-login");
        assert_eq!(wt.branch, "phaze/fix-login");
        assert!(!wt.path.starts_with(&r.root));
        assert_eq!(
            std::fs::read_to_string(wt.path.join("a.txt")).unwrap(),
            "one\ntwo\nthree\n"
        );
        // The user's checkout does not see it.
        let status = git_ok(&r.root, &["status", "--porcelain"]).unwrap();
        assert!(status.is_empty(), "{status}");
        // From inside the worktree, the repository is still the main checkout.
        assert_eq!(repo_root(&wt.path).unwrap(), r.root);
        assert_eq!(list(&wt.path).unwrap(), vec![wt.clone()]);
        assert!(matches!(
            create(&r.root, "fix login"),
            Err(WorktreeError::Exists(_))
        ));
        assert_eq!(open_or_create(&r.root, "fix-login").unwrap(), wt);
    }

    #[test]
    fn work_is_committed_merged_and_cleaned_up() {
        let r = repo();
        let wt = create(&r.root, "edit").unwrap();
        std::fs::write(wt.path.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(wt.path.join("new.txt"), "hello\n").unwrap();
        let s = state(&r.root, &wt).unwrap();
        assert_eq!(s.uncommitted.len(), 2);
        assert!(!s.is_empty());
        // Unmerged work is not thrown away by accident.
        assert!(matches!(
            remove(&r.root, &wt, false),
            Err(WorktreeError::Dirty(_))
        ));
        assert!(matches!(merge(&r.root, &wt), Err(WorktreeError::Dirty(_))));

        assert!(commit_all(&wt, "Capitalise two").unwrap());
        assert!(!commit_all(&wt, "nothing").unwrap());
        let s = state(&r.root, &wt).unwrap();
        assert_eq!(s.commits_ahead, 1);
        assert_eq!(s.changed_files, vec!["a.txt", "new.txt"]);
        // The user's files are untouched until they merge.
        assert_eq!(
            std::fs::read_to_string(r.root.join("a.txt")).unwrap(),
            "one\ntwo\nthree\n"
        );

        merge(&r.root, &wt).unwrap();
        assert_eq!(
            std::fs::read_to_string(r.root.join("a.txt")).unwrap(),
            "one\nTWO\nthree\n"
        );
        assert!(r.root.join("new.txt").exists());
        assert!(state(&r.root, &wt).unwrap().is_empty());
        remove(&r.root, &wt, false).unwrap();
        assert!(!wt.path.exists());
        assert!(list(&r.root).unwrap().is_empty());
        let branches = git_ok(&r.root, &["branch", "--list", "phaze/*"]).unwrap();
        assert!(branches.trim().is_empty());
    }

    #[test]
    fn a_conflicting_merge_changes_nothing() {
        let r = repo();
        let wt = create(&r.root, "clash").unwrap();
        std::fs::write(wt.path.join("a.txt"), "one\nagent\nthree\n").unwrap();
        commit_all(&wt, "agent edit").unwrap();
        std::fs::write(r.root.join("a.txt"), "one\nuser\nthree\n").unwrap();
        git(&r.root, &["commit", "-qam", "user edit"]);

        let err = merge(&r.root, &wt).unwrap_err();
        assert!(
            matches!(err, WorktreeError::Conflict { ref files, .. } if files == "a.txt"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(r.root.join("a.txt")).unwrap(),
            "one\nuser\nthree\n"
        );
        let status = git_ok(&r.root, &["status", "--porcelain"]).unwrap();
        assert!(status.is_empty(), "{status}");
        // A user edit in progress blocks a merge before it starts.
        std::fs::write(r.root.join("a.txt"), "changed\n").unwrap();
        assert!(matches!(merge(&r.root, &wt), Err(WorktreeError::Dirty(_))));
        remove(&r.root, &wt, true).unwrap();
        assert!(list(&r.root).unwrap().is_empty());
    }

    #[test]
    fn parallel_worktrees_do_not_see_each_other() {
        let r = repo();
        let a = create(&r.root, "a").unwrap();
        let b = create(&r.root, "b").unwrap();
        std::fs::write(a.path.join("a.txt"), "from a\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(b.path.join("a.txt")).unwrap(),
            "one\ntwo\nthree\n"
        );
        let names: Vec<String> = list(&r.root).unwrap().into_iter().map(|w| w.name).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn errors_outside_a_repo_and_before_the_first_commit() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            create(dir.path(), "x"),
            Err(WorktreeError::NotARepo(_))
        ));
        git(dir.path(), &["init", "-q"]);
        assert!(matches!(
            create(dir.path(), "x"),
            Err(WorktreeError::NoCommit)
        ));
    }
}
