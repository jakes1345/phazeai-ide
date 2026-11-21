//! What compaction keeps word for word, apart from the model's summary: the
//! agent's checklist as it last stood and the files it changed in the part
//! that was summarised.
//!
//! A summary is written by the model and can drop either one, and both are
//! what a long task most needs after compaction: without the checklist the
//! model loses its place in the plan, and without the file list it no longer
//! knows what it already changed. So [`super::ConversationHistory::compact`]
//! appends them to the summary under [`HEADING`], taken from the tool calls
//! themselves. They live in the summary text, so every place that carries a
//! summary over (the IDE rebuilds the history from it each turn) carries them
//! too; the next compaction reads them back from there and merges them with
//! what it folds.

use crate::llm::Message;

/// Heading of the kept section at the end of a summary.
pub const HEADING: &str = "### Kept across compaction";
const CHECKLIST: &str = "Your checklist as you last updated it:";
const FILES: &str = "Files you changed in the summarised part, newest first:";

/// The most file paths kept; older ones drop off.
pub const MAX_FILES: usize = 20;
/// A kept checklist longer than this is cut in the middle (the `todo` tool
/// allows 30 items of 200 bytes; a list that long is rare).
const MAX_CHECKLIST_BYTES: usize = 3000;

/// Tools that change the file at their `path` argument.
const WRITES_PATH: &[&str] = &["write_file", "edit_file", "delete_path", "create_directory"];
/// Tools that change the file at their `destination` argument.
const WRITES_DESTINATION: &[&str] = &["move_path", "copy_path"];

