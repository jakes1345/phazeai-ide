//! Append one NDJSON line to the file named by `PHAZEAI_DEBUG_LOG` if set.
//! Used for opt-in agent/tool tracing; no hardcoded paths.

const ENV: &str = "PHAZEAI_DEBUG_LOG";

pub fn log(
    session_id: &str,
    run_id: &str,
    hypothesis_id: &str,
    location: &str,
    message: &str,
    data: serde_json::Value,
) {
    let Ok(path) = std::env::var(ENV) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let payload = serde_json::json!({
        "sessionId": session_id,
        "runId": run_id,
        "hypothesisId": hypothesis_id,
        "location": location,
        "message": message,
        "data": data,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    });
    let log_path = std::path::Path::new(&path);
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{}", payload);
    }
}
