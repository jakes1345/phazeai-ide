//! Tool permissions for single-prompt (`-p`) mode.
//!
//! There's no TUI to show an approval prompt, so `-p` runs with a fixed
//! permission level chosen by `--allow`. Read-only tools always run. Anything
//! above the chosen level is asked about on the terminal when there is one,
//! and refused otherwise (piped input, CI), with a note saying which `--allow`
//! level would permit it.
//!
//! `--allow edit` covers the file tools only. A shell command that writes
//! (`mkdir`, `cp`, `git push`, `pip install`, `>`) is still a command running
//! an arbitrary program, so it needs `--allow all` like any other command.

use std::io::{BufRead, IsTerminal, Write};
use std::sync::{Arc, Mutex};

use phazeai_core::tools::{ToolApprovalManager, ToolPermission};
use serde_json::Value;

use crate::tool_view::{approval_preview, PreviewLine};

/// How much `-p` mode may do without asking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum Allow {
    /// Read files and run read-only commands (ls, cat, grep, git status, ...).
    #[default]
    Read,
    /// Also create, edit, copy and move files with the file tools. Shell
    /// commands stay limited to read-only ones.
    Edit,
    /// Also run any shell command.
    All,
}

impl Allow {
    /// Whether a call to `tool`, classified as `permission`, may run unasked.
    pub fn permits(self, tool: &str, permission: ToolPermission) -> bool {
        self >= Allow::needed_for(tool, permission)
    }

    /// The smallest level that lets `tool` run at `permission` unasked.
    ///
    /// A shell command is either read-only or needs `all`: `bash` classified
    /// as Write (`mkdir`, `cp`, `git push`, `pip install`, `>` redirection)
    /// still runs an arbitrary program, which `edit` must not cover.
    pub fn needed_for(tool: &str, permission: ToolPermission) -> Allow {
        match permission {
            ToolPermission::ReadOnly => Allow::Read,
            ToolPermission::Write if tool != "bash" => Allow::Edit,
            ToolPermission::Write | ToolPermission::Execute | ToolPermission::Destructive => {
                Allow::All
            }
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            Allow::Read => "--allow read",
            Allow::Edit => "--allow edit",
            Allow::All => "--allow all",
        }
    }
}

/// Tool calls refused because nobody could be asked.
pub type Refusals = Arc<Mutex<Vec<(String, Allow)>>>;

/// One line summarising a tool call, e.g. `bash: cargo build` or
/// `edit_file: src/main.rs`. The model wrote the text, so control characters
/// and escape sequences are stripped before it goes anywhere near a terminal.
pub fn describe_call(name: &str, params: &Value) -> String {
    let detail = match name {
        "bash" => params.get("command").and_then(Value::as_str),
        _ => params
            .get("path")
            .or_else(|| params.get("url"))
            .and_then(Value::as_str),
    };
    match detail {
        Some(d) => {
            let first = crate::text::sanitize(d.lines().next().unwrap_or(""));
            let more = if d.lines().nth(1).is_some() {
                " …"
            } else {
                ""
            };
            format!("{name}: {first}{more}")
        }
        None => name.to_string(),
    }
}

/// The approval callback for `-p` mode. `can_ask` is true when stdin and
/// stderr are both a terminal, so a y/N question can be answered.
pub fn approval_fn(allow: Allow, can_ask: bool, refusals: Refusals) -> phazeai_core::ApprovalFn {
    // One question at a time, even if the agent asks about calls concurrently.
    let prompt_lock = Arc::new(tokio::sync::Mutex::new(()));
    Box::new(move |name, params| {
        let refusals = refusals.clone();
        let prompt_lock = prompt_lock.clone();
        Box::pin(async move {
            let permission = ToolApprovalManager::default().classify_tool(&name, &params);
            if allow.permits(&name, permission) {
                return true;
            }
            let needed = Allow::needed_for(&name, permission);
            if !can_ask {
                let call = describe_call(&name, &params);
                eprintln!("  ✗ blocked {call} (needs {})", needed.flag());
                refusals
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((call, needed));
                return false;
            }
            let _guard = prompt_lock.lock().await;
            let question = render_question(&name, &params, permission).await;
            tokio::task::spawn_blocking(move || ask_yes_no(&question))
                .await
                .unwrap_or(false)
        })
    })
}

fn risk_label(permission: ToolPermission) -> &'static str {
    match permission {
        ToolPermission::ReadOnly => "read-only",
        ToolPermission::Write => "changes files",
        ToolPermission::Execute => "runs a command",
        ToolPermission::Destructive => "DESTRUCTIVE",
    }
}

async fn render_question(name: &str, params: &Value, permission: ToolPermission) -> String {
    let mut out = format!(
        "\nPhazeAI wants to run {} ({}):\n",
        name,
        risk_label(permission)
    );
    for line in approval_preview(name, params).await.iter().take(40) {
        let text = match line {
            PreviewLine::Command(c) => format!("  $ {c}"),
            PreviewLine::Add(l) => format!("  + {l}"),
            PreviewLine::Remove(l) => format!("  - {l}"),
            PreviewLine::Context(l) => format!("    {l}"),
            PreviewLine::Hunk(h) | PreviewLine::Note(h) => format!("  {h}"),
        };
        out.push_str(&crate::text::sanitize(&text));
        out.push('\n');
    }
    out.push_str("Allow? [y/N] ");
    out
}

