//! A file change approved in a diff review is exactly what gets written, or
//! nothing is.
//!
//! The IDE chat waves `write_file` and `edit_file` through its approval
//! callback and gates them with the agent's diff hook instead: the user is
//! shown (path, before, after) and approves or rejects. These tests drive the
//! production loop (`Agent::run_with_events`) with a scripted model, the real
//! tools (the default registry, as the chat uses), the chat's approval rule
//! and a diff hook that records what it was shown, then compare that with
//! what is on disk. Every test runs inside a sandbox root, as the IDE does.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::tools::sandbox;
use phazeai_core::{
    Agent, ApprovalFn, DiffHookFn, LlmClient, LlmResponse, Message, PhazeError, StreamEvent,
    ToolDefinition,
};
use serde_json::{json, Value};

// ── The workspace ────────────────────────────────────────────────────────────

/// The sandbox root of every test in this file (the IDE sets one at start).
fn workspace() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("phazeai-diff-review-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        sandbox::set_workspace_root(Some(root.clone()));
        root
    })
}

/// A fresh directory inside the workspace.
fn project() -> tempfile::TempDir {
    tempfile::tempdir_in(workspace()).unwrap()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// `args` with `"path"` set to `path`.
fn at(path: &Path, mut args: Value) -> Value {
    args["path"] = json!(path);
    args
}

// ── The model, the review and the run ────────────────────────────────────────

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

/// A model reply that makes one tool call.
fn call(tool: &str, args: &Value) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCallStart {
            id: "c1".into(),
            name: tool.into(),
        },
        StreamEvent::ToolCallDelta {
            id: "c1".into(),
            arguments_delta: args.to_string(),
        },
        StreamEvent::ToolCallEnd { id: "c1".into() },
        StreamEvent::Done,
    ]
}

/// What a diff review was shown.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    path: String,
    before: String,
    after: String,
}

/// The IDE chat's approval rule (`panels/chat.rs`): file writes are left to
/// the diff review; nothing else is called in these tests.
fn chat_approval() -> ApprovalFn {
    Box::new(|name, _| {
        let file_write = matches!(name.as_str(), "write_file" | "edit_file");
        Box::pin(async move { file_write })
    })
}

/// A diff hook that records what it is shown, lets `during` act while the
/// review is open (the user saving the file in the editor, say), then
/// answers `approve`.
fn reviewer(
    log: &Arc<Mutex<Vec<Shown>>>,
    approve: bool,
    during: impl Fn() + Send + Sync + 'static,
) -> DiffHookFn {
    let log = log.clone();
    Box::new(move |path, before, after| {
        log.lock().unwrap().push(Shown {
            path,
            before,
            after,
        });
        during();
        Box::pin(async move { approve })
    })
}

/// A run in which the model makes one `tool` call with `args` and the review
/// answers `approve`: what the review was shown, and whether the call
/// succeeded with what result.
async fn run(
    tool: &str,
    args: Value,
    approve: bool,
    during: impl Fn() + Send + Sync + 'static,
) -> (Vec<Shown>, bool, String) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let script = Script(Mutex::new(VecDeque::from([call(tool, &args)])));
    let agent = Agent::new(Box::new(script))
        .with_approval(chat_approval())
        .with_diff_hook(reviewer(&log, approve, during));
    let response = agent.run("change the file").await.expect("the run ends");
    let [done] = response.tool_calls.as_slice() else {
        panic!("expected one tool call: {:?}", response.tool_calls);
    };
    let shown = log.lock().unwrap().clone();
    (shown, done.success, done.result_summary.clone())
}

// ── An approved diff is what is written ─────────────────────────────────────

