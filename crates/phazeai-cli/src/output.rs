//! Machine-readable output for single-prompt (`-p`) mode, for scripts and CI.
//!
//! `--output-format json` prints one JSON object when the run ends;
//! `stream-json` prints one JSON object per line as the run goes and the same
//! result object last. The names and the result's fields follow Claude Code's
//! `-p --output-format`, so a script written for one reads the other:
//!
//! ```json
//! {"type":"result","subtype":"success","is_error":false,"result":"…",
//!  "num_turns":3,"duration_ms":8123,"provider":"claude","model":"…",
//!  "usage":{"input_tokens":812,"cache_read_input_tokens":20480,
//!           "cache_creation_input_tokens":1024,"output_tokens":311},
//!  "total_cost_usd":0.0123,"unpriced_calls":0,"permission_denials":[]}
//! ```
//!
//! As in Claude's API, `input_tokens` counts only the prompt tokens that were
//! neither read from nor written to the provider's cache. `total_cost_usd` is
//! `null` when no call had a known price. Stream events are
//! `{"type":"system","subtype":"init",…}`, `{"type":"text","text":…}`,
//! `{"type":"tool_use","name":…}`, `{"type":"tool_result","name":…,
//! "success":…,"summary":…}`, `{"type":"usage",…}` (one per model call) and
//! `{"type":"compacted",…}`.

use phazeai_core::cost::Meter;
use phazeai_core::AgentEvent;
use serde_json::{json, Value};

/// How `-p` mode writes its answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// The answer as text on stdout, progress on stderr.
    #[default]
    Text,
    /// One JSON object with the answer, usage and cost when the run ends.
    Json,
    /// One JSON object per line as the run goes, the result last.
    StreamJson,
}

/// The first line of a stream: what the run is about to use.
pub fn init_event(provider: &str, model: &str, cwd: &str) -> Value {
    json!({
        "type": "system",
        "subtype": "init",
        "provider": provider,
        "model": model,
        "cwd": cwd,
    })
}

/// A stream line for an agent event, or `None` for events a script has no
/// use for (approval prompts, file previews, browser progress).
pub fn stream_event(event: &AgentEvent) -> Option<Value> {
    Some(match event {
        AgentEvent::TextDelta(text) => json!({"type": "text", "text": text}),
        AgentEvent::ToolStart { name } => json!({"type": "tool_use", "name": name}),
        AgentEvent::ToolResult {
            name,
            success,
            summary,
        } => json!({
            "type": "tool_result",
            "name": name,
            "success": success,
            "summary": summary,
        }),
        AgentEvent::CallUsage(u) => json!({
            "type": "usage",
            "input_tokens": uncached(
                u64::from(u.input_tokens),
                u64::from(u.cache_read_tokens),
                u64::from(u.cache_write_tokens),
            ),
            "cache_read_input_tokens": u.cache_read_tokens,
            "cache_creation_input_tokens": u.cache_write_tokens,
            "output_tokens": u.output_tokens,
        }),
        AgentEvent::SubagentUsage { model, usage: u } => json!({
            "type": "usage",
            "subagent_model": model,
            "input_tokens": uncached(
                u64::from(u.input_tokens),
                u64::from(u.cache_read_tokens),
                u64::from(u.cache_write_tokens),
            ),
            "cache_read_input_tokens": u.cache_read_tokens,
            "cache_creation_input_tokens": u.cache_write_tokens,
            "output_tokens": u.output_tokens,
        }),
        AgentEvent::ContextCompacted {
            messages_folded,
            tokens_before,
            tokens_after,
        } => json!({
            "type": "compacted",
            "messages_folded": messages_folded,
            "tokens_before": tokens_before,
            "tokens_after": tokens_after,
        }),
        AgentEvent::McpReconnected { servers } => {
            json!({"type": "mcp_reconnected", "servers": servers})
        }
        _ => return None,
    })
}

/// Prompt tokens that were neither read from nor written to the cache.
fn uncached(input: u64, cache_read: u64, cache_write: u64) -> u64 {
    input.saturating_sub(cache_read.saturating_add(cache_write))
}

/// How the run ended, for [`result_object`].
pub struct RunOutcome<'a> {
    /// The answer, or what went wrong.
    pub result: Result<&'a str, &'a str>,
    pub num_turns: usize,
    pub duration_ms: u128,
    pub provider: &'a str,
    pub model: &'a str,
    /// Every model call of the run, helpers included.
    pub meter: &'a Meter,
    /// Calls refused because nobody could be asked: (call, the `--allow`
    /// flag that would let it run).
    pub denials: &'a [(String, &'static str)],
}

