//! What the model is told about a tool's result.
//!
//! Tools return JSON (the UI, the CLI and the tests read its fields), but
//! JSON is a costly way to show a model a file or a command's output: every
//! line of a file becomes one escaped string, every grep hit repeats its
//! keys and its path, and pretty-printing adds indentation on top. Each
//! result stays in the conversation for the rest of the session, so it is
//! paid for again on every later request.
//!
//! [`for_model`] turns the results of the everyday tools into the plain text
//! a person would read in a terminal. Paths under the current directory are
//! shown relative to it (the tools resolve relative paths against it, so the
//! model can pass them straight back). A result of another tool, or one whose
//! shape is not the expected one, is sent as compact JSON.

use serde_json::Value;
use std::fmt::Write as _;
use std::path::Path;

/// The text the model gets for `value`, the successful result of `tool`.
pub fn for_model(tool: &str, value: &Value) -> String {
    // Paths are shown relative to the folder relative tool paths are read
    // from, which is the open project once the host has set one.
    let cwd = Some(crate::tools::sandbox::base_dir());
    let rendered = match tool {
        "read_file" => read_file(value),
        "bash" => bash(value),
        "grep" => grep(value, cwd.as_deref()),
        "glob" => glob(value, cwd.as_deref()),
        "list_files" => list_files(value),
        "write_file" => write_file(value, cwd.as_deref()),
        "edit_file" => edit_file(value, cwd.as_deref()),
        "code_search" => code_search(value, cwd.as_deref()),
        _ => None,
    };
    let mut text = rendered.unwrap_or_else(|| match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    });
    // A write the user kept only part of says which parts were left out.
    if let Some(review) = value.get("user_review").and_then(Value::as_str) {
        if !text.contains(review) {
            text.push('\n');
            text.push_str(review);
        }
    }
    text
}

/// `path` relative to `cwd` when it lies under it, else as given.
fn short(path: &str, cwd: Option<&Path>) -> String {
    let path = path.strip_prefix("./").unwrap_or(path);
    let Some(cwd) = cwd else {
        return path.to_string();
    };
    match Path::new(path).strip_prefix(cwd) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => path.to_string(),
    }
}

fn read_file(v: &Value) -> Option<String> {
    let content = v.get("content")?.as_str()?;
    let total = v.get("total_lines")?.as_u64()?;
    let shown = v.get("lines_shown")?.as_u64()?;
    let first = v.get("first_line").and_then(Value::as_u64).unwrap_or(1);
    let mut out = content.to_string();
    if total == 0 {
        out.push_str("(empty file)");
    } else if shown == 0 {
        let _ = write!(out, "(no lines from line {first}; the file has {total})");
    } else if shown < total {
        let last = first + shown - 1;
        let _ = write!(out, "\n[lines {first}-{last} of {total}");
        if last < total {
            let _ = write!(out, "; read on with offset {}", last + 1);
        }
        out.push(']');
    }
    if v.get("long_lines_cut").and_then(Value::as_bool) == Some(true) {
        out.push_str("\n[long lines were cut]");
    }
    Some(out)
}

fn bash(v: &Value) -> Option<String> {
    let stdout = v.get("stdout")?.as_str()?;
    let stderr = v.get("stderr")?.as_str()?;
    let code = v.get("exit_code")?;
    let mut out = String::with_capacity(stdout.len() + stderr.len() + 32);
    out.push_str(stdout.trim_end_matches('\n'));
    let stderr = stderr.trim_end_matches('\n');
    if !stderr.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[stderr]\n");
        out.push_str(stderr);
    }
    match code.as_i64() {
        Some(0) => {
            if out.is_empty() {
                out.push_str("(no output)");
            }
        }
        Some(n) => {
            if !out.is_empty() {
                out.push('\n');
            }
            let _ = write!(out, "[exit code {n}]");
        }
        None => {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("[killed by a signal]");
        }
    }
    Some(out)
}

/// Hits grouped by file, as `rg --heading` shows them.
fn grep(v: &Value, cwd: Option<&Path>) -> Option<String> {
    let matches = v.get("matches")?.as_array()?;
    let mut out = String::new();
    let mut current: Option<&str> = None;
    for m in matches {
        let file = m.get("file")?.as_str()?;
        let line = m.get("line")?.as_u64()?;
        let content = m.get("content")?.as_str()?;
        if current != Some(file) {
            if current.is_some() {
                out.push('\n');
            }
            out.push_str(&short(file, cwd));
            out.push('\n');
            current = Some(file);
        }
        let _ = writeln!(out, "{line}: {content}");
    }
    if matches.is_empty() {
        out.push_str("No matches.");
    }
    let out = out.trim_end_matches('\n').to_string();
    Some(with_notes(out, v))
}

