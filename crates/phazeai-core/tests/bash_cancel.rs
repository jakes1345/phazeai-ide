//! Interrupting a run kills the shell command it was waiting on.
//!
//! The IDE's Stop button and Esc / Ctrl+C in the CLI set the agent's cancel
//! token; the CLI worker also drops the whole run. The agent used to keep
//! awaiting `bash` until the command finished on its own, and a dropped
//! `tokio::process` future left the child running, so a cancelled `cargo
//! build` or `sleep 30` carried on in the background. Now the agent drops the
//! tool's future as soon as the token is set, and `BashTool` kills the shell's
//! whole process group when its future goes or its timeout fires.
//!
//! These are integration tests rather than unit tests because other lib tests
//! set the global workspace sandbox, which would refuse a shell in a temp dir.
#![cfg(unix)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::tools::BashTool;
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
        assert!(
            Instant::now() < deadline,
            "sleep (pid {pid}) is still running"
        );
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