#[tokio::test]
async fn an_approved_edit_writes_exactly_the_diff_that_was_shown() {
    // (case, file content, edit_file arguments, the content after the edit)
    let cases = [
        (
            "one occurrence",
            "alpha\nbeta\ngamma\n",
            json!({"old_text": "beta", "new_text": "BETA"}),
            "alpha\nBETA\ngamma\n",
        ),
        (
            "replace_all: every occurrence, CRLF kept",
            "let x = 1;\r\nlet y = x;\r\nprint(x);\r\n",
            json!({"old_text": "x", "new_text": "count", "replace_all": true}),
            "let count = 1;\r\nlet y = count;\r\nprint(count);\r\n",
        ),
        (
            "replace_all with a multi-byte old_text",
            "naïve, naïve\nnaïve\n",
            json!({"old_text": "ï", "new_text": "i", "replace_all": true}),
            "naive, naive\nnaive\n",
        ),
        (
            "three occurrences, context picks the second",
            "fn ä() { todo!() } // 一\nfn ö() { todo!() } // 二\nfn ü() { todo!() } // 三\n",
            json!({"old_text": "todo!()", "new_text": "42", "context": "fn ö() { todo!() }"}),
            "fn ä() { todo!() } // 一\nfn ö() { 42 } // 二\nfn ü() { todo!() } // 三\n",
        ),
        (
            "old_text across a CRLF line break",
            "a\r\nb\r\nc\r\n",
            json!({"old_text": "a\r\nb", "new_text": "a b"}),
            "a b\r\nc\r\n",
        ),
        (
            "replace_all: false, one occurrence",
            "日本語のテキスト\n",
            json!({"old_text": "テキスト", "new_text": "text", "replace_all": false}),
            "日本語のtext\n",
        ),
        (
            "replace_all: true, one occurrence, no final newline",
            "start 🦀 end",
            json!({"old_text": "🦀", "new_text": "🐍🐍", "replace_all": true}),
            "start 🐍🐍 end",
        ),
    ];
    for (case, content, edit, expected) in cases {
        let dir = project();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, content).unwrap();

        let (shown, ok, result) = run("edit_file", at(&file, edit), true, || {}).await;

        assert_eq!(shown.len(), 1, "{case}: reviews: {shown:?} ({result})");
        let shown = &shown[0];
        assert!(ok, "{case}: {result}");
        assert_eq!(
            read(&file),
            shown.after,
            "{case}: what was written is not what was approved"
        );
        assert_eq!(shown.after, expected, "{case}: the reviewed change");
        assert_eq!(shown.before, content, "{case}");
        assert_eq!(shown.path, file.display().to_string(), "{case}");
    }
}

// ── A call that would fail shows nothing to approve ─────────────────────────

#[tokio::test]
async fn a_call_the_tool_would_refuse_shows_no_diff_and_writes_nothing() {
    let dir = project();
    let outside = tempfile::tempdir().unwrap();
    assert!(!outside.path().starts_with(workspace()));
    let twice = "x = 1\nx = 1\n";
    // (case, tool, file, its content (None: absent), arguments, the error)
    let cases = [
        (
            "old_text twice, no replace_all or context",
            "edit_file",
            dir.path().join("twice.txt"),
            Some(twice),
            json!({"old_text": "x = 1", "new_text": "x = 2"}),
            "matches 2 times",
        ),
        (
            "old_text missing",
            "edit_file",
            dir.path().join("abc.txt"),
            Some("abc\n"),
            json!({"old_text": "zzz", "new_text": "y"}),
            "old_text not found",
        ),
        (
            "context missing",
            "edit_file",
            dir.path().join("ctx.txt"),
            Some(twice),
            json!({"old_text": "x = 1", "new_text": "x = 2", "context": "no such text"}),
            "'context' string was not found",
        ),
        (
            "edit outside the workspace",
            "edit_file",
            outside.path().join("notes.txt"),
            Some("abc\n"),
            json!({"old_text": "abc", "new_text": "xyz"}),
            "outside the workspace",
        ),
        (
            "write outside the workspace",
            "write_file",
            outside.path().join("new.txt"),
            None,
            json!({"content": "hello\n"}),
            "outside the workspace",
        ),
    ];
    for (case, tool, file, content, args, error) in cases {
        if let Some(content) = content {
            std::fs::write(&file, content).unwrap();
        }

        let (shown, ok, result) = run(tool, at(&file, args), true, || {}).await;

        assert!(shown.is_empty(), "{case}: a diff was offered: {shown:?}");
        assert!(!ok, "{case}: {result}");
        assert!(result.contains(error), "{case}: {result}");
        match content {
            Some(content) => assert_eq!(read(&file), content, "{case}"),
            None => assert!(!file.exists(), "{case}"),
        }
    }
}

// ── The file changes while the diff is open ─────────────────────────────────

#[tokio::test]
async fn a_file_changed_while_its_diff_is_reviewed_is_not_overwritten() {
    // The user saves the file in the editor while the review is open, then
    // approves. (case, tool, content before (None: absent), arguments, saved)
    let cases = [
        (
            "edit with replace_all",
            "edit_file",
            Some("v1 v1\n"),
            json!({"old_text": "v1", "new_text": "v2", "replace_all": true}),
            "v1 v1 v1\n",
        ),
        (
            "write over an existing file",
            "write_file",
            Some("draft 1\n"),
            json!({"content": "the agent's version\n"}),
            "draft 2\n",
        ),
        (
            "write to a new file",
            "write_file",
            None,
            json!({"content": "the agent's version\n"}),
            "the user's own file\n",
        ),
    ];
    for (case, tool, content, args, saved) in cases {
        let dir = project();
        let file = dir.path().join("f.txt");
        if let Some(content) = content {
            std::fs::write(&file, content).unwrap();
        }
        let target = file.clone();
        let save = move || std::fs::write(&target, saved).unwrap();

        let (shown, ok, result) = run(tool, at(&file, args), true, save).await;

        assert_eq!(shown.len(), 1, "{case}: reviews: {shown:?}");
        assert_eq!(
            read(&file),
            saved,
            "{case}: the user's save was overwritten"
        );
        assert!(!ok, "{case}: {result}");
        assert!(result.contains("changed"), "{case}: {result}");
    }
}