/// Ask on stderr, read one line from stdin. Anything but y/yes is a no.
fn ask_yes_no(question: &str) -> bool {
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(question.as_bytes());
    let _ = stderr.flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Whether `-p` mode can ask questions interactively.
pub fn can_ask(stdin_piped: bool) -> bool {
    !stdin_piped && std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// The note printed after a run in which calls were refused.
pub fn refusal_summary(refusals: &[(String, Allow)]) -> Option<String> {
    let highest = refusals.iter().map(|(_, a)| *a).max()?;
    let what = match highest {
        Allow::Edit => "file changes",
        _ => "file changes and shell commands",
    };
    Some(format!(
        "{} tool call(s) were blocked. Re-run with {} to allow {what}, or run `phazeai` \
         without -p to approve each one.",
        refusals.len(),
        highest.flag()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels_permit_up_to_their_risk() {
        use ToolPermission::*;
        assert!(Allow::Read.permits("read_file", ReadOnly));
        assert!(Allow::Read.permits("bash", ReadOnly));
        assert!(!Allow::Read.permits("write_file", Write));
        assert!(Allow::Edit.permits("write_file", Write));
        assert!(Allow::Edit.permits("create_directory", Write));
        // A shell command that writes is still a command.
        assert!(!Allow::Edit.permits("bash", Write));
        assert!(!Allow::Edit.permits("bash", Execute));
        assert!(!Allow::Edit.permits("delete_path", Execute));
        assert!(Allow::All.permits("bash", Destructive));
        assert_eq!(Allow::needed_for("write_file", Write), Allow::Edit);
        assert_eq!(Allow::needed_for("bash", Write), Allow::All);
        assert_eq!(Allow::needed_for("bash", Destructive), Allow::All);
    }

    #[tokio::test]
    async fn without_a_terminal_risky_calls_are_refused_and_recorded() {
        let refusals: Refusals = Arc::default();
        let approve = approval_fn(Allow::Read, false, refusals.clone());
        assert!(approve("read_file".into(), json!({"path": "a.rs"})).await);
        assert!(approve("bash".into(), json!({"command": "git status"})).await);
        assert!(!approve("bash".into(), json!({"command": "cargo build"})).await);
        assert!(!approve("write_file".into(), json!({"path": "x", "content": ""})).await);
        let got = refusals.lock().unwrap().clone();
        assert_eq!(
            got,
            vec![
                ("bash: cargo build".to_string(), Allow::All),
                ("write_file: x".to_string(), Allow::Edit),
            ]
        );
        let note = refusal_summary(&got).unwrap();
        assert!(note.starts_with("2 tool call(s) were blocked"));
        assert!(note.contains("--allow all"));
    }

    #[tokio::test]
    async fn edit_level_allows_file_tools_but_no_command_that_changes_things() {
        let refusals: Refusals = Arc::default();
        let approve = approval_fn(Allow::Edit, false, refusals.clone());
        // The file tools.
        assert!(approve("write_file".into(), json!({"path": "x", "content": ""})).await);
        assert!(approve("edit_file".into(), json!({"path": "x"})).await);
        assert!(approve("create_directory".into(), json!({"path": "d"})).await);
        assert!(
            approve(
                "copy_path".into(),
                json!({"source": "a", "destination": "b"})
            )
            .await
        );
        assert!(
            approve(
                "move_path".into(),
                json!({"source": "a", "destination": "b"})
            )
            .await
        );
        // Read-only shell commands still run.
        assert!(approve("bash".into(), json!({"command": "git status"})).await);
        // Shell commands classified as Write are commands, not file edits.
        let blocked = [
            "mkdir x",
            "cp a b",
            "git push",
            "pip install requests",
            "echo hi > f",
            "rm -rf build",
            "cargo build",
        ];
        for cmd in blocked {
            assert!(
                !approve("bash".into(), json!({"command": cmd})).await,
                "{cmd}"
            );
        }
        let got = refusals.lock().unwrap().clone();
        assert_eq!(got.len(), blocked.len());
        assert_eq!(got[0], ("bash: mkdir x".to_string(), Allow::All));
        assert!(got.iter().all(|(_, needed)| *needed == Allow::All));
        let note = refusal_summary(&got).unwrap();
        assert!(note.starts_with("7 tool call(s) were blocked"), "{note}");
        assert!(note.contains("--allow all"), "{note}");
        assert!(note.contains("shell commands"), "{note}");
        assert_eq!(refusal_summary(&[]), None);
    }

    #[test]
    fn describe_call_is_one_line() {
        assert_eq!(
            describe_call("bash", &json!({"command": "make\nmake install"})),
            "bash: make …"
        );
        assert_eq!(describe_call("glob", &json!({})), "glob");
    }

    #[test]
    fn describe_call_strips_escape_sequences_and_control_characters() {
        assert_eq!(
            describe_call(
                "bash",
                &json!({"command": "echo \u{1b}[31mred\u{1b}[0m\rX\u{7}"})
            ),
            "bash: echo redX"
        );
        assert_eq!(
            describe_call("write_file", &json!({"path": "a\u{1b}]0;title\u{7}.rs"})),
            "write_file: a.rs"
        );
    }
}
