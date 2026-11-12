//! Checkpoints undo exactly what an agent run wrote, and never the user's own
//! edits.
//!
//! These tests drive the production loop (`Agent::run_with_events`) with a
//! scripted model and the real file tools, with and without a diff review,
//! then undo with the same `Checkpoints` the IDE chat shares between runs.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::tools::sandbox;
use phazeai_core::{
    Agent, Checkpoints, DiffAnswer, LlmClient, LlmResponse, Message, PhazeError, StreamEvent,
    ToolDefinition,
};
use serde_json::{json, Value};

fn workspace() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("phazeai-checkpoint-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        sandbox::set_workspace_root(Some(root.clone()));
        root
    })
}

fn project() -> tempfile::TempDir {
    tempfile::tempdir_in(workspace()).unwrap()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
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

/// Run `script` as one request with `checkpoints`, approving every tool.
async fn run(
    request: &str,
    script: Script,
    checkpoints: &Arc<std::sync::Mutex<Checkpoints>>,
    review: Option<DiffAnswer>,
) {
    let mut agent = Agent::new(Box::new(script))
        .with_approval(Box::new(|_, _| Box::pin(async { true })))
        .with_checkpoints(checkpoints.clone());
    if let Some(answer) = review {
        agent = agent.with_diff_hook(Box::new(move |_, _, _| {
            let answer = answer.clone();
            Box::pin(async move { answer })
        }));
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent.run_with_events(request, tx).await.unwrap();
}

#[tokio::test]
async fn undo_restores_edited_files_and_removes_created_ones() {
    let dir = project();
    let (lib, new) = (dir.path().join("lib.rs"), dir.path().join("new.rs"));
    std::fs::write(&lib, "fn a() {}\nfn b() {}\n").unwrap();
    let cp = Arc::new(std::sync::Mutex::new(Checkpoints::new()));

    let script = calls(&[
        (
            "edit_file",
            json!({"path": lib, "old_text": "fn a() {}", "new_text": "fn a() { 1 }"}),
        ),
        (
            "edit_file",
            json!({"path": lib, "old_text": "fn b() {}", "new_text": "fn b() { 2 }"}),
        ),
        (
            "write_file",
            json!({"path": new, "content": "pub fn n() {}\n"}),
        ),
    ]);
    run("add bodies and a module", script, &cp, None).await;
    assert_eq!(read(&lib), "fn a() { 1 }\nfn b() { 2 }\n");
    assert!(new.exists());

    let report = cp.lock().unwrap().undo_last().unwrap();
    assert_eq!(read(&lib), "fn a() {}\nfn b() {}\n");
    assert!(!new.exists());
    assert_eq!(report.restored, [lib]);
    assert_eq!(report.removed, [new]);
    assert_eq!(report.label, "add bodies and a module");
}

#[tokio::test]
async fn your_edit_after_the_run_is_kept() {
    let dir = project();
    let main = dir.path().join("main.rs");
    std::fs::write(&main, "v1\n").unwrap();
    let cp = Arc::new(std::sync::Mutex::new(Checkpoints::new()));

    run(
        "rewrite",
        calls(&[("write_file", json!({"path": main, "content": "v2\n"}))]),
        &cp,
        None,
    )
    .await;
    std::fs::write(&main, "v2\nmine\n").unwrap();

    let report = cp.lock().unwrap().undo_last().unwrap();
    assert_eq!(read(&main), "v2\nmine\n");
    assert_eq!(report.kept, [main]);
}

#[tokio::test]
async fn undo_goes_back_one_run_at_a_time_and_skips_rejected_reviews() {
    let dir = project();
    let f = dir.path().join("f.txt");
    std::fs::write(&f, "0\n").unwrap();
    let cp = Arc::new(std::sync::Mutex::new(Checkpoints::new()));

    let write = |text: &str| calls(&[("write_file", json!({"path": f, "content": text}))]);
    run("one", write("1\n"), &cp, Some(DiffAnswer::Approve)).await;
    run("two", write("2\n"), &cp, None).await;
    run("rejected", write("3\n"), &cp, Some(DiffAnswer::Reject)).await;
    assert_eq!(read(&f), "2\n");
    assert_eq!(cp.lock().unwrap().undoable(), 2);

    assert_eq!(cp.lock().unwrap().undo_last().unwrap().label, "two");
    assert_eq!(read(&f), "1\n");
    assert_eq!(cp.lock().unwrap().undo_last().unwrap().label, "one");
    assert_eq!(read(&f), "0\n");
    assert!(cp.lock().unwrap().undo_last().is_none());
}

#[tokio::test]
async fn a_partly_kept_review_undoes_to_before_the_run() {
    let dir = project();
    let f = dir.path().join("g.txt");
    let before = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
    std::fs::write(&f, before).unwrap();
    let cp = Arc::new(std::sync::Mutex::new(Checkpoints::new()));
    let after = "A\nb\nc\nd\ne\nf\ng\nh\ni\nJ\n";
    run(
        "capitalise",
        calls(&[("write_file", json!({"path": f, "content": after}))]),
        &cp,
        Some(DiffAnswer::Keep(vec![true, false])),
    )
    .await;
    assert_eq!(read(&f), "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");

    cp.lock().unwrap().undo_last().unwrap();
    assert_eq!(read(&f), before);
}
