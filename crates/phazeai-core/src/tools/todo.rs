//! `todo`: the agent's checklist for a task of several steps.
//!
//! Stateless and light: the model sends the whole list each time and gets it
//! back as a few short lines, which is also what the user sees. Nothing is
//! stored, so the list costs tokens only when the model updates it, and it is
//! held back behind `more_tools` (a line on every request, not a schema).

use super::traits::{Tool, ToolResult};
use crate::error::PhazeError;
use serde_json::Value;

/// The most items a list may hold; a longer one is a plan, not a checklist.
const MAX_ITEMS: usize = 30;
/// An item's text is cut to this many bytes.
const MAX_ITEM_BYTES: usize = 200;

pub struct TodoTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Todo,
    Doing,
    Done,
}

impl Status {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "todo" | "pending" | "open" => Some(Self::Todo),
            "doing" | "in_progress" | "in progress" | "active" => Some(Self::Doing),
            "done" | "completed" | "complete" => Some(Self::Done),
            _ => None,
        }
    }

    fn mark(self) -> &'static str {
        match self {
            Self::Todo => "[ ]",
            Self::Doing => "[>]",
            Self::Done => "[x]",
        }
    }
}

/// The list in `params`, rendered one item to a line, with a count.
fn render(params: &Value) -> Result<String, String> {
    let items = params
        .get("items")
        .and_then(Value::as_array)
        .ok_or("todo needs `items`: the whole list, each item {\"task\", \"status\"}")?;
    if items.len() > MAX_ITEMS {
        return Err(format!(
            "A list holds at most {MAX_ITEMS} items; group the steps into fewer."
        ));
    }
    if items.is_empty() {
        return Ok("The list is empty.".to_string());
    }
    let mut lines = Vec::with_capacity(items.len() + 1);
    let mut done = 0;
    for (i, item) in items.iter().enumerate() {
        let task = item
            .get("task")
            .and_then(Value::as_str)
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty())
            .ok_or_else(|| format!("Item {} has no `task` text.", i + 1))?;
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .map_or(Some(Status::Todo), Status::parse)
            .ok_or_else(|| format!("Item {}'s `status` must be todo, doing or done.", i + 1))?;
        if status == Status::Done {
            done += 1;
        }
        let task = crate::text::truncate_bytes(&task, MAX_ITEM_BYTES);
        lines.push(format!("{} {task}", status.mark()));
    }
    lines.push(format!("{done} of {} done", items.len()));
    Ok(lines.join("\n"))
}

#[async_trait::async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Keep a checklist for a task of three or more steps, which the user sees. Send the \
         whole list each time: mark the item you are on doing and items you finish done."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "task": { "type": "string" },
                            "status": { "type": "string", "enum": ["todo", "doing", "done"] }
                        },
                        "required": ["task", "status"]
                    }
                }
            },
            "required": ["items"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        render(&params)
            .map(Value::String)
            .map_err(|e| PhazeError::tool("todo", e))
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_list_comes_back_as_short_lines_with_a_count() {
        let out = render(&json!({"items": [
            {"task": "Read the parser", "status": "done"},
            {"task": "Fix the  off-by-one\n", "status": "in_progress"},
            {"task": "Add a test", "status": "todo"},
        ]}))
        .unwrap();
        assert_eq!(
            out,
            "[x] Read the parser\n[>] Fix the off-by-one\n[ ] Add a test\n1 of 3 done"
        );
    }

    #[test]
    fn bad_lists_say_what_to_fix() {
        assert!(render(&json!({})).unwrap_err().contains("needs `items`"));
        assert_eq!(
            render(&json!({"items": [{"task": " ", "status": "todo"}]})).unwrap_err(),
            "Item 1 has no `task` text."
        );
        assert_eq!(
            render(&json!({"items": [{"task": "x", "status": "maybe"}]})).unwrap_err(),
            "Item 1's `status` must be todo, doing or done."
        );
        let many: Vec<Value> = (0..=MAX_ITEMS)
            .map(|i| json!({"task": format!("step {i}"), "status": "todo"}))
            .collect();
        assert!(render(&json!({ "items": many })).is_err());
        assert_eq!(render(&json!({"items": []})).unwrap(), "The list is empty.");
    }
}
