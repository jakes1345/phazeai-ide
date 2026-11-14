//! Plan and Ask modes are read-only for real: while the agent's read-only
//! switch is on, a call that would change something does not run (and no
//! approval is asked for it), while reading still works. Turning the switch
//! off lets the next call act.
//!
//! Drives the production loop with a scripted model and the real tools.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::tools::sandbox;
use phazeai_core::{
    Agent, LlmClient, LlmResponse, Message, PhazeError, StreamEvent, ToolDefinition,
};
use serde_json::{json, Value};

fn workspace() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("phazeai-read-only-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        sandbox::set_workspace_root(Some(root.clone()));
        root
    })
}

/// Replies from a script, then "done".
struct Script(Mutex<VecDeque<Vec<StreamEvent>>>);

#[async_trait::async_trait]
impl LlmClient for Script {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant("summary"),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        let reply = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![StreamEvent::TextDelta("done".into()), StreamEvent::Done]);
        let (tx, rx) = unbounded();
        for event in reply {
            tx.unbounded_send(event).unwrap();
        }
        Ok(rx)
    }
}

/// One model reply per call, each making that one tool call.
fn calls(list: &[(&str, Value)]) -> Script {
    let replies = list
        .iter()
        .enumerate()
        .map(|(n, (tool, args))| {
            let id = format!("c{n}");
            vec![
                StreamEvent::ToolCallStart {
                    id: id.clone(),
                    name: (*tool).into(),
                },
                StreamEvent::ToolCallDelta {
                    id: id.clone(),
                    arguments_delta: args.to_string(),
                },
                StreamEvent::ToolCallEnd { id },
                StreamEvent::Done,
            ]
        })
        .collect();
    Script(Mutex::new(replies))
}

/// Run `script` with the read-only switch `flag`; the tool results, in order,
/// and the tools approval was asked for.
async fn run(script: Script, flag: Arc<AtomicBool>) -> (Vec<String>, Vec<String>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    let agent = Agent::new(Box::new(script))
        .with_read_only(flag)
        .with_approval(Box::new(move |name, _| {
            seen.lock().unwrap().push(name);
            Box::pin(async { true })
        }));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent.run_with_events("plan the change", tx).await.unwrap();
    let results = agent
        .get_conversation_history()
        .await
        .into_iter()
        .filter(|m| m.tool_call_id.is_some())
        .map(|m| m.content)
        .collect();
    let asked = asked.lock().unwrap().clone();
    (results, asked)
}

#[tokio::test]
async fn read_only_reads_but_does_not_write_delete_or_run() {
    let dir = tempfile::tempdir_in(workspace()).unwrap();
    let lib = dir.path().join("lib.rs");
    std::fs::write(&lib, "fn a() {}\n").unwrap();
    let new = dir.path().join("new.rs");

    let (results, asked) = run(
        calls(&[
            ("read_file", json!({"path": lib})),
            ("write_file", json!({"path": new, "content": "x"})),
            (
                "edit_file",
                json!({"path": lib, "old_text": "fn a() {}", "new_text": "fn b() {}"}),
            ),
            ("delete_path", json!({"path": lib})),
            (
                "bash",
                json!({"command": format!("touch {}", new.display())}),
            ),
        ]),
        Arc::new(AtomicBool::new(true)),
    )
    .await;

    assert!(results[0].contains("fn a() {}"), "{}", results[0]);
    for refused in &results[1..] {
        assert!(
            refused.ends_with(phazeai_core::READ_ONLY_REFUSAL),
            "{refused}"
        );
    }
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), "fn a() {}\n");
    assert!(!new.exists());
    // Nothing was put to the user: a refused call is not an approval prompt.
    assert!(asked.iter().all(|t| t == "read_file"), "{asked:?}");
}

#[tokio::test]
async fn turning_read_only_off_lets_the_plan_be_carried_out() {
    let dir = tempfile::tempdir_in(workspace()).unwrap();
    let new = dir.path().join("new.rs");
    let flag = Arc::new(AtomicBool::new(true));

    let (planned, _) = run(
        calls(&[(
            "write_file",
            json!({"path": new, "content": "pub fn n() {}\n"}),
        )]),
        flag.clone(),
    )
    .await;
    assert_eq!(planned.len(), 1);
    assert!(
        planned[0].ends_with(phazeai_core::READ_ONLY_REFUSAL),
        "{}",
        planned[0]
    );
    assert!(!new.exists());

    flag.store(false, Ordering::Relaxed);
    let (done, asked) = run(
        calls(&[(
            "write_file",
            json!({"path": new, "content": "pub fn n() {}\n"}),
        )]),
        flag,
    )
    .await;
    assert!(
        !done[0].contains(phazeai_core::READ_ONLY_REFUSAL),
        "{}",
        done[0]
    );
    assert_eq!(asked, ["write_file"]);
    assert_eq!(std::fs::read_to_string(&new).unwrap(), "pub fn n() {}\n");
}
