//! The agent reports each model call's usage as it lands, so the token and
//! cost meters count a run as it goes, and count a run that fails part way.

use std::collections::VecDeque;
use std::sync::Mutex;

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use phazeai_core::cost::Meter;
use phazeai_core::llm::Usage;
use phazeai_core::{
    Agent, AgentEvent, LlmClient, LlmResponse, Message, PhazeError, ProviderId, StreamEvent,
    ToolDefinition,
};
use serde_json::json;

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
        let reply = self.0.lock().unwrap().pop_front().unwrap();
        let (tx, rx) = unbounded();
        for event in reply {
            tx.unbounded_send(event).unwrap();
        }
        Ok(rx)
    }
}

fn usage(input: u32, output: u32, read: u32) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: read,
        cache_write_tokens: 0,
    }
}

/// A reply that lists the current directory, then reports `u`.
fn tool_call(u: Usage) -> Vec<StreamEvent> {
    vec![
        StreamEvent::ToolCallStart {
            id: "c1".into(),
            name: "list_files".into(),
        },
        StreamEvent::ToolCallDelta {
            id: "c1".into(),
            arguments_delta: json!({"path": "."}).to_string(),
        },
        StreamEvent::ToolCallEnd { id: "c1".into() },
        StreamEvent::Usage(u),
        StreamEvent::Done,
    ]
}

async fn run(script: Vec<Vec<StreamEvent>>) -> (Vec<AgentEvent>, bool) {
    let agent = Agent::new(Box::new(Script(Mutex::new(script.into()))))
        .with_approval(Box::new(|_, _| Box::pin(async { true })));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ok = agent.run_with_events("go", tx).await.is_ok();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    (events, ok)
}

fn calls(events: &[AgentEvent]) -> Vec<Usage> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::CallUsage(u) => Some(u.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn each_call_is_reported_and_they_add_up_to_the_run() {
    let (events, ok) = run(vec![
        tool_call(usage(3_000, 40, 0)),
        vec![
            StreamEvent::TextDelta("done".into()),
            StreamEvent::Usage(usage(3_200, 10, 2_900)),
            StreamEvent::Done,
        ],
    ])
    .await;
    assert!(ok);
    let per_call = calls(&events);
    assert_eq!(per_call, [usage(3_000, 40, 0), usage(3_200, 10, 2_900)]);
    // The first call is reported before its tool runs.
    let first_usage = events
        .iter()
        .position(|e| matches!(e, AgentEvent::CallUsage(_)))
        .unwrap();
    let first_tool = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ToolStart { .. }))
        .unwrap();
    assert!(first_usage < first_tool);
    let total = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                ..
            } => Some((*input_tokens, *output_tokens, *cache_read_tokens)),
            _ => None,
        })
        .unwrap();
    assert_eq!(total, (6_200, 50, 2_900));

    let mut meter = Meter::default();
    for u in &per_call {
        meter.add(Some((&ProviderId::Claude, "claude-sonnet-5-5")), u);
    }
    assert_eq!(meter.cached_percent(), 46);
    let expected = (3_300.0 * 2.0 + 2_900.0 * 0.10 + 50.0 * 10.0) / 1e6;
    assert!((meter.cost - expected).abs() < 1e-12);
}

#[tokio::test]
async fn a_run_that_fails_still_reports_the_calls_it_paid_for() {
    let (events, ok) = run(vec![
        tool_call(usage(5_000, 30, 0)),
        vec![StreamEvent::Error("overloaded".into())],
    ])
    .await;
    assert!(!ok);
    assert_eq!(calls(&events), [usage(5_000, 30, 0)]);
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::TokenUsage { .. })));
}
