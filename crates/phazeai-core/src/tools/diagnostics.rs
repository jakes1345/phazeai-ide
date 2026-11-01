use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

const MAX_DIAGNOSTICS: usize = 100;
/// How much of the checker's own output `raw_output` returns: longer output
/// keeps its start and its end (see [`crate::text::cut_middle`]).
const MAX_RAW_OUTPUT_BYTES: usize = 10_000;
/// How long a check may run before it is killed.
const CHECK_TIMEOUT: Duration = Duration::from_secs(120);

pub struct DiagnosticsTool;

#[async_trait::async_trait]
impl Tool for DiagnosticsTool {
    fn name(&self) -> &str {
        "diagnostics"
    }

    fn description(&self) -> &str {
        "Get compiler/linter diagnostics (errors, warnings) for the project. Runs language-specific checkers: `cargo check` for Rust, `npx tsc --noEmit` for TypeScript, `python -m py_compile` for Python. Returns structured error information with file, line, and message."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project directory or file to check (default: current directory)"
                },
                "language": {
                    "type": "string",
                    "enum": ["rust", "typescript", "python", "auto"],
                    "description": "Language to check (default: 'auto' — detects from project files)"
                }
            }
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let path_str = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        let path = Path::new(path_str);
        if !path.exists() {
            return Err(PhazeError::tool(
                "diagnostics",
                format!("Path does not exist: {path_str}"),
            ));
        }
        // Compiler output quotes source lines, so a denied file can't be checked.
        crate::privacy::tool_check("diagnostics", path, path_str)?;

        let language = params
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("auto");

        let detected_language = if language == "auto" {
            detect_language(path)
        } else {
            language.to_string()
        };

        let (command, args) = match detected_language.as_str() {
            "rust" => ("cargo", vec!["check", "--message-format=short"]),
            "typescript" => ("npx", vec!["tsc", "--noEmit", "--pretty", "false"]),
            "python" => ("python3", vec!["-m", "py_compile", path_str]),
            other => {
                return Err(PhazeError::tool(
                    "diagnostics",
                    format!("Unsupported language: {other}. Supported: rust, typescript, python"),
                ));
            }
        };

        let mut cmd = crate::process::tokio_command(command);
        cmd.args(&args).current_dir(path_str);
        // Stop drops this future, which kills the checker and everything it
        // started (cargo's rustc and build scripts); so does the timeout.
        let output = crate::process::output_or_kill(cmd, CHECK_TIMEOUT)
            .await
            .map_err(|e| PhazeError::tool("diagnostics", format!("Failed to run {command}: {e}")))?
            .ok_or_else(|| {
                PhazeError::tool(
                    "diagnostics",
                    format!("Diagnostics check timed out ({}s)", CHECK_TIMEOUT.as_secs()),
                )
            })?;

        Ok(report(&detected_language, &output))
    }
}

/// The result of a check that ran: the diagnostics parsed from everything the
/// checker printed, and that output itself, cut in the middle when long (the
/// first errors come first, the summary last).
fn report(language: &str, output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    let diagnostics = parse_diagnostics(&combined, language);

    serde_json::json!({
        "language": language,
        "success": output.status.success(),
        "exit_code": output.status.code(),
        "diagnostics": diagnostics,
        "count": diagnostics.len(),
        "raw_output": crate::text::cut_middle(&combined, MAX_RAW_OUTPUT_BYTES),
    })
}

fn detect_language(path: &Path) -> String {
    if path.join("Cargo.toml").exists() {
        "rust".to_string()
    } else if path.join("tsconfig.json").exists() || path.join("package.json").exists() {
        "typescript".to_string()
    } else if path.join("requirements.txt").exists()
        || path.join("pyproject.toml").exists()
        || path.join("setup.py").exists()
    {
        "python".to_string()
    } else {
        "rust".to_string() // Default to rust for PhazeAI
    }
}

fn parse_diagnostics(output: &str, language: &str) -> Vec<Value> {
    let mut diagnostics = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parsed = match language {
            "rust" => parse_rust_diagnostic(line),
            "typescript" => parse_ts_diagnostic(line),
            _ => None,
        };

        if let Some(diag) = parsed {
            diagnostics.push(diag);
            if diagnostics.len() >= MAX_DIAGNOSTICS {
                break;
            }
        }
    }

    diagnostics
}

fn parse_rust_diagnostic(line: &str) -> Option<Value> {
    // Format: "error[E0425]: cannot find value `foo` in this scope"
    // Or: "src/main.rs:10:5: error: description"
    if line.starts_with("error") || line.starts_with("warning") {
        let severity = if line.starts_with("error") {
            "error"
        } else {
            "warning"
        };
        Some(serde_json::json!({
            "severity": severity,
            "message": line,
        }))
    } else if line.contains(": error") || line.contains(": warning") {
        let severity = if line.contains(": error") {
            "error"
        } else {
            "warning"
        };
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() >= 3 {
            Some(serde_json::json!({
                "file": parts[0],
                "line": parts[1].trim().parse::<u32>().ok(),
                "column": parts[2].trim().parse::<u32>().ok(),
                "severity": severity,
                "message": parts.get(3).unwrap_or(&"").trim(),
            }))
        } else {
            Some(serde_json::json!({
                "severity": severity,
                "message": line,
            }))
        }
    } else {
        None
    }
}

