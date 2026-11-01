//! Interrupting a run kills the command it was waiting on: the shell of
//! `bash`, and the `cargo check` of `diagnostics`. Also: how `bash` returns
//! long output.
//!
//! The IDE's Stop button and Esc / Ctrl+C in the CLI set the agent's cancel
//! token; the CLI worker also drops the whole run. The agent used to keep
//! awaiting `bash` until the command finished on its own, and a dropped
//! `tokio::process` future left the child running, so a cancelled `cargo
//! build` or `sleep 30` carried on in the background. Now the agent drops the
//! tool's future as soon as the token is set, and both tools kill the
//! command's whole process group when their future goes or their timeout
//! fires (`process::output_or_kill`).
//!
//! These are integration tests rather than unit tests because other lib tests
//! set the global workspace sandbox, which would refuse a shell in a temp dir.
#![cfg(unix)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::tools::{BashTool, DiagnosticsTool};
use phazeai_core::{
    Agent, LlmClient, LlmResponse, Message, PhazeError, StreamEvent, Tool, ToolDefinition,
    ToolRegistry,
};
use serde_json::json;

/// `sleep 30` in the background with its pid written to `pid_file`, then wait
/// for it: the shell stays busy for 30 s unless something stops it, and the
/// test learns the grandchild's pid.
fn sleeper(pid_file: &Path) -> String {
    format!("sleep 30 & echo $! > '{}'; wait", pid_file.display())
}

/// Pid of the `sleep` the shell started, once the shell has written it.
async fn read_pid(pid_file: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = std::fs::read_to_string(pid_file)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "the shell never started sleep");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// True once `pid` is gone, or is a zombie waiting for its new parent to reap
/// it (killed, but still listed by the kernel).
fn is_dead(pid: i32) -> bool {
    // SAFETY: signal 0 only checks whether the process exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return true;
    }
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| {
            stat.rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
        .unwrap_or(false)
}

async fn wait_until_dead(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !is_dead(pid) {
        assert!(Instant::now() < deadline, "process {pid} is still running");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn dropping_the_execute_future_kills_the_shell_and_its_children() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let tool = Arc::new(BashTool::new(dir.path().to_path_buf()));
    let command = sleeper(&pid_file);
    let run = tokio::spawn({
        let tool = tool.clone();
        async move { tool.execute(json!({ "command": command })).await }
    });
    let pid = read_pid(&pid_file).await;
    assert!(
        !is_dead(pid),
        "sleep should be running before the interrupt"
    );

    // What the CLI worker does on Esc / Ctrl+C: stop awaiting the run, which
    // drops the tool's future.
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    wait_until_dead(pid).await;
}

#[tokio::test]
async fn timeout_kills_the_shell_and_its_children() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let tool = BashTool::new(dir.path().to_path_buf());
    let err = tool
        .execute(json!({ "command": sleeper(&pid_file), "timeout_secs": 1 }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("timed out after 1s"), "{err}");
    let pid = read_pid(&pid_file).await;
    wait_until_dead(pid).await;
}

#[tokio::test]
async fn finished_commands_still_report_output_exit_code_and_cwd() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let tool = BashTool::new(dir.path().to_path_buf());
    let out = tool
        .execute(json!({ "command": "cd sub && echo out && echo err >&2" }))
        .await
        .unwrap();
    assert_eq!(out["success"], true);
    assert_eq!(out["exit_code"], 0);
    assert_eq!(out["stdout"], "out");
    assert_eq!(out["stderr"], "err\n");

    // The cwd change persisted into the next call.
    let out = tool.execute(json!({ "command": "pwd" })).await.unwrap();
    assert!(
        out["stdout"].as_str().unwrap().trim().ends_with("/sub"),
        "{out}"
    );

    let out = tool
        .execute(json!({ "command": "echo failing >&2; exit 3" }))
        .await
        .unwrap();
    assert_eq!(out["success"], false);
    assert_eq!(out["exit_code"], 3);
    assert_eq!(out["stderr"], "failing\n");
}

/// The line a long output is cut around: `OMITTED_PREFIX`, the number of
/// bytes left out, then `OMITTED_SUFFIX`.
const OMITTED_PREFIX: &str = "\n[... ";
const OMITTED_SUFFIX: &str = " bytes omitted ...]\n";

/// Long output keeps its start and its end (where errors and totals usually
/// are) around a line saying how many bytes were left out. It used to be cut
/// with `String::truncate` at byte 30 000, which panics when that byte falls
/// inside a multi-byte character, so a test run full of check marks killed
/// the whole agent run instead of returning.
#[tokio::test]
async fn long_output_keeps_its_start_and_end_and_never_splits_a_character() {
    let dir = tempfile::tempdir().unwrap();
    let content = format!("HEAD-START\n{}\nTAIL-END", "✓".repeat(20_000));
    // The old cut point lands inside a 3-byte check mark.
    assert!(!content.is_char_boundary(30_000));
    std::fs::write(dir.path().join("big.txt"), &content).unwrap();
    let tool = BashTool::new(dir.path().to_path_buf());
    let out = tool
        .execute(json!({ "command": "cat big.txt; cat big.txt >&2" }))
        .await
        .unwrap();
    assert_eq!(out["exit_code"], 0);
    for stream in ["stdout", "stderr"] {
        let text = out[stream].as_str().unwrap();
        let (head, rest) = text
            .split_once(OMITTED_PREFIX)
            .unwrap_or_else(|| panic!("{stream} was not cut ({} bytes)", text.len()));
        let (omitted, tail) = rest.split_once(OMITTED_SUFFIX).unwrap();
        let omitted: usize = omitted.parse().unwrap();
        assert!(head.starts_with("HEAD-START\n"), "{stream} lost its start");
        assert!(tail.ends_with("\nTAIL-END"), "{stream} lost its end");
        assert!(
            content.starts_with(head) && content.ends_with(tail),
            "{stream}"
        );
        assert_eq!(head.len() + omitted + tail.len(), content.len(), "{stream}");
        assert!(head.len() + tail.len() <= 30_000, "{stream} kept too much");
    }
}