/// How a tool result that did not do its work starts (see the agent loop).
const FAILED: &[&str] = &[
    "Error",
    "Failed to parse",
    "Tool execution denied",
    "Interrupted by the user",
    "Cancelled by the user",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Carried {
    /// The `todo` tool's last list, as it returned it.
    pub checklist: Option<String>,
    /// Paths changed, newest first, without repeats.
    pub files: Vec<String>,
}

impl Carried {
    /// A summary split into the model's text and the kept section after it.
    /// A summary without one keeps nothing.
    pub fn split(summary: &str) -> (&str, Carried) {
        let Some(at) = summary.find(HEADING) else {
            return (summary.trim(), Carried::default());
        };
        let text = summary[..at].trim();
        let kept = &summary[at + HEADING.len()..];
        let checklist_at = kept.find(CHECKLIST);
        let files_at = kept.find(FILES);
        let checklist = checklist_at.map(|start| {
            let body = &kept[start + CHECKLIST.len()..];
            let end = files_at
                .filter(|&f| f > start)
                .map_or(body.len(), |f| f - start - CHECKLIST.len());
            body[..end].trim().to_string()
        });
        let files = files_at
            .map(|start| {
                kept[start + FILES.len()..]
                    .lines()
                    .filter_map(|l| l.trim().strip_prefix("- "))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        (
            text,
            Carried {
                checklist: checklist.filter(|c| !c.is_empty()),
                files,
            },
        )
    }

    /// What to keep after folding `folded` into a summary whose earlier kept
    /// section was `previous`, with `kept` the messages that stay.
    ///
    /// The checklist is the newest one the folded part holds, or else the
    /// earlier one; none when a list the model can still see in `kept` has
    /// replaced it, or when its last update emptied it. Files are those the
    /// folded part changed, newest first, then the earlier ones.
    pub fn after_folding(previous: Carried, folded: &[Message], kept: &[Message]) -> Carried {
        let checklist = if latest_checklist(kept).is_some() {
            None
        } else {
            match latest_checklist(folded) {
                Some(list) => list,
                None => previous.checklist,
            }
            .map(|list| crate::text::cut_middle(&list, MAX_CHECKLIST_BYTES).into_owned())
        };
        let mut files: Vec<String> = Vec::new();
        for path in changed_files(folded)
            .into_iter()
            .rev()
            .chain(previous.files)
        {
            if !files.contains(&path) {
                files.push(path);
            }
        }
        files.truncate(MAX_FILES);
        Carried { checklist, files }
    }

    /// The kept section, or `None` when there is nothing to keep.
    pub fn render(&self) -> Option<String> {
        if self.checklist.is_none() && self.files.is_empty() {
            return None;
        }
        let mut out = String::from(HEADING);
        if let Some(list) = &self.checklist {
            out.push_str(&format!("\n{CHECKLIST}\n{list}"));
        }
        if !self.files.is_empty() {
            out.push_str(&format!("\n{FILES}"));
            for f in &self.files {
                out.push_str(&format!("\n- {f}"));
            }
        }
        Some(out)
    }
}

/// `text` with `carried` appended under [`HEADING`].
pub fn join(text: &str, carried: &Carried) -> String {
    match carried.render() {
        Some(kept) if text.is_empty() => kept,
        Some(kept) => format!("{text}\n\n{kept}"),
        None => text.to_string(),
    }
}

/// Each tool result in `messages` with the call it answers (the nearest
/// earlier call with its id; ids can repeat across replies).
fn answered(messages: &[Message]) -> Vec<(&crate::llm::ToolCall, &str)> {
    let mut out = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let Some(id) = m.tool_call_id.as_deref() else {
            continue;
        };
        let call = messages[..i]
            .iter()
            .rev()
            .find_map(|e| e.tool_calls.as_ref()?.iter().find(|c| c.id == id));
        if let Some(call) = call {
            out.push((call, m.content.as_str()));
        }
    }
    out
}

fn succeeded(result: &str) -> bool {
    !FAILED.iter().any(|p| result.starts_with(p))
}

/// The newest list the `todo` tool returned in `messages`: `Some(None)` when
/// that update emptied it, `None` when there is none.
fn latest_checklist(messages: &[Message]) -> Option<Option<String>> {
    answered(messages)
        .into_iter()
        .rev()
        .filter(|(call, result)| call.function.name == "todo" && succeeded(result))
        .find_map(|(_, result)| {
            let result = result.trim();
            if result == "The list is empty." {
                return Some(None);
            }
            let last = result.lines().last()?;
            let done = last.split_once(" of ")?;
            (done.0.parse::<usize>().is_ok() && done.1.ends_with(" done"))
                .then(|| Some(result.to_string()))
        })
}

/// Paths the folded calls changed, in the order they were changed.
fn changed_files(messages: &[Message]) -> Vec<String> {
    answered(messages)
        .into_iter()
        .filter(|(_, result)| succeeded(result))
        .filter_map(|(call, _)| {
            let name = call.function.name.as_str();
            let key = if WRITES_PATH.contains(&name) {
                "path"
            } else if WRITES_DESTINATION.contains(&name) {
                "destination"
            } else {
                return None;
            };
            let args: serde_json::Value = serde_json::from_str(&call.function.arguments).ok()?;
            let path = args.get(key)?.as_str()?.trim();
            (!path.is_empty() && !path.contains('\n')).then(|| path.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{FunctionCall, ToolCall};

    fn call(id: &str, tool: &str, args: serde_json::Value) -> Message {
        Message::assistant_with_tools(
            "",
            vec![ToolCall {
                id: id.to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: tool.to_string(),
                    arguments: args.to_string(),
                },
            }],
        )
    }

    fn step(id: &str, tool: &str, args: serde_json::Value, result: &str) -> [Message; 2] {
        [call(id, tool, args), Message::tool_result(id, result)]
    }

    const LIST_A: &str = "[x] read the code\n[>] fix the bug\n[ ] add a test\n1 of 3 done";
    const LIST_B: &str = "[x] read the code\n[x] fix the bug\n[>] add a test\n2 of 3 done";

    #[test]
    fn the_newest_checklist_and_the_files_written_are_kept() {
        let mut folded = vec![Message::user("fix it")];
        folded.extend(step("1", "todo", serde_json::json!({}), LIST_A));
        folded.extend(step(
            "2",
            "edit_file",
            serde_json::json!({"path": "src/a.rs"}),
            "Edited src/a.rs (1 replacement)",
        ));
        folded.extend(step(
            "3",
            "write_file",
            serde_json::json!({"path": "src/b.rs"}),
            "Wrote 10 bytes to src/b.rs",
        ));
        folded.extend(step(
            "4",
            "edit_file",
            serde_json::json!({"path": "src/c.rs"}),
            "Error: old_text not found",
        ));
        folded.extend(step(
            "5",
            "read_file",
            serde_json::json!({"path": "src/d.rs"}),
            "fn d() {}",
        ));
        folded.extend(step(
            "6",
            "move_path",
            serde_json::json!({"source": "x.rs", "destination": "y.rs"}),
            "Moved",
        ));
        folded.extend(step("7", "todo", serde_json::json!({}), LIST_B));
        folded.extend(step(
            "8",
            "edit_file",
            serde_json::json!({"path": "src/a.rs"}),
            "Edited src/a.rs (1 replacement)",
        ));
        let kept = vec![Message::user("and now?")];

        let carried = Carried::after_folding(Carried::default(), &folded, &kept);
        assert_eq!(carried.checklist.as_deref(), Some(LIST_B));
        // Newest first, once each; the failed edit and the read are not changes.
        assert_eq!(carried.files, vec!["src/a.rs", "y.rs", "src/b.rs"]);
    }

    #[test]
    fn a_list_still_in_view_or_emptied_is_not_kept() {
        let mut folded = vec![Message::user("go")];
        folded.extend(step("1", "todo", serde_json::json!({}), LIST_A));
        let mut kept = vec![Message::user("next")];
        kept.extend(step("2", "todo", serde_json::json!({}), LIST_B));
        let carried = Carried::after_folding(Carried::default(), &folded, &kept);
        assert_eq!(carried.checklist, None);

        let mut emptied = folded.clone();
        emptied.extend(step(
            "3",
            "todo",
            serde_json::json!({}),
            "The list is empty.",
        ));
        let previous = Carried {
            checklist: Some(LIST_A.into()),
            files: vec![],
        };
        let carried = Carried::after_folding(previous, &emptied, &[Message::user("x")]);
        assert_eq!(carried.checklist, None);

        // A todo call that failed changes nothing.
        let mut failed = vec![Message::user("go")];
        failed.extend(step(
            "4",
            "todo",
            serde_json::json!({}),
            "Error: todo needs `items`",
        ));
        let previous = Carried {
            checklist: Some(LIST_A.into()),
            files: vec![],
        };
        let carried = Carried::after_folding(previous, &failed, &[Message::user("x")]);
        assert_eq!(carried.checklist.as_deref(), Some(LIST_A));
    }

    #[test]
    fn a_second_compaction_merges_with_what_the_first_kept() {
        let first = Carried {
            checklist: Some(LIST_A.into()),
            files: vec!["src/a.rs".into(), "src/b.rs".into()],
        };
        let summary = join("Goal: fix the bug.", &first);
        let (text, back) = Carried::split(&summary);
        assert_eq!(text, "Goal: fix the bug.");
        assert_eq!(back, first);

        let mut folded = vec![Message::user("more")];
        folded.extend(step(
            "9",
            "write_file",
            serde_json::json!({"path": "src/b.rs"}),
            "Wrote 3 bytes to src/b.rs",
        ));
        let merged = Carried::after_folding(back, &folded, &[Message::user("x")]);
        assert_eq!(merged.checklist.as_deref(), Some(LIST_A));
        assert_eq!(merged.files, vec!["src/b.rs", "src/a.rs"]);
    }

    #[test]
    fn the_file_list_is_capped_and_nothing_renders_as_nothing() {
        let previous = Carried {
            checklist: None,
            files: (0..MAX_FILES).map(|n| format!("old{n}.rs")).collect(),
        };
        let mut folded = vec![Message::user("go")];
        folded.extend(step(
            "1",
            "create_directory",
            serde_json::json!({"path": "new"}),
            "Created",
        ));
        let carried = Carried::after_folding(previous, &folded, &[]);
        assert_eq!(carried.files.len(), MAX_FILES);
        assert_eq!(carried.files[0], "new");
        assert!(!carried.files.contains(&format!("old{}.rs", MAX_FILES - 1)));

        assert_eq!(Carried::default().render(), None);
        assert_eq!(join("text", &Carried::default()), "text");
        assert_eq!(
            Carried::split("just text"),
            ("just text", Carried::default())
        );
    }
}
