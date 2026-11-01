use crate::error::PhazeError;
use crate::tools::file::{writable_target, FileWrite};
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;

const TOOL: &str = "edit_file";

/// Surgical text replacement tool - finds old_text and replaces with new_text.
pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        "Make a surgical edit to a file by replacing old_text with new_text. If old_text appears multiple times, you MUST provide 'context' (an exact string of surrounding code) to disambiguate, or set 'replace_all' to true."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_text": {
                    "type": "string",
                    "description": "The exact text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "The text to replace old_text with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)",
                    "default": false
                },
                "context": {
                    "type": "string",
                    "description": "Surrounding context to disambiguate when old_text appears multiple times"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        plan_edit(&params).await?.commit().await
    }

    async fn plan_write(&self, params: &Value) -> Option<Result<FileWrite, PhazeError>> {
        Some(plan_edit(params).await)
    }
}

/// What `edit_file` with `params` makes of a file holding `content`, or the
/// error the tool reports for it.
///
/// This is the one computation behind the edit and every preview of it (the
/// agent's diff review, the CLI's approval prompt), so a preview shows what
/// the tool writes. It reads no file: `content` is what the file holds.
pub fn apply_edit(content: &str, params: &Value) -> Result<Edited, PhazeError> {
    EditRequest::parse(params)?.apply(content)
}

/// The outcome of [`apply_edit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edited {
    /// The whole content of the file after the edit.
    pub content: String,
    /// How many occurrences of `old_text` were replaced.
    pub replacements: usize,
}

/// The parameters of an `edit_file` call.
struct EditRequest<'a> {
    path: &'a str,
    old_text: &'a str,
    new_text: &'a str,
    replace_all: bool,
    context: Option<&'a str>,
}

impl<'a> EditRequest<'a> {
    fn parse(params: &'a Value) -> Result<Self, PhazeError> {
        let required = |name: &str| {
            params.get(name).and_then(|v| v.as_str()).ok_or_else(|| {
                PhazeError::tool(TOOL, format!("Missing required parameter: {name}"))
            })
        };
        Ok(Self {
            path: required("path")?,
            old_text: required("old_text")?,
            new_text: required("new_text")?,
            replace_all: params
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            context: params.get("context").and_then(|v| v.as_str()),
        })
    }

    fn apply(&self, content: &str) -> Result<Edited, PhazeError> {
        let (path, old_text, new_text) = (self.path, self.old_text, self.new_text);
        let match_count = content.matches(old_text).count();

        if match_count == 0 {
            return Err(PhazeError::tool(
                TOOL,
                format!("old_text not found in '{}'", path),
            ));
        }

        let new_content = if self.replace_all {
            content.replace(old_text, new_text)
        } else if match_count == 1 {
            content.replacen(old_text, new_text, 1)
        } else if let Some(ctx) = self.context {
            let best_offset = occurrence_in_context(content, old_text, ctx)?;
            format!(
                "{}{}{}",
                &content[..best_offset],
                new_text,
                &content[best_offset + old_text.len()..]
            )
        } else {
            return Err(PhazeError::tool(
                TOOL,
                format!(
                    "old_text matches {} times in '{}'. Use 'replace_all: true' or provide a 'context' string (surrounding code) to disambiguate.",
                    match_count, path
                ),
            ));
        };

        Ok(Edited {
            content: new_content,
            replacements: if self.replace_all { match_count } else { 1 },
        })
    }
}

/// What `edit_file` with `params` would write, worked out without writing.
async fn plan_edit(params: &Value) -> Result<FileWrite, PhazeError> {
    let request = EditRequest::parse(params)?;
    let path = request.path;
    let target = writable_target(TOOL, path)?;
    let content = tokio::fs::read_to_string(&target)
        .await
        .map_err(|e| PhazeError::tool(TOOL, format!("Failed to read '{}': {}", path, e)))?;
    let edited = request.apply(&content)?;
    let result = serde_json::json!({
        "path": path,
        "replacements": edited.replacements,
        "success": true,
    });
    Ok(FileWrite::new(
        TOOL,
        path,
        target,
        Some(content.into_bytes()),
        edited.content,
        result,
    ))
}