fn glob(v: &Value, cwd: Option<&Path>) -> Option<String> {
    let matches = v.get("matches")?.as_array()?;
    let mut lines = Vec::with_capacity(matches.len());
    for m in matches {
        lines.push(short(m.as_str()?, cwd));
    }
    let out = if lines.is_empty() {
        "No files matched.".to_string()
    } else {
        lines.join("\n")
    };
    Some(with_notes(out, v))
}

fn list_files(v: &Value) -> Option<String> {
    let files = v.get("files")?.as_array()?;
    let mut lines = Vec::with_capacity(files.len());
    for f in files {
        let name = f.get("name")?.as_str()?;
        if f.get("type").and_then(Value::as_str) == Some("directory") {
            lines.push(format!("{name}/"));
        } else {
            lines.push(name.to_string());
        }
    }
    let out = if lines.is_empty() {
        "(empty directory)".to_string()
    } else {
        lines.join("\n")
    };
    Some(with_notes(out, v))
}

fn write_file(v: &Value, cwd: Option<&Path>) -> Option<String> {
    let path = v.get("path")?.as_str()?;
    let bytes = v.get("bytes_written")?.as_u64()?;
    Some(format!("Wrote {bytes} bytes to {}", short(path, cwd)))
}

fn edit_file(v: &Value, cwd: Option<&Path>) -> Option<String> {
    let path = v.get("path")?.as_str()?;
    let n = v.get("replacements")?.as_u64()?;
    let s = if n == 1 { "" } else { "s" };
    Some(format!("Edited {} ({n} replacement{s})", short(path, cwd)))
}

fn code_search(v: &Value, cwd: Option<&Path>) -> Option<String> {
    let matches = v.get("matches")?.as_array()?;
    let mut out = String::new();
    for m in matches {
        let file = m.get("file")?.as_str()?;
        let start = m.get("start_line")?.as_u64()?;
        let end = m.get("end_line")?.as_u64()?;
        let snippet = m.get("snippet")?.as_str()?;
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(out, "{}:{start}-{end}", short(file, cwd));
        out.push_str(snippet.trim_end_matches('\n'));
        out.push('\n');
    }
    if matches.is_empty() {
        out.push_str("No matches.");
    }
    let mut out = out.trim_end_matches('\n').to_string();
    if let Some(note) = v.get("note").and_then(Value::as_str) {
        let _ = write!(out, "\n[{note}]");
    }
    Some(with_notes(out, v))
}

/// The notes a listing can carry: hits cut at a cap, files the privacy
/// policy kept out.
fn with_notes(mut out: String, v: &Value) -> String {
    if let Some(limit) = v.get("limit").and_then(Value::as_u64) {
        if v.get("truncated").and_then(Value::as_bool) == Some(true) {
            let _ = write!(
                out,
                "\n[stopped at {limit}; narrow the search to see the rest]"
            );
        }
    }
    if let Some(n) = v.get("privacy_skipped").and_then(Value::as_u64) {
        if n > 0 {
            let _ = write!(out, "\n[{n} files left out by the privacy policy]");
        }
    }
    out
}

