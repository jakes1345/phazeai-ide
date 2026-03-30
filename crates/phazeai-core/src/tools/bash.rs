use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
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

        // Capture pwd after command so we can track cwd changes (only on success)
        let wrapped_command = format!("{command} && echo \"PWD:$(pwd)\"");

        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c").arg(&wrapped_command).current_dir(&cwd);

        let output =
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output())
                .await
                .map_err(|_| {
                    PhazeError::tool("bash", format!("Command timed out after {timeout_secs}s"))
                })?
                .map_err(|e| PhazeError::tool("bash", format!("Failed to execute: {e}")))?;

        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Parse and strip the PWD line, then update stored cwd
        if let Some(pwd_line_pos) = stdout.rfind("PWD:") {
            let pwd_start = pwd_line_pos + 4; // skip "PWD:"
            let pwd_end = stdout[pwd_start..]
                .find('\n')
                .map(|i| pwd_start + i)
                .unwrap_or(stdout.len());
            let new_pwd = stdout[pwd_start..pwd_end].trim().to_string();
            // Strip the PWD line (and the preceding newline if present)
            let strip_from = if pwd_line_pos > 0 && stdout.as_bytes().get(pwd_line_pos - 1) == Some(&b'\n') {
                pwd_line_pos - 1
            } else {
                pwd_line_pos
            };
            let strip_to = if pwd_end < stdout.len() { pwd_end + 1 } else { pwd_end };
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
            "exit_code": output.status.code(),
            "success": output.status.success(),
        }))
    }
}
