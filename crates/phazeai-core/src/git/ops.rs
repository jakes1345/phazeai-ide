use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Git operations using the system git binary.
/// Falls back gracefully if git is not installed.
pub struct GitOps {
    repo_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct GitStatus {
    pub branch: String,
    pub files: Vec<FileStatus>,
    pub is_clean: bool,
}

#[derive(Debug, Clone)]
pub struct FileStatus {
    pub path: String,
    pub status: FileState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileState {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl GitOps {
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
        }
    }

    /// Find the git root from a given path by walking up.
    pub fn find_root(start: &Path) -> Option<PathBuf> {
        let mut current = start.to_path_buf();
        loop {
            if current.join(".git").exists() {
                return Some(current);
            }
            if !current.pop() {
                return None;
            }
        }
    }

    async fn run_git(&self, args: &[&str]) -> Result<String, String> {
        // Paths in `diff` / `status` output come back unquoted (non-ASCII as
        // is), so they can be matched against the privacy deny list.
        let output = crate::process::tokio_command("git")
            .args(crate::privacy::GIT_NO_QUOTE_PATH)
            .args(args)
            .current_dir(&self.repo_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("Failed to run git: {e}"))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }

    pub async fn status(&self) -> Result<GitStatus, String> {
        let branch = self
            .run_git(&["branch", "--show-current"])
            .await
            .unwrap_or_else(|_| "unknown".into());

        let porcelain = self.run_git(&["status", "--porcelain"]).await?;

        let files: Vec<FileStatus> = porcelain
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|line| {
                // Git status --porcelain format: XY PATH where X is index status, Y is worktree status
                // However, the exact format can vary, so we need to find where the filename starts
                let parts: Vec<&str> = line.splitn(2, ' ').collect();
                if parts.len() < 2 {
                    return None;
                }

                let status_code = parts[0];
                let path = parts[1].trim().to_string();

                let status = match status_code {
                    "M" | "MM" | " M" | "M " => FileState::Modified,
                    "A" | "AM" | " A" | "A " => FileState::Added,
                    "D" | " D" | "D " => FileState::Deleted,
                    "R" | " R" | "R " => FileState::Renamed,
                    "??" => FileState::Untracked,
                    "UU" | "AA" | "DD" => FileState::Conflicted,
                    _ => FileState::Modified,
                };
                Some(FileStatus { path, status })
            })
            .collect();

        let is_clean = files.is_empty();

        Ok(GitStatus {
            branch,
            files,
            is_clean,
        })
    }

    pub async fn diff(&self, staged: bool) -> Result<String, String> {
        if staged {
            self.run_git(&["diff", "--cached"]).await
        } else {
            self.run_git(&["diff"]).await
        }
    }

    pub async fn add(&self, paths: &[&str]) -> Result<(), String> {
        let mut args = vec!["add"];
        args.extend(paths);
        self.run_git(&args).await?;
        Ok(())
    }

    pub async fn commit(&self, message: &str) -> Result<String, String> {
        self.run_git(&["commit", "-m", message]).await
    }

    pub async fn log(&self, count: usize) -> Result<String, String> {
        let count_str = format!("-{count}");
        self.run_git(&["log", &count_str, "--oneline"]).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git must be installed");
        assert!(out.status.success(), "git {args:?}");
    }

    #[tokio::test]
    async fn status_and_diff_report_non_ascii_paths_unquoted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@t"]);
        git(root, &["config", "user.name", "t"]);
        git(root, &["config", "core.quotePath", "true"]);
        std::fs::write(root.join("café.txt"), "1\n").unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("café.txt"), "2\n").unwrap();

        let ops = GitOps::new(root);
        let status = ops.status().await.unwrap();
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].path, "café.txt");
        let diff = ops.diff(false).await.unwrap();
        assert!(diff.contains("diff --git a/café.txt b/café.txt"), "{diff}");
        assert!(!diff.contains("\\303\\251"), "{diff}");
    }
}
