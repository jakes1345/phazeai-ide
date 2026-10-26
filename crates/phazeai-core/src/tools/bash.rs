use crate::error::PhazeError;
use crate::tools::sandbox;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

const MAX_OUTPUT_CHARS: usize = 30000;

pub struct BashTool {
    cwd: Arc<Mutex<PathBuf>>,
}

impl BashTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd: Arc::new(Mutex::new(cwd)),
        }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

/// Kills the shell and everything it started.
///
/// The shell runs in its own process group (Unix), so one signal to the group
/// reaches its children too: `bash -c "cargo build"` is bash plus cargo plus
/// rustc, and killing only bash would leave the build running. On Windows the
/// direct child is killed through the `Child` handle instead.
fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: plain libc call. A negative pid addresses the process group
        // with that id, which is the shell's own group because it was spawned
        // with `process_group(0)`. Failure (already gone) is harmless.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Kills the shell's process group if the command is still running when the
/// `execute` future is dropped. That is what happens when the user interrupts
/// a run (Esc / Ctrl+C in the CLI, Stop in the IDE): the agent stops awaiting
/// the tool, and without this the command would carry on in the background.
/// Disarmed once the command has exited.
struct KillOnDrop {
    pid: Option<u32>,
}

impl KillOnDrop {
    fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            kill_process_group(pid);
        }
    }
}

/// Kills a child that is still running (timeout) and reaps it.
async fn kill_and_reap(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        kill_process_group(pid);
    }
    // On Windows this is the kill itself; on Unix the group is already dead
    // and this reaps the shell.
    let _ = child.kill().await;
}

async fn read_to_end(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf).await;
    }
    buf
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

        let cwd = self.cwd.lock().await.clone();
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
        cmd.arg("-c")
            .arg(&wrapped_command)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Dropping this future (an interrupted run) kills the shell…
            .kill_on_drop(true);
        // …and a process group of its own lets `KillOnDrop` take whatever the
        // shell started along with it.
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd
            .spawn()
            .map_err(|e| PhazeError::tool("bash", format!("Failed to execute: {e}")))?;
        let mut guard = KillOnDrop { pid: child.id() };
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let finished = {
            let run = async {
                tokio::join!(
                    child.wait(),
                    read_to_end(stdout_pipe),
                    read_to_end(stderr_pipe)
                )
            };
            tokio::select! {
                result = run => Some(result),
                _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => None,
            }
        };
        let Some((status, stdout, stderr)) = finished else {
            kill_and_reap(&mut child).await;
            guard.disarm();
            return Err(PhazeError::tool(
                "bash",
                format!("Command timed out after {timeout_secs}s"),
            ));
        };
        guard.disarm();
        let status =
            status.map_err(|e| PhazeError::tool("bash", format!("Failed to execute: {e}")))?;

        let mut stdout = String::from_utf8_lossy(&stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&stderr).to_string();

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
                *self.cwd.lock().await = PathBuf::from(new_pwd);
            }
        }

        // Truncate if too long
        if stdout.len() > MAX_OUTPUT_CHARS {
            stdout.truncate(MAX_OUTPUT_CHARS);
            stdout.push_str("\n... [output truncated]");
        }
        if stderr.len() > MAX_OUTPUT_CHARS {
            stderr.truncate(MAX_OUTPUT_CHARS);
            stderr.push_str("\n... [output truncated]");
        }

        Ok(serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": status.code(),
            "success": status.success(),
        }))
    }
}

// Tests live in `tests/bash_cancel.rs` (interrupt, timeout and output
// capture) and `tests/tool_tests.rs`: they need a shell in a temp dir, which
// the lib tests that set the global workspace sandbox would refuse.
