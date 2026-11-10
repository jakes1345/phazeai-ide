//! Hooks: the user's own commands, run before and after the agent's tool
//! calls, set in `config.toml`:
//!
//! ```toml
//! [[hooks.pre_tool]]
//! tools = ["bash(git push*)"]
//! command = "./scripts/check-branch.sh"
//!
//! [[hooks.post_tool]]
//! tools = ["edit_file(*.rs)", "write_file(*.rs)"]
//! command = "cargo fmt"
//! timeout_secs = 30
//! ```
//!
//! `tools` takes permission rules ([`crate::permissions`]); left out, the hook
//! runs for every call. `post_tool` hooks run only after a call that
//! succeeded (a formatter has nothing to do after a failed edit). The command runs in the shell (`sh -c`, or `cmd /C` on
//! Windows) in the project root, with the call as JSON on stdin:
//! `{"event", "tool", "params", "project_dir"}`, and after the call also
//! `"success"` and `"result"` (cut to 16 KB). `PHAZE_EVENT`, `PHAZE_TOOL` and
//! `PHAZE_PROJECT_DIR` are set too.
//!
//! Exit 0 is fine. Exit 2 means "no" before a call: it does not run, and the
//! hook's stderr is what the model is told. After a call, exit 2 hands the
//! hook's stderr to the model with the result (a linter's complaints, say).
//! Any other failure (another exit code, a timeout, a command that won't
//! start) does not stop the call; a line about it goes with the result.
//!
//! Hooks come from the user's own config only: a hook in a cloned
//! repository would run its commands on the user's machine.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::permissions::{Call, Flavor, Rule};

/// How long a hook may run when `timeout_secs` is not set.
pub const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(60);
/// The most of a hook's stderr passed on, in bytes.
const MAX_MESSAGE_BYTES: usize = 2 * 1024;
/// The most of a call's result a post-call hook gets, in bytes.
const MAX_RESULT_BYTES: usize = 16 * 1024;

/// The `[hooks]` table of `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HookSettings {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pre_tool: Vec<HookEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub post_tool: Vec<HookEntry>,
}

impl HookSettings {
    pub fn is_empty(&self) -> bool {
        self.pre_tool.is_empty() && self.post_tool.is_empty()
    }
}

/// One `[[hooks.pre_tool]]` or `[[hooks.post_tool]]` entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HookEntry {
    /// The calls it runs for, as permission rules; empty for every call.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// The shell command.
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// A hook ready to run.
#[derive(Debug, Clone)]
pub(crate) struct Hook {
    matchers: Vec<Rule>,
    command: String,
    timeout: Duration,
}

/// How a hook run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HookOutcome {
    /// Exit 0.
    Ok,
    /// Exit 2: what its stderr says.
    Objects(String),
    /// Anything else, described for the user and the model.
    Failed(String),
}

