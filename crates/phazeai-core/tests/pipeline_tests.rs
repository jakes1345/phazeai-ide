//! End-to-end test of the multi-agent pipeline with scripted models: the
//! Coder must really edit files, a failing check must trigger a fix round,
//! and the Reviewer must see this run's diff.

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::agent::{
    CheckCommand, Pipeline, PipelineConfig, PipelineEvent, ReviewVerdict, Stage,
};
use phazeai_core::{LlmClient, LlmResponse, Message, PhazeError, StreamEvent, ToolDefinition};
use std::sync::{Arc, Mutex};

/// Replays one scripted stream per LLM call, in order, and records the
/// prompts it was sent.
struct ScriptedLlm {
    turns: Mutex<std::collections::VecDeque<Vec<StreamEvent>>>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl ScriptedLlm {
    fn new(turns: Vec<Vec<StreamEvent>>) -> (Box<Self>, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Box::new(Self {
                turns: Mutex::new(turns.into()),
                seen: seen.clone(),
            }),
            seen,
        )
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedLlm {
    async fn chat(&self, _: &[Message], _: &[ToolDefinition]) -> Result<LlmResponse, PhazeError> {
        unreachable!("the agent loop streams")
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<UnboundedReceiver<StreamEvent>, PhazeError> {
        if let Some(last) = messages.last() {
            self.seen.lock().unwrap().push(last.content.clone());
        }
        let events = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![StreamEvent::TextDelta("done".into()), StreamEvent::Done]);
        let (tx, rx) = unbounded();
        for e in events {
            tx.unbounded_send(e).unwrap();
        }
        Ok(rx)
    }
}

fn text(s: &str) -> Vec<StreamEvent> {
    vec![StreamEvent::TextDelta(s.into()), StreamEvent::Done]
}

fn write_file(id: &str, path: &std::path::Path, content: &str) -> Vec<StreamEvent> {
    let args = serde_json::json!({ "path": path, "content": content }).to_string();
    vec![
        StreamEvent::ToolCallStart {
            id: id.into(),
            name: "write_file".into(),
        },
        StreamEvent::ToolCallDelta {
            id: id.into(),
            arguments_delta: args,
        },
        StreamEvent::ToolCallEnd { id: id.into() },
        StreamEvent::Done,
    ]
}

fn git(root: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed");
}

#[tokio::test]
async fn pipeline_edits_files_fixes_failed_check_and_reviews_diff() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@t"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README"), "x\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "init"]);
    let target = root.join("status.txt");

    let (planner, _) = ScriptedLlm::new(vec![text("1. Create status.txt containing 'fixed'.")]);
    let (coder, coder_prompts) = ScriptedLlm::new(vec![
        // First implementation is wrong on purpose...
        write_file("c1", &target, "broken\n"),
        text("Created status.txt."),
        // ...then fixed after the check fails.
        write_file("c2", &target, "fixed\n"),
        text("Fixed status.txt."),
    ]);
    let (reviewer, reviewer_prompts) = ScriptedLlm::new(vec![text("APPROVED\nLooks right.")]);

    let mut config = PipelineConfig::new(&root);
    config.check_command = Some(CheckCommand {
        program: "grep".into(),
        args: vec!["-q".into(), "fixed".into(), "status.txt".into()],
    });

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let outcome = Pipeline::new(planner, coder, reviewer, config)
        .run("Make status.txt say fixed", tx)
        .await
        .expect("pipeline run");

    // The Coder's tool calls really wrote the file, and the fix stuck.
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "fixed\n");
    assert_eq!(outcome.check_passed, Some(true));
    assert_eq!(outcome.fix_rounds, 1);
    assert_eq!(outcome.verdict, ReviewVerdict::Approved);
    assert_eq!(outcome.changed_files, vec!["status.txt".to_string()]);

    // The Coder was shown the plan, then the failing check.
    let coder_prompts = coder_prompts.lock().unwrap();
    assert!(coder_prompts[0].contains("Create status.txt"));
    assert!(coder_prompts
        .iter()
        .any(|p| p.contains("fails after your changes")));

    // The Reviewer was told the check passes and which file changed.
    let review_prompt = &reviewer_prompts.lock().unwrap()[0];
    assert!(review_prompt.contains("passes"), "{review_prompt}");
    assert!(review_prompt.contains("status.txt"), "{review_prompt}");

    // Stages ran in order, with one failed and one passed check.
    let mut stages = Vec::new();
    let mut checks = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            PipelineEvent::StageStarted(s) if stages.last() != Some(&s) => stages.push(s),
            PipelineEvent::CheckResult { passed, .. } => checks.push(passed),
            _ => {}
        }
    }
    assert_eq!(checks, vec![false, true]);
    assert_eq!(
        stages,
        vec![
            Stage::Plan,
            Stage::Code,
            Stage::Verify,
            Stage::Code,
            Stage::Verify,
            Stage::Review
        ]
    );
}

#[tokio::test]
async fn reviewer_change_request_gets_one_fix_round() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let target = root.join("a.txt");

    let (planner, _) = ScriptedLlm::new(vec![text("plan")]);
    let (coder, _) = ScriptedLlm::new(vec![
        write_file("c1", &target, "v1\n"),
        text("v1"),
        write_file("c2", &target, "v2\n"),
        text("v2"),
    ]);
    let (reviewer, _) = ScriptedLlm::new(vec![
        text("CHANGES REQUESTED\n- a.txt must say v2"),
        text("APPROVED"),
    ]);

    let mut config = PipelineConfig::new(&root);
    config.skip_check = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let outcome = Pipeline::new(planner, coder, reviewer, config)
        .run("write a.txt", tx)
        .await
        .unwrap();

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "v2\n");
    assert_eq!(outcome.verdict, ReviewVerdict::Approved);
    assert_eq!(outcome.check_passed, None);
}

#[tokio::test]
async fn cancelled_pipeline_stops_before_coding() {
    let dir = tempfile::tempdir().unwrap();
    let (planner, _) = ScriptedLlm::new(vec![text("plan")]);
    let (coder, coder_prompts) = ScriptedLlm::new(vec![]);
    let (reviewer, _) = ScriptedLlm::new(vec![]);
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let result = Pipeline::new(planner, coder, reviewer, PipelineConfig::new(dir.path()))
        .with_cancel_token(cancel)
        .run("anything", tx)
        .await;
    assert!(result.is_err());
    assert!(coder_prompts.lock().unwrap().is_empty());
}
