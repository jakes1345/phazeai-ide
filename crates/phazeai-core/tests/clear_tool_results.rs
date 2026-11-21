//! A long agent session clears its old tool results in one batch before it
//! gets near compaction: earlier turns' outputs become one-line notes naming
//! the call, the newest stay, and no summary request is made.
//!
//! Drives the production loop (`Agent::run_with_events`) with a scripted
//! model and the real `read_file` tool, and checks what the model was sent.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::tools::sandbox;
use phazeai_core::{
    Agent, AgentEvent, LlmClient, LlmResponse, Message, PhazeError, StreamEvent, ToolDefinition,
};
use serde_json::json;

fn workspace() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("phazeai-clear-results-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        sandbox::set_workspace_root(Some(root.clone()));
        root
    })
}

/// Replies from a script, then "done"; keeps every request it was sent, and
/// counts summary requests (`chat`).
struct Script {
    replies: Mutex<VecDeque<Vec<StreamEvent>>>,
    sent: Arc<Mutex<Vec<Vec<Message>>>>,
    summaries: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl LlmClient for Script {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        *self.summaries.lock().unwrap() += 1;
        Ok(LlmResponse {
            message: Message::assistant("summary"),
            usage: None,
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        self.sent.lock().unwrap().push(messages.to_vec());
        let reply = self
            .replies
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

/// A file of about 6,000 bytes whose every line names it.
fn source(n: usize) -> String {
    (0..150)
        .map(|l| format!("pub fn file{n}_line{l:03}() -> u32 {{ {l} }}\n"))
        .collect()
}

#[tokio::test]
async fn a_long_session_clears_old_reads_instead_of_compacting() {
    let dir = tempfile::tempdir_in(workspace()).unwrap();
    let files: Vec<PathBuf> = (0..8)
        .map(|n| {
            let path = dir.path().join(format!("f{n}.rs"));
            std::fs::write(&path, source(n)).unwrap();
            path
        })
        .collect();
    // One turn per file: the model reads it, then answers.
    let replies = files
        .iter()
        .enumerate()
        .flat_map(|(n, path)| {
            let id = format!("c{n}");
            [
                vec![
                    StreamEvent::ToolCallStart {
                        id: id.clone(),
                        name: "read_file".into(),
                    },
                    StreamEvent::ToolCallDelta {
                        id: id.clone(),
                        arguments_delta: json!({ "path": path }).to_string(),
                    },
                    StreamEvent::ToolCallEnd { id },
                    StreamEvent::Done,
                ],
                vec![
                    StreamEvent::TextDelta(format!("read f{n}")),
                    StreamEvent::Done,
                ],
            ]
        })
        .collect();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let summaries = Arc::new(Mutex::new(0));
    let script = Script {
        replies: Mutex::new(replies),
        sent: sent.clone(),
        summaries: summaries.clone(),
    };
    // Eight reads of about 2,000 tokens each: the clearing point (60% of
    // 20,000) comes after six, compaction's (80%) would come after eight.
    let agent = Agent::new(Box::new(script))
        .with_context_budget(20_000)
        .with_approval(Box::new(|_, _| Box::pin(async { true })));
    let mut events = Vec::new();
    for n in 0..files.len() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        agent
            .run_with_events(format!("read f{n}"), tx)
            .await
            .unwrap();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
    }

    let cleared: Vec<(usize, usize, usize)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResultsCleared {
                results,
                tokens_before,
                tokens_after,
            } => Some((*results, *tokens_before, *tokens_after)),
            _ => None,
        })
        .collect();
    assert_eq!(cleared.len(), 1, "one batch: {cleared:?}");
    let (results, before, after) = cleared[0];
    assert!(results >= 3, "{cleared:?}");
    assert!(after * 2 < before + 2_000, "{cleared:?}");
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::ContextCompacted { .. })));
    assert_eq!(*summaries.lock().unwrap(), 0, "no summary request");

    // The last request: the first file is a note naming its call, the
    // newest three files are there in full, every call is still there.
    let sent = sent.lock().unwrap();
    let last = sent.last().unwrap();
    let text = |m: &Message| m.content.clone();
    let tool_results: Vec<String> = last
        .iter()
        .filter(|m| m.tool_call_id.is_some())
        .map(text)
        .collect();
    assert_eq!(tool_results.len(), 8);
    assert!(
        tool_results[0].starts_with("[Output cleared to save context: read_file"),
        "{}",
        tool_results[0]
    );
    assert!(tool_results[0].contains("f0.rs"));
    assert!(tool_results[0].contains("Run it again"));
    assert!(!tool_results[0].contains("file0_line000"));
    for (n, result) in tool_results.iter().enumerate().skip(5) {
        assert!(
            result.contains(&format!("file{n}_line149")),
            "{n}: {result}"
        );
    }
    assert_eq!(last.iter().filter(|m| m.tool_calls.is_some()).count(), 8);

    // Every other request extended the one before it unchanged, so a prefix
    // cache served it; the clearing changed the prefix once.
    let changed = sent
        .windows(2)
        .filter(|w| {
            w[1].len() < w[0].len() || w[0].iter().zip(&w[1]).any(|(a, b)| a.content != b.content)
        })
        .count();
    assert_eq!(changed, 1);
}