impl Hook {
    pub(crate) fn new(entry: &HookEntry) -> Result<Self, String> {
        let command = entry.command.trim();
        if command.is_empty() {
            return Err("a hook needs a `command`".into());
        }
        let matchers = entry
            .tools
            .iter()
            .map(|t| Rule::parse(t, Flavor::Phaze).map_err(|e| format!("`{t}`: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            matchers,
            command: command.to_string(),
            timeout: entry
                .timeout_secs
                .map_or(DEFAULT_HOOK_TIMEOUT, |s| Duration::from_secs(s.max(1))),
        })
    }

    pub(crate) fn command(&self) -> &str {
        &self.command
    }

    pub(crate) fn applies(&self, tool: &str, params: &Value, root: Option<&Path>) -> bool {
        let call = Call::new(tool, params, root);
        self.matchers.is_empty() || self.matchers.iter().any(|m| m.matches_any(&call))
    }

    /// Runs the hook for one call. `after` is the call's (success, result)
    /// for a post-call hook.
    pub(crate) async fn run(
        &self,
        event: &str,
        tool: &str,
        params: &Value,
        root: Option<&Path>,
        after: Option<(bool, &str)>,
    ) -> HookOutcome {
        let mut input = json!({
            "event": event,
            "tool": tool,
            "params": params,
            "project_dir": root.map(|r| r.display().to_string()),
        });
        if let Some((success, result)) = after {
            input["success"] = json!(success);
            input["result"] = json!(crate::text::truncate_bytes(result, MAX_RESULT_BYTES));
        }

        #[cfg(windows)]
        let mut cmd = {
            let mut c = crate::process::tokio_command("cmd");
            c.arg("/C").arg(&self.command);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = crate::process::tokio_command("sh");
            c.arg("-c").arg(&self.command);
            c
        };
        if let Some(root) = root {
            cmd.current_dir(root).env("PHAZE_PROJECT_DIR", root);
        }
        cmd.env("PHAZE_EVENT", event).env("PHAZE_TOOL", tool);

        let input = serde_json::to_vec(&input).unwrap_or_default();
        match crate::process::output_or_kill_with_input(cmd, Some(input), self.timeout).await {
            Err(e) => HookOutcome::Failed(format!("could not start: {e}")),
            Ok(None) => HookOutcome::Failed(format!(
                "stopped after {} s without finishing",
                self.timeout.as_secs()
            )),
            Ok(Some(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = crate::text::truncate_bytes(stderr.trim(), MAX_MESSAGE_BYTES);
                match output.status.code() {
                    Some(0) => HookOutcome::Ok,
                    Some(2) => HookOutcome::Objects(message.to_string()),
                    code => {
                        let how = code.map_or("was killed".to_string(), |c| format!("exited {c}"));
                        if message.is_empty() {
                            HookOutcome::Failed(how)
                        } else {
                            HookOutcome::Failed(format!("{how}: {message}"))
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn hook(command: &str, tools: &[&str]) -> Hook {
        Hook::new(&HookEntry {
            tools: tools.iter().map(|s| s.to_string()).collect(),
            command: command.into(),
            timeout_secs: Some(5),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn a_hook_reads_the_call_and_answers_by_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let params = json!({"command": "git push"});
        let check = hook(
            "grep -q '\"tool\":\"bash\"' && [ \"$PHAZE_TOOL\" = bash ] && echo 'not on main' >&2; exit 2",
            &[],
        );
        assert_eq!(
            check
                .run("pre_tool", "bash", &params, Some(dir.path()), None)
                .await,
            HookOutcome::Objects("not on main".into())
        );
        let ok = hook("cat > /dev/null; exit 0", &[]);
        assert_eq!(
            ok.run("pre_tool", "bash", &params, Some(dir.path()), None)
                .await,
            HookOutcome::Ok
        );
        let broken = hook("echo oops >&2; exit 7", &[]);
        assert_eq!(
            broken
                .run("pre_tool", "bash", &params, Some(dir.path()), None)
                .await,
            HookOutcome::Failed("exited 7: oops".into())
        );
        let slow = Hook::new(&HookEntry {
            tools: vec![],
            command: "sleep 5".into(),
            timeout_secs: Some(1),
        })
        .unwrap();
        assert!(matches!(
            slow.run("pre_tool", "bash", &params, Some(dir.path()), None).await,
            HookOutcome::Failed(m) if m.contains("stopped after 1 s")
        ));
    }

    #[tokio::test]
    async fn a_post_call_hook_gets_the_result_and_runs_in_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let h = hook("cat > seen.json; pwd > where.txt", &["edit_file(*.rs)"]);
        let params = json!({"path": "src/a.rs"});
        assert!(h.applies("edit_file", &params, Some(dir.path())));
        assert!(!h.applies("edit_file", &json!({"path": "a.md"}), Some(dir.path())));
        assert!(!h.applies("bash", &json!({"command": "ls"}), Some(dir.path())));
        assert_eq!(
            h.run(
                "post_tool",
                "edit_file",
                &params,
                Some(dir.path()),
                Some((true, "edited"))
            )
            .await,
            HookOutcome::Ok
        );
        let seen: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("seen.json")).unwrap())
                .unwrap();
        assert_eq!(seen["event"], "post_tool");
        assert_eq!(seen["params"]["path"], "src/a.rs");
        assert_eq!(seen["success"], true);
        assert_eq!(seen["result"], "edited");
        let where_ = std::fs::read_to_string(dir.path().join("where.txt")).unwrap();
        assert_eq!(
            Path::new(where_.trim()).canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn a_hook_needs_a_command_and_good_rules() {
        assert!(Hook::new(&HookEntry::default()).is_err());
        assert!(Hook::new(&HookEntry {
            tools: vec!["bash(".into()],
            command: "true".into(),
            timeout_secs: None,
        })
        .is_err());
    }
}
