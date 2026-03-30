/// Screenshot tool — captures what the user sees on screen.
///
/// Uses system tools (scrot/grim/import) to take screenshots and
/// save them. The AI can use this to understand the user's current
/// visual context, debug UI issues, or document application state.
use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;
use std::path::PathBuf;

pub struct ScreenshotTool;

#[async_trait::async_trait]
impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }

    fn description(&self) -> &str {
        "Take a screenshot of the user's screen or a specific window. Saves the image to a file and returns the path. Use this to see what the user sees, debug UI issues, or document the current state of an application."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "output_path": {
                    "type": "string",
                    "description": "Path to save the screenshot (default: /tmp/phazeai_screenshot.png)"
                },
                "region": {
                    "type": "string",
                    "enum": ["fullscreen", "window", "selection"],
                    "description": "What to capture: 'fullscreen' (entire screen), 'window' (active window), 'selection' (user selects area). Default: 'fullscreen'"
                },
                "delay_secs": {
                    "type": "integer",
                    "description": "Delay in seconds before taking the screenshot (default: 0)"
                }
            }
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let output_path = params
            .get("output_path")
            .and_then(|v| v.as_str())
            .unwrap_or("/tmp/phazeai_screenshot.png");

        let region = params
            .get("region")
            .and_then(|v| v.as_str())
            .unwrap_or("fullscreen");

        let delay_secs = params
            .get("delay_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if delay_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
        }

        // Detect available screenshot tool and display server
        let (command, args) = detect_screenshot_command(output_path, region)?;

        let output = tokio::process::Command::new(&command)
            .args(&args)
            .output()
            .await
            .map_err(|e| {
                PhazeError::tool(
                    "screenshot",
                    format!("Failed to run screenshot command '{command}': {e}. Install 'scrot' (X11) or 'grim' (Wayland): sudo apt install scrot"),
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PhazeError::tool(
                "screenshot",
                format!("Screenshot failed: {stderr}"),
            ));
        }

        let path = PathBuf::from(output_path);
        let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        Ok(serde_json::json!({
            "path": output_path,
            "size_bytes": file_size,
            "region": region,
            "format": "png",
            "message": format!("Screenshot saved to {output_path} ({} KB)", file_size / 1024),
        }))
    }
}

fn detect_screenshot_command(
    output_path: &str,
    region: &str,
) -> Result<(String, Vec<String>), PhazeError> {
    // Check if Wayland
    let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|v| v == "wayland")
            .unwrap_or(false);

    if is_wayland {
        // Try grim (Wayland screenshot tool)
        if which_exists("grim") {
            let args = match region {
                "window" | "selection" => {
                    if which_exists("slurp") {
                        // slurp lets user select a region
                        // We need to pipe slurp output to grim -g
                        vec![
                            "-g".to_string(),
                            "$(slurp)".to_string(),
                            output_path.to_string(),
                        ]
                    } else {
                        vec![output_path.to_string()]
                    }
                }
                _ => vec![output_path.to_string()],
            };
            return Ok(("grim".to_string(), args));
        }
    }

    // X11: try scrot
    if which_exists("scrot") {
        let mut args = vec![];
        match region {
            "window" => args.push("-u".to_string()),    // active window
            "selection" => args.push("-s".to_string()), // selection mode
            _ => {}                                     // fullscreen is default
        }
        args.push(output_path.to_string());
        return Ok(("scrot".to_string(), args));
    }

    // Try import (ImageMagick)
    if which_exists("import") {
        let mut args = vec![];
        match region {
            "window" => {
                args.push("-window".to_string());
                args.push("root".to_string());
            }
            _ => {
                args.push("-window".to_string());
                args.push("root".to_string());
            }
        }
        args.push(output_path.to_string());
        return Ok(("import".to_string(), args));
    }

    // Try gnome-screenshot
    if which_exists("gnome-screenshot") {
        let mut args = vec!["-f".to_string(), output_path.to_string()];
        match region {
            "window" => args.push("-w".to_string()),
            "selection" => args.push("-a".to_string()),
            _ => {}
        }
        return Ok(("gnome-screenshot".to_string(), args));
    }

    Err(PhazeError::tool(
        "screenshot",
        "No screenshot tool found. Install one: sudo apt install scrot (X11) or sudo apt install grim (Wayland)",
    ))
}

fn which_exists(command: &str) -> bool {
    // Try `{command} --version` first, then `-v` as fallback.
    // This avoids relying on `which` (Unix-only) or `where` (Windows-only).
    for flag in &["--version", "-v"] {
        match std::process::Command::new(command).arg(flag).output() {
            Ok(o) => {
                // Exit 0 means the binary exists and understood the flag.
                // Some tools exit non-zero for --version but still exist; treat
                // any successful spawn (even non-zero exit) as "found" unless
                // the OS itself couldn't find the binary (NotFound error).
                let _ = o;
                return true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
            Err(_) => {
                // Other OS error — binary may still exist, try next flag.
            }
        }
    }
    false
}