/// Where to apply an edit whose `old_text` occurs several times, given the
/// model's `context` (surrounding code): the one occurrence that lies inside
/// a place where the context appears. If the context holds no occurrence
/// (it is the code next to it), the occurrence nearest the context's single
/// appearance is used. Anything else is ambiguous and refused, so the model
/// gives more surrounding code instead of the edit landing on a line it did
/// not name.
fn occurrence_in_context(content: &str, old_text: &str, ctx: &str) -> Result<usize, PhazeError> {
    let places: Vec<usize> = content.match_indices(ctx).map(|(i, _)| i).collect();
    if places.is_empty() {
        return Err(PhazeError::tool(
            TOOL,
            "Provided 'context' string was not found in the file. Please provide an exact match of surrounding code.",
        ));
    }
    let occurrences: Vec<usize> = content.match_indices(old_text).map(|(i, _)| i).collect();
    let inside: Vec<usize> = occurrences
        .iter()
        .copied()
        .filter(|&o| {
            places
                .iter()
                .any(|&p| o >= p && o + old_text.len() <= p + ctx.len())
        })
        .collect();
    match (inside.as_slice(), places.as_slice()) {
        ([one], _) => Ok(*one),
        ([], [place]) => occurrences
            .iter()
            .copied()
            .min_by_key(|&o| o.abs_diff(*place))
            .ok_or_else(|| PhazeError::tool(TOOL, "old_text not found")),
        ([], _) => Err(PhazeError::tool(
            TOOL,
            format!(
                "'context' appears {} times in the file and contains old_text in none of them. \
                 Give more surrounding code, including old_text, so it names one place.",
                places.len()
            ),
        )),
        (several, _) => Err(PhazeError::tool(
            TOOL,
            format!(
                "old_text appears {} times inside the given 'context'. Give a context that \
                 contains it once.",
                several.len()
            ),
        )),
    }
}

#[cfg(test)]
mod context_tests {
    use super::apply_edit;
    use serde_json::json;

    fn edit(content: &str, old: &str, ctx: &str) -> Result<String, String> {
        apply_edit(
            content,
            &json!({ "path": "f", "old_text": old, "new_text": "Y", "context": ctx }),
        )
        .map(|e| e.content)
        .map_err(|e| e.to_string())
    }

    #[test]
    fn the_occurrence_inside_the_context_is_edited_even_when_another_starts_nearer() {
        // The first `x` starts 2 bytes before the context; the second is inside it.
        assert_eq!(
            edit("aaaaa x\nc = x\n", "x", "c = x").unwrap(),
            "aaaaa x\nc = Y\n"
        );
    }

    #[test]
    fn a_context_next_to_the_occurrence_picks_the_nearest_one() {
        let content = "x = 1\nfn a() {}\nfn b() {\nx = 1\n}\n";
        assert_eq!(
            edit(content, "x = 1", "fn b() {").unwrap(),
            "x = 1\nfn a() {}\nfn b() {\nY\n}\n"
        );
    }

    #[test]
    fn ambiguous_contexts_are_refused() {
        // The context holds old_text twice.
        let twice_inside = edit("x x\nx\n", "x", "x x");
        assert!(twice_inside.unwrap_err().contains("2 times inside"));
        // The context appears twice, so it holds old_text in both places.
        let context_twice = edit("f(1)\nf(1)\n", "1", "f(1)");
        assert!(context_twice.unwrap_err().contains("2 times inside"));
        // The context appears twice and holds old_text in neither.
        let repeated_without = edit("}\nx\n}\nx\n", "x", "}");
        assert!(repeated_without
            .unwrap_err()
            .contains("contains old_text in none"));
    }
}
