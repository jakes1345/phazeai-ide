use crate::error::PhazeError;
use crate::tools::sandbox;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// How much of each of stdout and stderr is returned: longer output keeps its
/// start and its end (see [`crate::text::cut_middle`]).
const MAX_OUTPUT_BYTES: usize = 30_000;

pub struct BashTool {
    shell: Arc<Mutex<Shell>>,
}

/// Where the shell is, and the workspace root it was in when it got there.
struct Shell {
    cwd: PathBuf,
    root: Option<PathBuf>,
}

impl Shell {
    /// The directory the next command runs in. When the host has opened another
    /// folder since (the workspace root changed), the shell starts over at the
    /// new root: a `cd` made in the old project means nothing in the new one,
    /// and the sandbox would refuse the old directory anyway.
    fn cwd_now(&mut self) -> PathBuf {
        let root = sandbox::workspace_root();
        if root != self.root {
            if let Some(new_root) = &root {
                self.cwd = new_root.clone();
            }
            self.root = root;
        }
        self.cwd.clone()
    }
}

impl BashTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            shell: Arc::new(Mutex::new(Shell {
                cwd,
                root: sandbox::workspace_root(),
            })),
        }
    }
}

impl Default for BashTool {
    /// A shell in the workspace root when one is set, else in the process's
    /// current directory.
    fn default() -> Self {
        Self::new(sandbox::base_dir())
    }
}

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run a bash command; returns stdout, stderr and exit code. The working directory persists between calls."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 120)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("bash", "Missing required parameter: command"))?;

        let timeout_secs = params
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(120);

        let cwd = self.shell.lock().await.cwd_now();
        // If a workspace root is configured, refuse to run commands whose cwd
        // already lies outside it. Defends against the agent doing `cd /etc &&
        // rm -rf foo` over multiple turns. Per-command shell escapes are not
        // detectable here — rely on the approval gate for that.
        if let Some(root) = sandbox::workspace_root() {
            let canonical_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
            if !canonical_cwd.starts_with(&root)
                && !sandbox::is_protected_system_path(&canonical_cwd)
            {
                return Err(PhazeError::tool(
                    "bash",
                    format!(
                        "REFUSED: shell cwd '{}' is outside the workspace root '{}'",
                        canonical_cwd.display(),
                        root.display()
                    ),
                ));
            }
        }

        // Privacy: refuse commands whose words name a denied file. Best-effort —
        // see `privacy::command_denial` for what this can and can't see.
        let policy = crate::privacy::active();
        if let Some((word, denial)) = crate::privacy::command_denial(&policy, command, &cwd) {
            let reason = denial.reason();
            crate::privacy::audit::record_blocked_file(&word, &reason, "bash");
            return Err(PhazeError::tool(
                "bash",
                crate::privacy::refusal(&word, &reason),
            ));
        }

        // Capture pwd after command so we can track cwd changes (only on success)
        let wrapped_command = format!("{command} && echo \"PWD:$(pwd)\"");

        let mut cmd = crate::process::tokio_command("bash");
        cmd.arg("-c").arg(&wrapped_command).current_dir(&cwd);
        // An interrupted run drops this future, which kills the shell and
        // whatever it started (its whole process group); so does the timeout.
        let output = crate::process::output_or_kill(cmd, Duration::from_secs(timeout_secs))
            .await
            .map_err(|e| PhazeError::tool("bash", format!("Failed to execute: {e}")))?
            .ok_or_else(|| {
                PhazeError::tool("bash", format!("Command timed out after {timeout_secs}s"))
            })?;
        let status = output.status;

        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Parse and strip the PWD line, then update stored cwd
        if let Some(pwd_line_pos) = stdout.rfind("PWD:") {
            let pwd_start = pwd_line_pos + 4; // skip "PWD:"
            let pwd_end = stdout[pwd_start..]
                .find('\n')
                .map(|i| pwd_start + i)
                .unwrap_or(stdout.len());
            let new_pwd = stdout[pwd_start..pwd_end].trim().to_string();
            // Strip the PWD line (and the preceding newline if present)
            let strip_from =
                if pwd_line_pos > 0 && stdout.as_bytes().get(pwd_line_pos - 1) == Some(&b'\n') {
                    pwd_line_pos - 1
                } else {
                    pwd_line_pos
                };
            let strip_to = if pwd_end < stdout.len() {
                pwd_end + 1
            } else {
                pwd_end
            };
            stdout = format!("{}{}", &stdout[..strip_from], &stdout[strip_to..]);
            if !new_pwd.is_empty() {
                self.shell.lock().await.cwd = PathBuf::from(new_pwd);
            }
        }

        // Colours, progress redraws and repeated lines are for a live
        // terminal; the model reads the finished text. Cleaned before the
        // cap, so the cap keeps more of what was said.
        let stdout = crate::text::clean_terminal_output(&stdout);
        let stderr = crate::text::clean_terminal_output(&stderr);
        // Paths under the project, written out in full by `find`, test
        // runners and linters, are given relative to it: the shell is there
        // and the file tools read relative paths from there, so they mean
        // the same and the root is not repeated on every line.
        let base = sandbox::base_dir();
        let ended = self.shell.lock().await.cwd.clone();
        let (stdout, stderr) = if same_dir(&cwd, &base) && same_dir(&ended, &base) {
            (
                relative_to(&stdout, &base, &cwd),
                relative_to(&stderr, &base, &cwd),
            )
        } else {
            (stdout.into_owned().into(), stderr.into_owned().into())
        };
        Ok(serde_json::json!({
            "stdout": crate::text::cut_middle(&stdout, MAX_OUTPUT_BYTES),
            "stderr": crate::text::cut_middle(&stderr, MAX_OUTPUT_BYTES),
            "exit_code": status.code(),
            "success": status.success(),
        }))
    }
}

