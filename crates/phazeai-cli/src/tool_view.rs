//! Presentation helpers for tool calls: one-line call labels, result digests,
//! approval previews (commands and file diffs) and the per-session
//! "always allow" policy.

use std::collections::HashSet;

use phazeai_core::{Message, Role};
use serde_json::Value;

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// Short, human description of a call's arguments, e.g. `cargo test` for
/// bash or `src/main.rs` for read_file.
pub fn call_label(name: &str, params: &Value) -> String {
    let path = || str_param(params, "path").unwrap_or("").to_string();
    let label = match name {
        "bash" => str_param(params, "command")
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .to_string(),
        "read_file" => {
            let mut s = path();
            if let Some(off) = params.get("offset").and_then(Value::as_u64) {
                s.push_str(&format!(":{off}"));
            }
            s
        }
        "write_file" | "edit_file" | "list_files" | "delete_path" | "create_directory" | "open"
        | "diagnostics" => path(),
        "grep" => {
            let pat = str_param(params, "pattern").unwrap_or("");
            match str_param(params, "path") {
                Some(p) if !p.is_empty() && p != "." => format!("\"{pat}\" in {p}"),
                _ => format!("\"{pat}\""),
            }
        }
        "glob" | "find_path" => str_param(params, "pattern")
            .or_else(|| str_param(params, "query"))
            .unwrap_or("")
            .to_string(),
        "fetch" | "browse" | "download" => str_param(params, "url").unwrap_or("").to_string(),
        "web_search" => str_param(params, "query").unwrap_or("").to_string(),
        "move_path" | "copy_path" => format!(
            "{} → {}",
            str_param(params, "source").unwrap_or("?"),
            str_param(params, "destination").unwrap_or("?")
        ),
        "memory" => str_param(params, "action")
            .map(|a| match str_param(params, "key") {
                Some(k) => format!("{a} {k}"),
                None => a.to_string(),
            })
            .unwrap_or_default(),
        _ => String::new(),
    };
    let label = crate::text::sanitize(&label);
    if !label.is_empty() {
        return label;
    }
    // Unknown tool: compact `key=value` list of scalar params.
    match params {
        Value::Object(map) => map
            .iter()
            .filter_map(|(k, v)| match v {
                Value::String(s) => Some(format!("{k}={}", s.lines().next().unwrap_or(""))),
                Value::Number(n) => Some(format!("{k}={n}")),
                Value::Bool(b) => Some(format!("{k}={b}")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A condensed view of a tool result.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Digest {
    /// One-line summary, e.g. "exit 0" or "12 matches".
    pub headline: String,
    /// Detail lines (shown truncated when the card is collapsed).
    pub body: Vec<String>,
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn lines_of(s: &str) -> Vec<String> {
    crate::text::sanitize(s)
        .lines()
        .map(str::to_string)
        .collect()
}

/// Turn the `summary` from `AgentEvent::ToolResult` into a digest. Successful
/// results are usually pretty-printed JSON; failures are plain text.
pub fn digest(name: &str, success: bool, summary: &str) -> Digest {
    if !success {
        let mut lines = lines_of(summary.trim());
        let headline = if lines.is_empty() {
            "failed".to_string()
        } else {
            lines.remove(0)
        };
        return Digest {
            headline,
            body: lines,
        };
    }

    let json: Option<Value> = serde_json::from_str(summary).ok();
    let Some(Value::Object(obj)) = json else {
        // Plain text (or a summary truncated mid-JSON by core).
        return Digest {
            headline: String::new(),
            body: lines_of(summary.trim_end()),
        };
    };
    let get_u64 = |k: &str| obj.get(k).and_then(Value::as_u64);
    let get_str = |k: &str| obj.get(k).and_then(Value::as_str);

    match name {
        "bash" => {
            let code = obj.get("exit_code").and_then(Value::as_i64);
            let headline = match code {
                Some(c) => format!("exit {c}"),
                None => "terminated".to_string(),
            };
            let mut body = lines_of(get_str("stdout").unwrap_or("").trim_end());
            let stderr = get_str("stderr").unwrap_or("").trim_end();
            if !stderr.is_empty() {
                body.extend(lines_of(stderr).into_iter().map(|l| format!("stderr: {l}")));
            }
            Digest { headline, body }
        }
        "read_file" => {
            let shown = get_u64("lines_shown").unwrap_or(0);
            let headline = match get_u64("total_lines") {
                Some(total) if total != shown => format!("read {shown} of {total} lines"),
                _ => format!("read {}", plural(shown, "line", "lines")),
            };
            Digest {
                headline,
                body: lines_of(get_str("content").unwrap_or("")),
            }
        }
        "write_file" => Digest {
            headline: format!(
                "wrote {}",
                plural(get_u64("bytes_written").unwrap_or(0), "byte", "bytes")
            ),
            body: Vec::new(),
        },
        "edit_file" => Digest {
            headline: format!(
                "{} made",
                plural(
                    get_u64("replacements").unwrap_or(1),
                    "replacement",
                    "replacements"
                )
            ),
            body: Vec::new(),
        },
        "grep" | "glob" => {
            let count = get_u64("count").unwrap_or(0);
            let headline = if name == "grep" {
                plural(count, "match", "matches")
            } else {
                plural(count, "file", "files")
            };
            let body = obj
                .get("matches")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .map(|m| match m {
                            Value::String(s) => s.clone(),
                            Value::Object(o) => format!(
                                "{}:{}: {}",
                                o.get("file").and_then(Value::as_str).unwrap_or("?"),
                                o.get("line").and_then(Value::as_u64).unwrap_or(0),
                                o.get("content")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .trim()
                            ),
                            other => other.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Digest { headline, body }
        }
        _ => {
            // Generic: prefer a human text field, else show the JSON.
            for key in ["content", "output", "result", "text", "message"] {
                if let Some(s) = get_str(key) {
                    return Digest {
                        headline: String::new(),
                        body: lines_of(s.trim_end()),
                    };
                }
            }
            Digest {
                headline: String::new(),
                body: lines_of(summary.trim_end()),
            }
        }
    }
}

/// Session-scoped "always allow" rules chosen from the approval prompt.
///
/// bash is scoped to the exact command (approving `cargo test` must not
/// approve `rm -rf ~`); every other tool is scoped to the tool name.
#[derive(Debug, Default)]
pub struct ApprovalPolicy {
    rules: HashSet<String>,
}

impl ApprovalPolicy {
    pub fn rule_key(name: &str, params: &Value) -> String {
        match name {
            "bash" => format!("bash:{}", str_param(params, "command").unwrap_or("").trim()),
            _ => name.to_string(),
        }
    }

    /// Human description of what "always" would allow.
    pub fn describe(name: &str, params: &Value) -> String {
        match name {
            "bash" => format!(
                "`{}`",
                crate::text::truncate_to_width(
                    str_param(params, "command").unwrap_or("").trim(),
                    40
                )
            ),
            _ => format!("all {name} calls"),
        }
    }

    pub fn allows(&self, name: &str, params: &Value) -> bool {
        self.rules.contains(&Self::rule_key(name, params))
    }

    pub fn allow(&mut self, name: &str, params: &Value) {
        self.rules.insert(Self::rule_key(name, params));
    }

    pub fn clear(&mut self) {
        self.rules.clear();
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// One line of an approval preview.
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewLine {
    /// A shell command line.
    Command(String),
    Add(String),
    Remove(String),
    Context(String),
    /// `@@ … @@` style separator between hunks.
    Hunk(String),
    /// Informational text (e.g. "new file", errors).
    Note(String),
}

/// Build the preview shown in the approval prompt.
pub fn approval_preview(name: &str, params: &Value) -> Vec<PreviewLine> {
    match name {
        "bash" => str_param(params, "command")
            .unwrap_or("")
            .lines()
            .map(|l| PreviewLine::Command(l.to_string()))
            .collect(),
        "edit_file" | "write_file" => file_change_preview(name, params),
        _ => {
            let label = call_label(name, params);
            if label.is_empty() {
                Vec::new()
            } else {
                vec![PreviewLine::Note(label)]
            }
        }
    }
}

fn file_change_preview(name: &str, params: &Value) -> Vec<PreviewLine> {
    let Some(path) = str_param(params, "path") else {
        return vec![PreviewLine::Note("missing `path` parameter".into())];
    };
    let resolved = match phazeai_core::tools::sandbox::resolve_within_workspace(name, path) {
        Ok(p) => p,
        Err(e) => return vec![PreviewLine::Note(format!("will be refused: {e}"))],
    };
    let existing = std::fs::read_to_string(&resolved).ok();
    let (before, after) = if name == "write_file" {
        let after = str_param(params, "content").unwrap_or("").to_string();
        (existing.clone().unwrap_or_default(), after)
    } else {
        let Some(before) = existing.clone() else {
            return vec![PreviewLine::Note(format!("{path} does not exist"))];
        };
        let old = str_param(params, "old_text").unwrap_or("");
        let new = str_param(params, "new_text").unwrap_or("");
        let replace_all = params
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if old.is_empty() || !before.contains(old) {
            return vec![PreviewLine::Note(
                "old_text not found in file — the edit will fail".into(),
            )];
        }
        let after = if replace_all {
            before.replace(old, new)
        } else {
            before.replacen(old, new, 1)
        };
        (before, after)
    };

    let mut out = Vec::new();
    if existing.is_none() {
        out.push(PreviewLine::Note(format!(
            "new file, {} lines",
            after.lines().count()
        )));
    }
    out.extend(unified_diff(&before, &after, 2));
    if out.is_empty() {
        out.push(PreviewLine::Note("no changes".into()));
    }
    out
}

/// Line diff between `before` and `after` with `context` lines around each
/// change.
pub fn unified_diff(before: &str, after: &str, context: usize) -> Vec<PreviewLine> {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::from_lines(before, after);
    let mut out = Vec::new();
    for (i, group) in diff.grouped_ops(context).iter().enumerate() {
        if let Some(first) = group.first() {
            let old_start = first.old_range().start + 1;
            let new_start = first.new_range().start + 1;
            if i > 0 || old_start > 1 {
                out.push(PreviewLine::Hunk(format!(
                    "@@ -{old_start} +{new_start} @@"
                )));
            }
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches(['\n', '\r']).to_string();
                out.push(match change.tag() {
                    ChangeTag::Delete => PreviewLine::Remove(text),
                    ChangeTag::Insert => PreviewLine::Add(text),
                    ChangeTag::Equal => PreviewLine::Context(text),
                });
            }
        }
    }
    out
}

/// IDs of tool calls in the last assistant turn that never got a result —
/// left behind when a run is interrupted mid-turn. Providers reject a history
/// containing such orphans, so they must be answered before the next request.
pub fn unanswered_tool_calls(messages: &[Message]) -> Vec<String> {
    let Some(idx) = messages
        .iter()
        .rposition(|m| m.role == Role::Assistant && m.tool_calls.is_some())
    else {
        return Vec::new();
    };
    let answered: HashSet<&str> = messages[idx + 1..]
        .iter()
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();
    messages[idx]
        .tool_calls
        .as_ref()
        .map(|calls| {
            calls
                .iter()
                .filter(|c| !answered.contains(c.id.as_str()))
                .map(|c| c.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labels_are_concise() {
        assert_eq!(
            call_label("bash", &json!({"command": "cargo test\necho hi"})),
            "cargo test"
        );
        assert_eq!(
            call_label("read_file", &json!({"path": "src/a.rs", "offset": 10})),
            "src/a.rs:10"
        );
        assert_eq!(
            call_label("grep", &json!({"pattern": "fn main", "path": "src"})),
            "\"fn main\" in src"
        );
        assert_eq!(
            call_label("move_path", &json!({"source": "a", "destination": "b"})),
            "a → b"
        );
        assert_eq!(
            call_label("mystery", &json!({"x": "1", "n": 2, "deep": {"a": 1}})),
            "n=2 x=1"
        );
    }

    #[test]
    fn bash_digest() {
        let s = serde_json::to_string_pretty(&json!({
            "stdout": "ok\nall good\n", "stderr": "warn\n", "exit_code": 0, "success": true
        }))
        .unwrap();
        let d = digest("bash", true, &s);
        assert_eq!(d.headline, "exit 0");
        assert_eq!(d.body, vec!["ok", "all good", "stderr: warn"]);
    }

    #[test]
    fn file_and_search_digests() {
        let d = digest(
            "read_file",
            true,
            &json!({"content": "a\nb", "path": "x", "total_lines": 10, "lines_shown": 2})
                .to_string(),
        );
        assert_eq!(d.headline, "read 2 of 10 lines");
        let d = digest(
            "grep",
            true,
            &json!({"matches": [{"file": "a.rs", "line": 3, "content": "  fn x()"}], "count": 1})
                .to_string(),
        );
        assert_eq!(d.headline, "1 match");
        assert_eq!(d.body, vec!["a.rs:3: fn x()"]);
        let d = digest("edit_file", true, &json!({"replacements": 2}).to_string());
        assert_eq!(d.headline, "2 replacements made");
    }

    #[test]
    fn failures_and_plain_text() {
        let d = digest("bash", false, "Tool execution denied by user");
        assert_eq!(d.headline, "Tool execution denied by user");
        let d = digest("x", true, "{\"truncated");
        assert_eq!(d.body, vec!["{\"truncated"]);
    }

    #[test]
    fn approval_policy_scopes() {
        let mut p = ApprovalPolicy::default();
        let test = json!({"command": "cargo test"});
        let rm = json!({"command": "rm -rf /"});
        p.allow("bash", &test);
        assert!(p.allows("bash", &test));
        assert!(!p.allows("bash", &rm));
        p.allow("edit_file", &json!({"path": "a"}));
        assert!(p.allows("edit_file", &json!({"path": "b"})));
        assert_eq!(p.len(), 2);
        p.clear();
        assert!(p.is_empty());
    }

    #[test]
    fn diff_shows_changes_with_context() {
        let before = "a\nb\nc\nd\ne\nf\ng\n";
        let after = "a\nb\nc\nD\ne\nf\ng\n";
        let d = unified_diff(before, after, 1);
        assert_eq!(
            d,
            vec![
                PreviewLine::Hunk("@@ -3 +3 @@".into()),
                PreviewLine::Context("c".into()),
                PreviewLine::Remove("d".into()),
                PreviewLine::Add("D".into()),
                PreviewLine::Context("e".into()),
            ]
        );
        assert!(unified_diff("same\n", "same\n", 2).is_empty());
    }

    #[test]
    fn bash_preview_is_the_command() {
        assert_eq!(
            approval_preview("bash", &json!({"command": "ls -la"})),
            vec![PreviewLine::Command("ls -la".into())]
        );
    }

    #[test]
    fn edit_preview_reads_the_file() {
        let dir = std::env::temp_dir().join(format!("phazeai-prev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let path = file.display().to_string();
        let lines = approval_preview(
            "edit_file",
            &json!({"path": path, "old_text": "two", "new_text": "2"}),
        );
        assert!(lines.contains(&PreviewLine::Remove("two".into())));
        assert!(lines.contains(&PreviewLine::Add("2".into())));
        let lines = approval_preview(
            "edit_file",
            &json!({"path": path, "old_text": "zzz", "new_text": "2"}),
        );
        assert!(matches!(&lines[0], PreviewLine::Note(n) if n.contains("not found")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn finds_unanswered_tool_calls() {
        use phazeai_core::llm::{FunctionCall, ToolCall};
        let call = |id: &str| ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
        };
        let msgs = vec![
            Message::user("hi"),
            Message::assistant_with_tools("", vec![call("a"), call("b")]),
            Message::tool_result("a", "done"),
        ];
        assert_eq!(unanswered_tool_calls(&msgs), vec!["b".to_string()]);
        assert!(unanswered_tool_calls(&[Message::user("x")]).is_empty());
    }
}
