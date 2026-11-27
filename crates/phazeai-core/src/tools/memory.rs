//! The `memory` tool: facts the agent saves for later sessions.
//!
//! A project's memories live in `<project>/.phazeai/memory.md`, a plain
//! Markdown list the user can read and edit, and every session in that
//! project carries them in its system prompt (see
//! [`crate::context::system_prompt::project_instructions`]), so the agent
//! starts out knowing them without spending a tool call. Memories meant for
//! every project go to the global store, `~/.phazeai/memory.json`, which the
//! agent searches when it needs them.

use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The project memory file, relative to the project root.
pub const PROJECT_MEMORY_FILE: &str = ".phazeai/memory.md";

const PROJECT_MEMORY_HEADER: &str = "# Project memory\n\n\
<!-- Saved by PhazeAI's memory tool and /remember, and read into every session in this \
project. Edit freely: one \"- key: text\" line per memory. -->\n\n";

#[derive(Serialize, Deserialize, Default)]
struct MemoryStore {
    items: HashMap<String, String>,
}

/// The memories in a project memory file, in file order: `- key: text` lines.
/// Other lines (the heading, the comment, notes the user added) are kept by
/// [`write_project_memory`] only in so far as they are memories, so the file
/// stays a clean list.
pub fn parse_project_memory(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let Some(item) = line.trim_start().strip_prefix("- ") else {
            continue;
        };
        let Some((key, value)) = item.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || value.is_empty() {
            continue;
        }
        match out.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.to_string(),
            None => out.push((key.to_string(), value.to_string())),
        }
    }
    out
}

/// The project's memories, or none when the file is missing or unreadable.
pub fn read_project_memory(root: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(root.join(PROJECT_MEMORY_FILE))
        .map(|text| parse_project_memory(&text))
        .unwrap_or_default()
}

fn render_project_memory(items: &[(String, String)]) -> String {
    let mut text = PROJECT_MEMORY_HEADER.to_string();
    for (key, value) in items {
        text.push_str(&format!("- {key}: {value}\n"));
    }
    text
}

fn write_project_memory(root: &Path, items: &[(String, String)]) -> std::io::Result<()> {
    let path = root.join(PROJECT_MEMORY_FILE);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, render_project_memory(items))
}

/// One line per memory: keys lose `:` and line breaks, text loses line breaks.
fn clean_key(key: &str) -> String {
    key.replace([':', '\n', '\r'], " ").trim().to_string()
}

fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Save `text` under `key` in the project's memory (replacing what that key
/// held), creating the file if needed. Used by the tool and by `/remember`.
pub fn remember(root: &Path, key: &str, text: &str) -> Result<(), String> {
    let (key, text) = (clean_key(key), clean_text(text));
    if key.is_empty() || text.is_empty() {
        return Err("a memory needs a key and some text".into());
    }
    let mut items = read_project_memory(root);
    match items.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = text,
        None => items.push((key, text)),
    }
    write_project_memory(root, &items).map_err(|e| format!("couldn't save the memory: {e}"))
}

/// Remove `key` from the project's memory; whether it was there.
pub fn forget(root: &Path, key: &str) -> Result<bool, String> {
    let key = clean_key(key);
    let mut items = read_project_memory(root);
    let before = items.len();
    items.retain(|(k, _)| *k != key);
    if items.len() == before {
        return Ok(false);
    }
    write_project_memory(root, &items)
        .map(|_| true)
        .map_err(|e| format!("couldn't save the memory: {e}"))
}

/// A key for a memory given as plain text (`/remember always run clippy`):
/// its first few words.
pub fn key_for(text: &str) -> String {
    text.split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join(" ")
        .replace(':', "")
        .to_lowercase()
}

pub struct MemoryTool;

#[derive(PartialEq)]
enum Scope {
    Project(PathBuf),
    Global,
}

fn scope(params: &Value) -> Result<Scope, PhazeError> {
    match params.get("scope").and_then(|v| v.as_str()) {
        Some("global") => Ok(Scope::Global),
        Some("project") | None => match crate::tools::sandbox::workspace_root() {
            Some(root) => Ok(Scope::Project(root)),
            None if params.get("scope").is_some() => Err(PhazeError::tool(
                "memory",
                "No project is open, so there is no project memory; use scope \"global\"",
            )),
            None => Ok(Scope::Global),
        },
        Some(other) => Err(PhazeError::tool(
            "memory",
            format!("Unknown scope '{other}': use \"project\" or \"global\""),
        )),
    }
}

fn global_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(".phazeai")
        .join("memory.json")
}

fn read_global() -> MemoryStore {
    std::fs::read_to_string(global_file())
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

fn write_global(store: &MemoryStore) -> Result<(), PhazeError> {
    let file = global_file();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let serialized = serde_json::to_string_pretty(store)
        .map_err(|e| PhazeError::tool("memory", format!("Failed to serialize: {e}")))?;
    std::fs::write(&file, serialized)
        .map_err(|e| PhazeError::tool("memory", format!("Failed to save memory file: {e}")))
}

fn required<'a>(params: &'a Value, name: &str, action: &str) -> Result<&'a str, PhazeError> {
    params
        .get(name)
        .and_then(|v| v.as_str())
        .ok_or_else(|| PhazeError::tool("memory", format!("Missing '{name}' for {action}")))
}

