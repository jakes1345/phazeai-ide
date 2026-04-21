//! Anonymous usage telemetry for PhazeAI.
//!
//! Sends a single fire-and-forget ping to Supabase on each app launch.
//! No personal data is collected — just app type, version, OS, and a random session ID.
//! The session ID is regenerated every launch (not persistent).
//!
//! ## Configuration
//!
//! The endpoint can be configured at build time via the `PHAZEAI_TELEMETRY_URL`
//! and `PHAZEAI_TELEMETRY_KEY` env vars (baked in via `option_env!`), or at
//! runtime via the same env vars. If neither is set, telemetry is a no-op.
//! Users can also disable telemetry entirely by setting `PHAZEAI_TELEMETRY=0`.

use uuid::Uuid;

// Compile-time fallbacks. The Supabase anon key is meant to be public (it's
// the client-side key with RLS enforced), but we still keep it off of the
// hardcoded path so anyone building PhazeAI from source can point at their
// own endpoint.
const DEFAULT_SUPABASE_URL: Option<&str> = option_env!("PHAZEAI_TELEMETRY_URL");
const DEFAULT_SUPABASE_KEY: Option<&str> = option_env!("PHAZEAI_TELEMETRY_KEY");

// Public telemetry endpoint used by official PhazeAI releases. Can be
// overridden at build or run time (see above). Empty string == no-op.
const BUILTIN_SUPABASE_URL: &str = "https://kcrxqmtcpanhldzvehlx.supabase.co";
const BUILTIN_SUPABASE_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImtjcnhxbXRjcGFuaGxkenZlaGx4Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3NzUzMzE5ODQsImV4cCI6MjA5MDkwNzk4NH0.0vYvrwMwYbqHcDkkigKumVpaT2PGW28nqPGnbZqaoRE";

fn supabase_url() -> String {
    std::env::var("PHAZEAI_TELEMETRY_URL")
        .ok()
        .or_else(|| DEFAULT_SUPABASE_URL.map(str::to_owned))
        .unwrap_or_else(|| BUILTIN_SUPABASE_URL.to_owned())
}

fn supabase_key() -> String {
    std::env::var("PHAZEAI_TELEMETRY_KEY")
        .ok()
        .or_else(|| DEFAULT_SUPABASE_KEY.map(str::to_owned))
        .unwrap_or_else(|| BUILTIN_SUPABASE_KEY.to_owned())
}

fn telemetry_enabled() -> bool {
    match std::env::var("PHAZEAI_TELEMETRY").as_deref() {
        Ok("0") | Ok("false") | Ok("off") | Ok("no") => false,
        _ => !supabase_url().is_empty() && !supabase_key().is_empty(),
    }
}

/// Which PhazeAI app is reporting.
#[derive(Debug, Clone, Copy)]
pub enum AppKind {
    Ide,
    Cli,
}

impl AppKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ide => "ide",
            Self::Cli => "cli",
        }
    }
}

/// Send an anonymous telemetry ping. Fire-and-forget — errors are silently ignored.
/// Call this once on app startup. It spawns a background task and returns immediately.
pub fn report_launch(app: AppKind) {
    if !telemetry_enabled() {
        return;
    }
    // Detached thread so this works whether or not a tokio runtime exists.
    std::thread::spawn(move || {
        if let Err(err) = send_ping(app) {
            tracing::debug!(error = %err, "telemetry ping failed");
        }
    });
}

fn send_ping(app: AppKind) -> Result<(), Box<dyn std::error::Error>> {
    let url_base = supabase_url();
    let key = supabase_key();

    let payload = serde_json::json!({
        "app": app.as_str(),
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "session_id": Uuid::new_v4().to_string(),
    });

    let url = format!("{url_base}/rest/v1/telemetry");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    client
        .post(&url)
        .header("apikey", &key)
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .header("Prefer", "return=minimal")
        .json(&payload)
        .send()?;

    Ok(())
}

/// Fetch the current global usage count. Returns (ide_launches, cli_launches).
/// Used for displaying stats (e.g. "Join 1,234 developers using PhazeAI").
pub async fn fetch_usage_count() -> Result<(u64, u64), Box<dyn std::error::Error + Send + Sync>> {
    if !telemetry_enabled() {
        return Ok((0, 0));
    }

    let url_base = supabase_url();
    let key = supabase_key();
    let url = format!("{url_base}/rest/v1/usage_summary?select=app,total_launches");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp: Vec<serde_json::Value> = client
        .get(&url)
        .header("apikey", &key)
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await?
        .json()
        .await?;

    let mut ide = 0u64;
    let mut cli = 0u64;
    for row in &resp {
        let app = row.get("app").and_then(|v| v.as_str()).unwrap_or("");
        let count = row
            .get("total_launches")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        match app {
            "ide" => ide = count,
            "cli" => cli = count,
            _ => {}
        }
    }

    Ok((ide, cli))
}
