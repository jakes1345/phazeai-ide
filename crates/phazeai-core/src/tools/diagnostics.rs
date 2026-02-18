use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::Path;

const MAX_DIAGNOSTICS: usize = 100;

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
        let path_str = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let path = Path::new(path_str);
        if !path.exists() {
            return Err(PhazeError::tool("diagnostics", format!("Path does not exist: {path_str}")));
        }

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

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            tokio::process::Command::new(command)
                .args(&args)
                .current_dir(path_str)
                .output(),
        )
        .await
        .map_err(|_| PhazeError::tool("diagnostics", "Diagnostics check timed out (120s)"))?
        .map_err(|e| PhazeError::tool("diagnostics", format!("Failed to run {command}: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");

        let diagnostics = parse_diagnostics(&combined, &detected_language);

        Ok(serde_json::json!({
            "language": detected_language,
            "success": output.status.success(),
            "exit_code": output.status.code(),
            "diagnostics": diagnostics,
            "count": diagnostics.len(),
            "raw_output": if combined.len() > 10000 {
                format!("{}... [truncated]", &combined[..10000])
            } else {
                combined.to_string()
            },
        }))
    }
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
        let severity = if line.starts_with("error") { "error" } else { "warning" };
        Some(serde_json::json!({
            "severity": severity,
            "message": line,
        }))
    } else if line.contains(": error") || line.contains(": warning") {
        let severity = if line.contains(": error") { "error" } else { "warning" };
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
    if line.contains("): error TS") || line.contains("): warning TS") {
        let severity = if line.contains("): error") { "error" } else { "warning" };
        let paren_idx = line.find('(')?;
        let file = &line[..paren_idx];
        let rest = &line[paren_idx + 1..];
        let close_paren = rest.find(')')?;
        let location = &rest[..close_paren];
        let message = rest[close_paren + 2..].trim();

        let mut parts = location.split(',');
        let line_num = parts.next().and_then(|s| s.trim().parse::<u32>().ok());
        let col_num = parts.next().and_then(|s| s.trim().parse::<u32>().ok());

        Some(serde_json::json!({
            "file": file,
            "line": line_num,
            "column": col_num,
            "severity": severity,
            "message": message,
        }))
    } else {
        None
    }
}
