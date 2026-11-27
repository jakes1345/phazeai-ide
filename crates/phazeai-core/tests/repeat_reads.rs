//! A file the agent reads again without anything changing is not sent to the
//! model a second time: the second result points at the first, which is
//! still in the conversation. A file that changed in between sends only the
//! lines that differ from that first read. A first result that is no longer
//! there in full, or a change too large for a diff to save much, sends the
//! file again.
//!
//! Drives the production loop (`Agent::run_with_events`) with a scripted
//! model and the real file tools, then reads the conversation the next
//! request would carry.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
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
        let root =
            std::env::temp_dir().join(format!("phazeai-repeat-reads-{}", std::process::id()));
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

/// Run `script` approving every tool; the tool results, in order, as the
/// conversation keeps them.
async fn tool_results(script: Script) -> Vec<(String, String)> {
    let agent =
        Agent::new(Box::new(script)).with_approval(Box::new(|_, _| Box::pin(async { true })));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    agent.run_with_events("look at the code", tx).await.unwrap();
    agent
        .get_conversation_history()
        .await
        .into_iter()
        .filter_map(|m| Some((m.tool_call_id?, m.content)))
        .collect()
}

/// A source file long enough that pointing at it beats sending it again.
fn long_source() -> String {
    (0..40)
        .map(|n| format!("pub fn function_{n}() -> u32 {{ {n} }}\n"))
        .collect()
}

#[tokio::test]
async fn an_unchanged_file_read_again_points_at_the_first_read() {
    let dir = tempfile::tempdir_in(workspace()).unwrap();
    let lib = dir.path().join("lib.rs");
    std::fs::write(&lib, long_source()).unwrap();

    let results = tool_results(calls(&[
        ("read_file", json!({"path": lib})),
        ("read_file", json!({"path": lib})),
    ]))
    .await;

    assert_eq!(results.len(), 2);
    assert!(results[0].1.contains("function_39"), "{}", results[0].1);
    assert!(
        results[1]
            .1
            .starts_with("Unchanged: same result as your earlier call c0"),
        "{}",
        results[1].1
    );
    assert!(results[1].1.len() * 5 < results[0].1.len());
}

#[tokio::test]
async fn a_file_changed_in_between_sends_only_the_changed_lines() {
    let dir = tempfile::tempdir_in(workspace()).unwrap();
    let lib = dir.path().join("lib.rs");
    std::fs::write(&lib, long_source()).unwrap();

    let results = tool_results(calls(&[
        ("read_file", json!({"path": lib})),
        (
            "edit_file",
            json!({"path": lib, "old_text": "{ 7 }", "new_text": "{ 700 }"}),
        ),
        ("read_file", json!({"path": lib})),
        ("read_file", json!({"path": lib})),
    ]))
    .await;

    assert_eq!(results.len(), 4);
    // The read after the edit lists the one changed line against the first
    // read, which is still above in full.
    let note = &results[2].1;
    assert!(
        note.starts_with("Changed since your earlier read c0"),
        "{note}"
    );
    assert!(
        note.contains("@@ -8,1 +8,1 @@\n-pub fn function_7() -> u32 { 7 }\n+8\tpub fn function_7() -> u32 { 700 }"),
        "{note}"
    );
    assert!(!note.contains("function_8"), "{note}");
    assert!(note.len() * 4 < results[0].1.len());
    // Reading once more still compares with that first read.
    assert_eq!(&results[3].1, note);
}

#[tokio::test]
async fn other_ranges_and_files_changed_throughout_are_sent_in_full() {
    let dir = tempfile::tempdir_in(workspace()).unwrap();
    let lib = dir.path().join("lib.rs");
    std::fs::write(&lib, long_source()).unwrap();
    let rewritten: String = (0..40)
        .map(|n| format!("pub fn renamed_{n}() -> u64 {{ {n} * 2 }}\n"))
        .collect();

    let results = tool_results(calls(&[
        ("read_file", json!({"path": lib, "limit": 10})),
        ("write_file", json!({"path": lib, "content": rewritten})),
        // Another range was never shown: sent as it is.
        ("read_file", json!({"path": lib, "offset": 11, "limit": 10})),
        // Every line changed: a diff would be no shorter, so the file is sent.
        ("read_file", json!({"path": lib, "limit": 10})),
    ]))
    .await;

    assert_eq!(results.len(), 4);
    assert!(results[2].1.contains("renamed_10"), "{}", results[2].1);
    assert!(
        results[3].1.starts_with("1\tpub fn renamed_0"),
        "{}",
        results[3].1
    );
}

#[tokio::test]
async fn short_results_and_other_ranges_are_sent_as_they_are() {
    let dir = tempfile::tempdir_in(workspace()).unwrap();
    let small = dir.path().join("small.rs");
    std::fs::write(&small, "fn tiny() {}\n").unwrap();
    let lib = dir.path().join("lib.rs");
    std::fs::write(&lib, long_source()).unwrap();

    let results = tool_results(calls(&[
        ("read_file", json!({"path": small})),
        ("read_file", json!({"path": small})),
        ("read_file", json!({"path": lib, "offset": 1, "limit": 20})),
        ("read_file", json!({"path": lib, "offset": 21, "limit": 20})),
    ]))
    .await;

    assert_eq!(
        results[0].1, results[1].1,
        "a short result is cheaper than a note"
    );
    assert!(results[2].1.contains("function_0"));
    assert!(results[3].1.contains("function_39"), "{}", results[3].1);
    assert!(!results.iter().any(|(_, r)| r.starts_with("Unchanged")));
}

#[tokio::test]
async fn a_repeated_search_points_at_the_first_one() {
    let dir = tempfile::tempdir_in(workspace()).unwrap();
    std::fs::write(dir.path().join("lib.rs"), long_source()).unwrap();

    let search = json!({"pattern": "function_", "path": dir.path()});
    let results = tool_results(calls(&[("grep", search.clone()), ("grep", search)])).await;

    assert_eq!(results.len(), 2);
    assert!(results[0].1.contains("function_1"), "{}", results[0].1);
    assert!(results[1].1.starts_with("Unchanged"), "{}", results[1].1);
}