/// The object `json` prints and `stream-json` ends with.
pub fn result_object(run: &RunOutcome) -> Value {
    let m = run.meter;
    let (subtype, is_error, text, error) = match run.result {
        Ok(text) => ("success", false, text, None),
        Err(e) => ("error", true, "", Some(e)),
    };
    let priced = m.cost > 0.0 || m.unpriced_calls == 0;
    let mut out = json!({
        "type": "result",
        "subtype": subtype,
        "is_error": is_error,
        "result": text,
        "num_turns": run.num_turns,
        "duration_ms": run.duration_ms as u64,
        "provider": run.provider,
        "model": run.model,
        "usage": {
            "input_tokens": uncached(m.input, m.cache_read, m.cache_write),
            "cache_read_input_tokens": m.cache_read,
            "cache_creation_input_tokens": m.cache_write,
            "output_tokens": m.output,
        },
        "total_cost_usd": if priced { json!(m.cost) } else { Value::Null },
        "unpriced_calls": m.unpriced_calls,
        "permission_denials": run
            .denials
            .iter()
            .map(|(call, needs)| json!({"call": call, "needs": needs}))
            .collect::<Vec<_>>(),
    });
    if let Some(e) = error {
        out["error"] = json!(e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use phazeai_core::llm::{ProviderId, Usage};

    fn usage(input: u32, read: u32, write: u32, output: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_write_tokens: write,
        }
    }

    #[test]
    fn the_result_counts_tokens_as_claudes_api_does_and_prices_them() {
        let mut meter = Meter::default();
        // Haiku 4.5: $1/M in, $5/M out, cache reads $0.10/M, writes $1.25/M.
        meter.add(
            Some((&ProviderId::Claude, "claude-haiku-4-5-20251001")),
            &usage(12_000, 10_000, 1_000, 200),
        );
        let denials = [("bash: cargo build".to_string(), "--allow all")];
        let run = RunOutcome {
            result: Ok("Done."),
            num_turns: 2,
            duration_ms: 1500,
            provider: "claude",
            model: "claude-haiku-4-5-20251001",
            meter: &meter,
            denials: &denials,
        };
        let out = result_object(&run);
        assert_eq!(out["type"], "result");
        assert_eq!(out["subtype"], "success");
        assert_eq!(out["is_error"], false);
        assert_eq!(out["result"], "Done.");
        assert_eq!(out["num_turns"], 2);
        assert_eq!(out["usage"]["input_tokens"], 1_000);
        assert_eq!(out["usage"]["cache_read_input_tokens"], 10_000);
        assert_eq!(out["usage"]["cache_creation_input_tokens"], 1_000);
        assert_eq!(out["usage"]["output_tokens"], 200);
        let cost = out["total_cost_usd"].as_f64().unwrap();
        assert!((cost - meter.cost).abs() < 1e-12 && cost > 0.0);
        assert_eq!(out["permission_denials"][0]["needs"], "--allow all");
        assert!(out.get("error").is_none());
    }

    #[test]
    fn a_failed_run_says_so_and_an_unknown_price_is_null() {
        let mut meter = Meter::default();
        meter.add(None, &usage(100, 0, 0, 10));
        let run = RunOutcome {
            result: Err("the key was refused"),
            num_turns: 1,
            duration_ms: 10,
            provider: "custom",
            model: "x",
            meter: &meter,
            denials: &[],
        };
        let out = result_object(&run);
        assert_eq!(out["subtype"], "error");
        assert_eq!(out["is_error"], true);
        assert_eq!(out["error"], "the key was refused");
        assert!(out["total_cost_usd"].is_null());
        assert_eq!(out["unpriced_calls"], 1);
    }

    #[test]
    fn stream_lines_for_text_tools_and_usage() {
        assert_eq!(
            stream_event(&AgentEvent::TextDelta("hi".into())).unwrap(),
            json!({"type": "text", "text": "hi"})
        );
        let line = stream_event(&AgentEvent::ToolResult {
            name: "bash".into(),
            success: false,
            summary: "exit 1".into(),
        })
        .unwrap();
        assert_eq!(line["type"], "tool_result");
        assert_eq!(line["success"], false);
        let line = stream_event(&AgentEvent::CallUsage(usage(500, 300, 100, 20))).unwrap();
        assert_eq!(line["input_tokens"], 100);
        assert!(stream_event(&AgentEvent::Thinking { iteration: 1 }).is_none());
    }
}