/// Whether a directory entry is a version-control store (`.git` and the
/// like). The walkers show hidden files, so without this a recursive listing
/// or search goes through every object in `.git`.
pub(crate) fn is_vcs_dir(entry: &ignore::DirEntry) -> bool {
    entry.depth() > 0
        && entry.file_type().is_some_and(|t| t.is_dir())
        && matches!(
            entry.file_name().to_str(),
            Some(".git" | ".hg" | ".svn" | ".jj")
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cwd() -> Option<&'static Path> {
        Some(Path::new("/w"))
    }

    #[test]
    fn a_whole_file_is_its_numbered_lines_and_nothing_else() {
        let v = json!({"content": "1\tfn main() {}\n2\t", "path": "a.rs",
                       "total_lines": 2, "lines_shown": 2, "first_line": 1});
        assert_eq!(read_file(&v).unwrap(), "1\tfn main() {}\n2\t");
    }

    #[test]
    fn part_of_a_file_says_where_to_read_on() {
        let v = json!({"content": "11\tx", "path": "a.rs",
                       "total_lines": 40, "lines_shown": 1, "first_line": 11});
        assert_eq!(
            read_file(&v).unwrap(),
            "11\tx\n[lines 11-11 of 40; read on with offset 12]"
        );
        let end = json!({"content": "40\tz", "path": "a.rs",
                         "total_lines": 40, "lines_shown": 1, "first_line": 40});
        assert_eq!(read_file(&end).unwrap(), "40\tz\n[lines 40-40 of 40]");
    }

    #[test]
    fn an_empty_file_and_an_offset_past_the_end_say_so() {
        let empty = json!({"content": "", "path": "a", "total_lines": 0,
                           "lines_shown": 0, "first_line": 1});
        assert_eq!(read_file(&empty).unwrap(), "(empty file)");
        let past = json!({"content": "", "path": "a", "total_lines": 3,
                          "lines_shown": 0, "first_line": 9});
        assert_eq!(
            read_file(&past).unwrap(),
            "(no lines from line 9; the file has 3)"
        );
    }

    #[test]
    fn a_command_shows_its_output_and_only_a_failing_exit_code() {
        let ok = json!({"stdout": "hi\n", "stderr": "", "exit_code": 0, "success": true});
        assert_eq!(bash(&ok).unwrap(), "hi");
        let quiet = json!({"stdout": "", "stderr": "", "exit_code": 0, "success": true});
        assert_eq!(bash(&quiet).unwrap(), "(no output)");
        let bad = json!({"stdout": "a\n", "stderr": "boom\n", "exit_code": 2, "success": false});
        assert_eq!(bash(&bad).unwrap(), "a\n[stderr]\nboom\n[exit code 2]");
        let killed = json!({"stdout": "", "stderr": "", "exit_code": null, "success": false});
        assert_eq!(bash(&killed).unwrap(), "[killed by a signal]");
    }

    #[test]
    fn grep_hits_are_grouped_under_their_file_relative_to_the_cwd() {
        let v = json!({"matches": [
            {"file": "/w/src/a.rs", "line": 3, "content": "fn a()"},
            {"file": "/w/src/a.rs", "line": 9, "content": "a();"},
            {"file": "/elsewhere/b.rs", "line": 1, "content": "a"},
        ], "count": 3, "pattern": "a"});
        assert_eq!(
            grep(&v, cwd()).unwrap(),
            "src/a.rs\n3: fn a()\n9: a();\n\n/elsewhere/b.rs\n1: a"
        );
    }

    #[test]
    fn a_capped_listing_says_it_stopped_and_names_privacy_skips() {
        let v = json!({"matches": [], "count": 0, "pattern": "x", "privacy_skipped": 2});
        assert_eq!(
            grep(&v, cwd()).unwrap(),
            "No matches.\n[2 files left out by the privacy policy]"
        );
        let capped = json!({"matches": ["/w/a", "/w/b"], "count": 2, "pattern": "*",
                            "limit": 2, "truncated": true});
        assert_eq!(
            glob(&capped, cwd()).unwrap(),
            "a\nb\n[stopped at 2; narrow the search to see the rest]"
        );
    }

    #[test]
    fn directories_end_with_a_slash() {
        let v = json!({"files": [{"name": "src", "type": "directory"},
                                 {"name": "Cargo.toml", "type": "file"}],
                       "path": ".", "count": 2});
        assert_eq!(list_files(&v).unwrap(), "src/\nCargo.toml");
    }

    #[test]
    fn writes_and_edits_are_one_line() {
        let w = json!({"path": "/w/a.rs", "bytes_written": 12, "success": true});
        assert_eq!(write_file(&w, cwd()).unwrap(), "Wrote 12 bytes to a.rs");
        let e = json!({"path": "a.rs", "replacements": 2, "success": true});
        assert_eq!(
            edit_file(&e, cwd()).unwrap(),
            "Edited a.rs (2 replacements)"
        );
    }

    #[test]
    fn code_search_shows_each_snippet_under_its_place() {
        let v = json!({"matches": [
            {"file": "/w/a.rs", "start_line": 4, "end_line": 5, "score": 1.5, "snippet": "x\ny\n"},
            {"file": "/w/b.rs", "start_line": 1, "end_line": 1, "score": 0.5, "snippet": "z"},
        ]});
        assert_eq!(
            code_search(&v, cwd()).unwrap(),
            "a.rs:4-5\nx\ny\n\nb.rs:1-1\nz"
        );
    }

    #[test]
    fn another_tool_or_an_unexpected_shape_is_compact_json() {
        assert_eq!(for_model("now", &json!({"a": 1})), r#"{"a":1}"#);
        assert_eq!(for_model("bash", &json!({"odd": true})), r#"{"odd":true}"#);
        assert_eq!(for_model("fetch", &json!("page text")), "page text");
    }
}
