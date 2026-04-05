//! Anonymous usage telemetry for PhazeAI.
//!
//! Sends a single fire-and-forget ping to Supabase on each app launch.
//! No personal data is collected — just app type, version, OS, and a random session ID.
//! The session ID is regenerated every launch (not persistent).

use uuid::Uuid;

const SUPABASE_URL: &str = "https://kcrxqmtcpanhldzvehlx.supabase.co";
const SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImtjcnhxbXRjcGFuaGxkenZlaGx4Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3NzUzMzE5ODQsImV4cCI6MjA5MDkwNzk4NH0.0vYvrwMwYbqHcDkkigKumVpaT2PGW28nqPGnbZqaoRE";

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
    // Spawn a detached thread so this works whether or not a tokio runtime exists.
    std::thread::spawn(move || {
        let _ = send_ping(app);
    });
}

fn send_ping(app: AppKind) -> Result<(), Box<dyn std::error::Error>> {
    let payload = serde_json::json!({
        "app": app.as_str(),
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "session_id": Uuid::new_v4().to_string(),
    });

    let url = format!("{}/rest/v1/telemetry", SUPABASE_URL);

    // Use a short-lived blocking reqwest client (no tokio needed).
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    client
        .post(&url)
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", SUPABASE_ANON_KEY))
        .header("Content-Type", "application/json")
        .header("Prefer", "return=minimal")
        .json(&payload)
        .send()?;

    Ok(())
}

/// Fetch the current global usage count. Returns (ide_launches, cli_launches).
/// Used for displaying stats (e.g. "Join 1,234 developers using PhazeAI").
pub async fn fetch_usage_count() -> Result<(u64, u64), Box<dyn std::error::Error + Send + Sync>> {
    let url = format!(
        "{}/rest/v1/usage_summary?select=app,total_launches",
        SUPABASE_URL
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp: Vec<serde_json::Value> = client
        .get(&url)
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", SUPABASE_ANON_KEY))
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
