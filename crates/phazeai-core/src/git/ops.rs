use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

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
        let output = Command::new("git")
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
