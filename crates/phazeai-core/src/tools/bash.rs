use crate::error::PhazeError;
use crate::tools::sandbox;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::PathBuf;
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
        "Execute a bash command. Returns stdout, stderr, and exit code. The working directory persists between calls."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Optional timeout in seconds (default: 120)",
                    "default": 120
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

        Ok(serde_json::json!({
            "stdout": crate::text::cut_middle(&stdout, MAX_OUTPUT_BYTES),
            "stderr": crate::text::cut_middle(&stderr, MAX_OUTPUT_BYTES),
            "exit_code": status.code(),
            "success": status.success(),
        }))
    }
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