fn parse_ts_diagnostic(line: &str) -> Option<Value> {
    // Format: "src/index.ts(10,5): error TS2304: Cannot find name 'foo'."
    // Parsed back from the "): error TS" that closes the location: the path
    // may hold parentheses itself (Next.js route groups: `app/(auth)/x.tsx`),
    // and every slice below starts or ends at a char that was searched for,
    // never at a guessed byte offset.
    let (severity, close_paren) = match line.find("): error TS") {
        Some(i) => ("error", i),
        None => ("warning", line.find("): warning TS")?),
    };
    let head = &line[..close_paren];
    let open_paren = head.rfind('(')?;
    let file = &head[..open_paren];
    let mut location = head[open_paren + 1..].split(',');
    let line_num = location.next().and_then(|s| s.trim().parse::<u32>().ok());
    let col_num = location.next().and_then(|s| s.trim().parse::<u32>().ok());
    // After "):".
    let message = line[close_paren + 2..].trim();

    Some(serde_json::json!({
        "file": file,
        "line": line_num,
        "column": col_num,
        "severity": severity,
        "message": message,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::ExitStatus;

    /// The checker's output used to be cut with `&combined[..10000]`, which
    /// panics when byte 10 000 falls inside a multi-byte character. Now the
    /// start (first errors) and the end (summary) are kept.
    #[test]
    fn long_checker_output_keeps_its_start_and_end_and_never_splits_a_character() {
        let first = "error[E0425]: cannot find value `größe` in this scope\n";
        let last = "error: could not compile `demo` (lib) due to 1 previous error\n";
        let mut stderr = first.to_string();
        // The combined output is "{stdout}\n{stderr}" with an empty stdout,
        // so its byte 10 000 is the second byte of the first 'ß'.
        stderr.push_str(&"=".repeat(9_998 - stderr.len()));
        stderr.push_str(&"ß".repeat(10_000));
        stderr.push('\n');
        stderr.push_str(last);
        let combined = format!("\n{stderr}");
        assert!(!combined.is_char_boundary(10_000));

        let output = Output {
            status: ExitStatus::default(),
            stdout: Vec::new(),
            stderr: stderr.into_bytes(),
        };
        let result = report("rust", &output);

        let raw = result["raw_output"].as_str().unwrap();
        let (head, rest) = raw.split_once("\n[... ").expect("raw_output was not cut");
        let (omitted, tail) = rest.split_once(" bytes omitted ...]\n").unwrap();
        assert!(head.starts_with(&format!("\n{first}")), "lost the start");
        assert!(tail.ends_with(last), "lost the summary");
        assert!(combined.starts_with(head) && combined.ends_with(tail));
        let omitted: usize = omitted.parse().unwrap();
        assert_eq!(head.len() + omitted + tail.len(), combined.len());
        assert!(head.len() + tail.len() <= MAX_RAW_OUTPUT_BYTES);
        // Diagnostics still come from all of the output, not the cut copy.
        assert_eq!(result["count"], 2);
        assert_eq!(result["diagnostics"][1]["message"], last.trim());
    }

    /// The location used to be found from the first '(' and ')' of the line,
    /// and the message sliced 2 bytes past that ')': inside a character when
    /// a non-ASCII one follows it (a panic), and at the wrong place whenever
    /// the path holds parentheses.
    #[test]
    fn ts_lines_whose_path_holds_parentheses_parse_without_panicking() {
        let d = parse_ts_diagnostic(
            "app/(auth)é/page.tsx(12,5): error TS2304: Cannot find name 'foo'.",
        )
        .unwrap();
        assert_eq!(d["file"], "app/(auth)é/page.tsx");
        assert_eq!(d["line"], 12);
        assert_eq!(d["column"], 5);
        assert_eq!(d["severity"], "error");
        assert_eq!(d["message"], "error TS2304: Cannot find name 'foo'.");

        let d = parse_ts_diagnostic("src/index.ts(3,14): warning TS6133: 'x' is unused.").unwrap();
        assert_eq!(d["file"], "src/index.ts");
        assert_eq!(d["line"], 3);
        assert_eq!(d["column"], 14);
        assert_eq!(d["severity"], "warning");
        assert_eq!(d["message"], "warning TS6133: 'x' is unused.");

        assert!(parse_ts_diagnostic("Found 2 errors in 1 file.").is_none());
    }
}