/// Whether `a` and `b` are the same directory.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// `text` with the project folder's own prefix taken off the paths under it:
/// `/home/me/app/src/main.rs:3:` becomes `src/main.rs:3:`. The folder is
/// matched as written (`spelled`, the shell's working directory) and as its
/// canonical path, only where a path starts, and never when it is `/`.
fn relative_to<'a>(text: &'a str, root: &Path, spelled: &Path) -> std::borrow::Cow<'a, str> {
    let mut prefixes: Vec<String> = [
        Some(root.to_path_buf()),
        Some(spelled.to_path_buf()),
        root.canonicalize().ok(),
    ]
    .into_iter()
    .flatten()
    .filter(|p| p.is_absolute() && p.parent().is_some())
    .filter_map(|p| p.to_str().map(|s| format!("{}/", s.trim_end_matches('/'))))
    .collect();
    // The longest first, so a prefix that holds another is taken whole.
    prefixes.sort_by_key(|p| std::cmp::Reverse(p.len()));
    prefixes.dedup();
    if !prefixes.iter().any(|p| text.contains(p.as_str())) {
        return text.into();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while !rest.is_empty() {
        let at_path_start = out
            .chars()
            .last()
            .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '~')));
        if at_path_start {
            for p in &prefixes {
                if let Some(after) = rest.strip_prefix(p.as_str()) {
                    // The folder itself (`<root>/`, then a space or the end)
                    // stays a path: `.`.
                    if after.is_empty() || after.starts_with(char::is_whitespace) {
                        out.push('.');
                    }
                    rest = after;
                    continue 'scan;
                }
            }
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out.into()
}

// Tests live in `tests/bash_cancel.rs` (interrupt, timeout, output capture
// and long output) and `tests/tool_tests.rs`: they need a shell in a temp
// dir, which the lib tests that set the global workspace sandbox would refuse.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::sandbox::{set_workspace_root, tests::SERIAL, workspace_root};

    /// Holds the process-wide workspace root at `root` until dropped.
    struct Root {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<PathBuf>,
    }

    impl Drop for Root {
        fn drop(&mut self) {
            set_workspace_root(self.prev.take());
        }
    }

    fn hold_root(root: &std::path::Path) -> Root {
        let lock = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let prev = workspace_root();
        set_workspace_root(Some(root.to_path_buf()));
        Root { _lock: lock, prev }
    }

    #[test]
    fn paths_under_the_project_are_given_relative_to_it() {
        let root = std::path::Path::new("/work/app");
        let out = "/work/app/src/main.rs:3: fn main\n\
                   ./tests/a.rs\n\
                   see /work/app/README.md and (/work/app/docs/x.md)\n\
                   /work/app-old/src/lib.rs\n\
                   /usr/lib/x.so /tmp/work/app/y\n\
                   cd /work/app/ done\n";
        assert_eq!(
            relative_to(out, root, root),
            "src/main.rs:3: fn main\n\
             ./tests/a.rs\n\
             see README.md and (docs/x.md)\n\
             /work/app-old/src/lib.rs\n\
             /usr/lib/x.so /tmp/work/app/y\n\
             cd . done\n"
        );
        // Nothing to change: the text is passed through as it is.
        assert!(matches!(
            relative_to("no paths here", root, root),
            std::borrow::Cow::Borrowed(_)
        ));
        // The filesystem root is never taken off.
        let slash = std::path::Path::new("/");
        assert_eq!(relative_to("/etc/hosts", slash, slash), "/etc/hosts");
    }

    #[test]
    fn the_shell_starts_in_the_open_folder_and_moves_when_another_is_opened() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_root = first.path().canonicalize().unwrap();
        let second_root = second.path().canonicalize().unwrap();
        std::fs::create_dir_all(first_root.join("sub")).unwrap();

        let held = hold_root(&first_root);
        let bash = BashTool::default();
        let mut shell = bash.shell.try_lock().unwrap();
        assert_eq!(shell.cwd_now(), first_root);

        // A `cd` inside the project is kept while the project stays open.
        shell.cwd = first_root.join("sub");
        assert_eq!(shell.cwd_now(), first_root.join("sub"));

        // Open Folder: the next command runs at the new root, not in `sub`.
        set_workspace_root(Some(second_root.clone()));
        assert_eq!(shell.cwd_now(), second_root);
        drop(shell);
        drop(held);
    }
}