// ── No write skips the review ────────────────────────────────────────────────

#[tokio::test]
async fn every_write_that_changes_a_file_is_reviewed_first() {
    // Writes the preview used to take for "no change", and so let through
    // with no review at all. The reviewer rejects each one.
    let dir = project();

    // An empty file where there was none.
    let new = dir.path().join("new.txt");
    let (shown, ok, result) =
        run("write_file", at(&new, json!({"content": ""})), false, || {}).await;
    assert_eq!(
        shown,
        vec![Shown {
            path: new.display().to_string(),
            before: String::new(),
            after: String::new(),
        }],
        "{result}"
    );
    assert!(!ok && result.contains("rejected"), "{result}");
    assert!(!new.exists(), "created without review");

    // Emptying a file that is not UTF-8.
    let binary = dir.path().join("data.bin");
    let bytes = [0xff, 0xfe, b'a', 0, 0x80];
    std::fs::write(&binary, bytes).unwrap();
    let (shown, ok, result) = run(
        "write_file",
        at(&binary, json!({"content": ""})),
        false,
        || {},
    )
    .await;
    assert_eq!(shown.len(), 1, "{result}");
    assert_eq!(shown[0].before, String::from_utf8_lossy(&bytes));
    assert_eq!(shown[0].after, "");
    assert!(!ok, "{result}");
    assert_eq!(
        std::fs::read(&binary).unwrap(),
        bytes,
        "emptied without review"
    );

    // Through a link whose name the privacy policy denies (`.env`, built in)
    // to a file it allows: the write tools check the file written.
    #[cfg(unix)]
    {
        let real = dir.path().join("settings.cfg");
        std::fs::write(&real, "A=1\n").unwrap();
        let link = dir.path().join(".env");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (shown, ok, result) = run(
            "write_file",
            at(&link, json!({"content": "A=2\n"})),
            false,
            || {},
        )
        .await;
        assert_eq!(read(&real), "A=1\n", "written without review: {result}");
        assert!(!ok, "{result}");
        if let [shown] = shown.as_slice() {
            assert_eq!(shown.path, real.display().to_string());
            assert_eq!(
                (shown.before.as_str(), shown.after.as_str()),
                ("A=1\n", "A=2\n")
            );
        } else {
            assert!(shown.is_empty() && result.contains("REFUSED"), "{shown:?}");
        }
    }

    // An edit that changes nothing has nothing to review, and succeeds.
    let same = dir.path().join("same.txt");
    std::fs::write(&same, "same\n").unwrap();
    let edit = json!({"old_text": "same", "new_text": "same"});
    let (shown, ok, result) = run("edit_file", at(&same, edit), false, || {}).await;
    assert!(shown.is_empty(), "{shown:?}");
    assert!(ok, "{result}");
    assert_eq!(read(&same), "same\n");
}

#[cfg(unix)]
#[tokio::test]
async fn a_write_through_a_dangling_link_is_refused_with_the_real_reason() {
    // The link's destination does not exist, inside the workspace or out of
    // it. Nothing may be shown as "a new file" and then fail, and nothing may
    // be created through the link.
    let dir = project();
    let outside = tempfile::tempdir().unwrap();
    for dest in [
        dir.path().join("config/local.toml"),
        outside.path().join("gone.txt"),
    ] {
        let link = dir.path().join(format!(
            "link-{}",
            dest.file_name().unwrap().to_string_lossy()
        ));
        std::os::unix::fs::symlink(&dest, &link).unwrap();
        let (shown, ok, result) = run(
            "write_file",
            at(&link, json!({"content": "hello\n"})),
            true,
            || {},
        )
        .await;
        assert!(shown.is_empty(), "a diff was offered: {shown:?}");
        assert!(!ok, "{result}");
        assert!(
            result.contains("symbolic link") && result.contains("does not exist"),
            "{result}"
        );
        assert!(
            !dest.exists(),
            "created through the link: {}",
            dest.display()
        );
    }
}