#[async_trait::async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn summary(&self) -> Option<&str> {
        Some("Facts kept for later sessions")
    }

    fn description(&self) -> &str {
        "Save facts for later sessions. Project memories (the default) are shown to every \
         session in this project, so store only what you would want to know next time: \
         conventions, commands, decisions. Actions: store, forget, search, list. Use scope \
         \"global\" for facts about the user that hold in every project."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "store, forget, search or list",
                    "enum": ["store", "forget", "search", "list"]
                },
                "key": {
                    "type": "string",
                    "description": "Short name of the memory (e.g. 'test command'). Required for store and forget; a filter for search."
                },
                "content": {
                    "type": "string",
                    "description": "One line to remember. Required for store."
                },
                "scope": {
                    "type": "string",
                    "description": "project (default when a project is open) or global",
                    "enum": ["project", "global"]
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let action = required(&params, "action", "the memory tool")?;
        let scope = scope(&params)?;

        if let Scope::Project(root) = &scope {
            let file = root.join(PROJECT_MEMORY_FILE);
            crate::privacy::tool_check("memory", &file, PROJECT_MEMORY_FILE)?;
        }

        let items: BTreeMap<String, String> = match &scope {
            Scope::Project(root) => read_project_memory(root).into_iter().collect(),
            Scope::Global => read_global().items.into_iter().collect(),
        };

        match action {
            "store" => {
                let key = required(&params, "key", "store")?;
                let content = required(&params, "content", "store")?;
                match &scope {
                    Scope::Project(root) => {
                        remember(root, key, content).map_err(|e| PhazeError::tool("memory", e))?
                    }
                    Scope::Global => {
                        let mut store = read_global();
                        store.items.insert(key.to_string(), content.to_string());
                        write_global(&store)?;
                    }
                }
                Ok(serde_json::json!({
                    "message": format!("Saved '{}' to {} memory", clean_key(key), scope_name(&scope)),
                }))
            }
            "forget" => {
                let key = required(&params, "key", "forget")?;
                let removed = match &scope {
                    Scope::Project(root) => {
                        forget(root, key).map_err(|e| PhazeError::tool("memory", e))?
                    }
                    Scope::Global => {
                        let mut store = read_global();
                        let removed = store.items.remove(key).is_some();
                        if removed {
                            write_global(&store)?;
                        }
                        removed
                    }
                };
                Ok(serde_json::json!({
                    "message": if removed {
                        format!("Forgot '{key}'")
                    } else {
                        format!("No memory named '{key}' in {} memory", scope_name(&scope))
                    },
                }))
            }
            "search" => {
                let query = params
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                let results: BTreeMap<String, String> = items
                    .into_iter()
                    .filter(|(k, v)| {
                        query.is_empty()
                            || k.to_lowercase().contains(&query)
                            || v.to_lowercase().contains(&query)
                    })
                    .collect();
                Ok(serde_json::json!({ "results": results }))
            }
            "list" => Ok(serde_json::json!({
                "memory_keys": items.keys().cloned().collect::<Vec<_>>(),
            })),
            _ => Err(PhazeError::tool(
                "memory",
                format!("Unknown action: {action}"),
            )),
        }
    }
}

fn scope_name(scope: &Scope) -> &'static str {
    match scope {
        Scope::Project(_) => "project",
        Scope::Global => "global",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_round_trips_and_a_key_is_replaced_not_repeated() {
        let dir = tempfile::tempdir().unwrap();
        remember(dir.path(), "test command", "cargo test -q").unwrap();
        remember(dir.path(), "style", "no unwrap in library code").unwrap();
        remember(dir.path(), "test command", "make test\nthen clippy").unwrap();

        let text = std::fs::read_to_string(dir.path().join(PROJECT_MEMORY_FILE)).unwrap();
        assert!(text.starts_with("# Project memory\n"), "{text}");
        assert_eq!(
            read_project_memory(dir.path()),
            [
                (
                    "test command".to_string(),
                    "make test then clippy".to_string()
                ),
                ("style".to_string(), "no unwrap in library code".to_string()),
            ]
        );

        assert!(forget(dir.path(), "style").unwrap());
        assert!(!forget(dir.path(), "style").unwrap());
        assert_eq!(read_project_memory(dir.path()).len(), 1);
    }

    #[test]
    fn hand_edited_lines_are_read_and_odd_ones_skipped() {
        let parsed = parse_project_memory(
            "# Project memory\n\nSome note\n- db: postgres 16\n  - deploy: fly.io\n- broken line\n- empty:\n",
        );
        assert_eq!(
            parsed,
            [
                ("db".to_string(), "postgres 16".to_string()),
                ("deploy".to_string(), "fly.io".to_string()),
            ]
        );
    }

    #[test]
    fn a_key_for_plain_text_is_its_first_words() {
        assert_eq!(
            key_for("Always run: cargo clippy before a PR"),
            "always run cargo clippy before"
        );
    }
}
