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
                *self.cwd.lock().await = PathBuf::from(new_pwd);
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