/// A crate whose build script writes its own pid and its parent's (cargo's)
/// to `pids`, then sleeps: `cargo check` on it stays busy for 30 s unless
/// something stops it, like a check of a large project.
fn slow_to_check_crate(dir: &Path) {
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"slow-to-check\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "").unwrap();
    std::fs::write(
        dir.join("build.rs"),
        r#"fn main() {
    let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let pids = format!("{} {}", std::process::id(), std::os::unix::process::parent_id());
    std::fs::write(dir.join("pids.part"), pids).unwrap();
    std::fs::rename(dir.join("pids.part"), dir.join("pids")).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(30));
}
"#,
    )
    .unwrap();
}

/// Stop while `diagnostics` waits on `cargo check` kills cargo and the build
/// script it is running. The checker used to be awaited with plain
/// `output()`, and a dropped `tokio::process` future leaves its child running,
/// so a cancelled check of a large project kept compiling (and holding the
/// build directory lock) in the background.
#[tokio::test]
async fn dropping_the_diagnostics_future_kills_cargo_and_its_build_script() {
    let dir = tempfile::tempdir().unwrap();
    slow_to_check_crate(dir.path());
    let path = dir.path().to_str().unwrap().to_string();
    let run = tokio::spawn(async move {
        DiagnosticsTool
            .execute(json!({ "path": path, "language": "rust" }))
            .await
    });

    // Cargo compiles the build script first, so allow it a while.
    let deadline = Instant::now() + Duration::from_secs(180);
    let pids_file = dir.path().join("pids");
    let pids: Vec<i32> = loop {
        if let Ok(text) = std::fs::read_to_string(&pids_file) {
            break text
                .split_whitespace()
                .map(|p| p.parse().unwrap())
                .collect();
        }
        if run.is_finished() {
            panic!(
                "diagnostics returned before the build script ran: {:?}",
                run.await
            );
        }
        assert!(
            Instant::now() < deadline,
            "cargo never ran the build script"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let [build_script, cargo] = pids[..] else {
        panic!("expected two pids, got {pids:?}");
    };
    assert!(
        !is_dead(build_script) && !is_dead(cargo),
        "the check should be running"
    );

    // What the CLI worker does on Esc / Ctrl+C, and what the agent does on
    // Stop: stop awaiting the tool, which drops its future.
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    wait_until_dead(cargo).await;
    wait_until_dead(build_script).await;
}

/// Scripted provider: the first turn asks for one `bash` call, every later
/// turn just answers.
struct ScriptedLlm {
    command: String,
    turns: Mutex<u32>,
}

#[async_trait::async_trait]
impl LlmClient for ScriptedLlm {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant("unused"),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        let (tx, rx) = unbounded();
        let mut turns = self.turns.lock().unwrap();
        *turns += 1;
        if *turns == 1 {
            let id = "call_1".to_string();
            let arguments_delta = json!({ "command": self.command }).to_string();
            for event in [
                StreamEvent::ToolCallStart {
                    id: id.clone(),
                    name: "bash".into(),
                },
                StreamEvent::ToolCallDelta {
                    id: id.clone(),
                    arguments_delta,
                },
                StreamEvent::ToolCallEnd { id },
            ] {
                tx.unbounded_send(event).unwrap();
            }
        } else {
            tx.unbounded_send(StreamEvent::TextDelta("done".into()))
                .unwrap();
        }
        tx.unbounded_send(StreamEvent::Done).unwrap();
        Ok(rx)
    }
}

#[tokio::test]
async fn cancelling_a_run_kills_the_shell_command_it_is_waiting_on() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let cancel = Arc::new(AtomicBool::new(false));
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(BashTool::new(dir.path().to_path_buf())));
    let agent = Agent::new(Box::new(ScriptedLlm {
        command: sleeper(&pid_file),
        turns: Mutex::new(0),
    }))
    .with_tools(tools)
    .with_cancel_token(cancel.clone());

    // What the IDE's Stop button does once the sleeper is running: set the
    // token and nothing else.
    let stop = async {
        let pid = read_pid(&pid_file).await;
        assert!(!is_dead(pid), "sleep should be running before Stop");
        cancel.store(true, Ordering::SeqCst);
        (pid, Instant::now())
    };
    let (result, (pid, stopped_at)) = tokio::join!(agent.run("run the sleeper"), stop);
    let reaction = stopped_at.elapsed();

    assert!(matches!(result, Err(PhazeError::Cancelled)), "{result:?}");
    assert!(
        reaction < Duration::from_secs(5),
        "the run took {reaction:?} to stop"
    );
    wait_until_dead(pid).await;
}
