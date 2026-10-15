//! Tool permissions for single-prompt (`-p`) mode.
//!
//! There's no TUI to show an approval prompt, so `-p` runs with a fixed
//! permission level chosen by `--allow`. Read-only tools always run. Anything
//! above the chosen level is asked about on the terminal when there is one,
//! and refused otherwise (piped input, CI), with a note saying which `--allow`
//! level would permit it.

use std::io::{BufRead, IsTerminal, Write};
use std::sync::{Arc, Mutex};

use phazeai_core::tools::{ToolApprovalManager, ToolPermission};
use serde_json::Value;

use crate::tool_view::{approval_preview, PreviewLine};

/// How much `-p` mode may do without asking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Allow {
    /// Read files and run read-only commands (ls, cat, grep, git status, ...).
    #[default]
    Read,
    /// Also create and edit files.
    Edit,
    /// Also run any command.
    All,
}

impl Allow {
    pub fn permits(self, permission: ToolPermission) -> bool {
        match self {
            Allow::Read => permission == ToolPermission::ReadOnly,
            Allow::Edit => permission <= ToolPermission::Write,
            Allow::All => true,
        }
    }

    /// The smallest level that permits `permission`.
    pub fn needed_for(permission: ToolPermission) -> Allow {
        match permission {
            ToolPermission::ReadOnly => Allow::Read,
            ToolPermission::Write => Allow::Edit,
            ToolPermission::Execute | ToolPermission::Destructive => Allow::All,
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

/// One line summarising a tool call, e.g. `bash: cargo build` or `edit_file: src/main.rs`.
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
            let first = d.lines().next().unwrap_or("");
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
            if allow.permits(permission) {
                return true;
            }
            let needed = Allow::needed_for(permission);
            if !can_ask {
                eprintln!(
                    "  ✗ blocked {} (needs {})",
                    describe_call(&name, &params),
                    needed.flag()
                );
                refusals
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((describe_call(&name, &params), needed));
                return false;
            }
            let _guard = prompt_lock.lock().await;
            let question = render_question(&name, &params, permission);
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

fn render_question(name: &str, params: &Value, permission: ToolPermission) -> String {
    let mut out = format!(
        "\nPhazeAI wants to run {} ({}):\n",
        name,
        risk_label(permission)
    );
    for line in approval_preview(name, params).iter().take(40) {
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
    let highest = refusals.iter().map(|(_, a)| *a).max_by_key(|a| *a as u8)?;
    let what = match highest {
        Allow::Edit => "file changes",
        _ => "file changes and commands",
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
        assert!(Allow::Read.permits(ReadOnly));
        assert!(!Allow::Read.permits(Write));
        assert!(Allow::Edit.permits(Write));
        assert!(!Allow::Edit.permits(Execute));
        assert!(Allow::All.permits(Destructive));
        assert_eq!(Allow::needed_for(Write), Allow::Edit);
        assert_eq!(Allow::needed_for(Destructive), Allow::All);
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
    async fn edit_level_allows_writes_but_not_commands() {
        let refusals: Refusals = Arc::default();
        let approve = approval_fn(Allow::Edit, false, refusals.clone());
        assert!(approve("edit_file".into(), json!({"path": "x"})).await);
        assert!(!approve("bash".into(), json!({"command": "rm -rf build"})).await);
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
}
