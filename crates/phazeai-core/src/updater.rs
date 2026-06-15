use serde::Deserialize;

/// Info about a release fetched from the GitHub releases API.
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    /// Semver tag, e.g. "v0.2.0"
    pub version: String,
    /// One-line summary from the release title
    pub title: String,
    /// URL to the GitHub release page (not a direct download)
    pub release_url: String,
    /// Per-platform download URLs extracted from the release assets
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
    pub size_bytes: u64,
}

impl ReleaseInfo {
    /// Best download asset for the current platform, in priority order:
    /// `.deb` > `.AppImage` on Linux, `.dmg` on macOS, `-setup.exe` on Windows.
    pub fn best_asset_for_current_platform(&self) -> Option<&ReleaseAsset> {
        let preferred: &[&str] = if cfg!(target_os = "linux") {
            &[".deb", ".AppImage", ".tar.gz"]
        } else if cfg!(target_os = "macos") {
            &[".dmg", ".tar.gz"]
        } else if cfg!(target_os = "windows") {
            &["-setup.exe", ".zip"]
        } else {
            &[".tar.gz"]
        };

        let arch_hint: &str = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };

        for ext in preferred {
            // Prefer arch-matching asset, then fall back to any match
            if let Some(a) = self
                .assets
                .iter()
                .find(|a| a.name.contains(arch_hint) && a.name.ends_with(ext))
            {
                return Some(a);
            }
            if let Some(a) = self.assets.iter().find(|a| a.name.ends_with(ext)) {
                return Some(a);
            }
        }
        None
    }
}

// ── GitHub API response types ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    prerelease: bool,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

// ── Public API ────────────────────────────────────────────────────────────────

const GITHUB_REPO: &str = "jakes1345/phazeai-ide";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Fetch the latest stable release from GitHub.
/// Returns `None` if the current version is already up-to-date or if the
/// fetch fails (network error, rate limit, etc.).
pub async fn check_for_update() -> Option<ReleaseInfo> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::Client::builder()
        .user_agent(format!("phazeai-ide/{}", CURRENT_VERSION))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let release: GhRelease = resp.json().await.ok()?;

    // Skip pre-releases
    if release.prerelease {
        return None;
    }

    // Compare semver strings — strip leading 'v'
    let latest = release.tag_name.trim_start_matches('v');
    let current = CURRENT_VERSION;
    if !is_newer(latest, current) {
        return None;
    }

    Some(ReleaseInfo {
        version: release.tag_name.clone(),
        title: release.name.unwrap_or_else(|| release.tag_name.clone()),
        release_url: release.html_url,
        assets: release
            .assets
            .into_iter()
            .map(|a| ReleaseAsset {
                name: a.name,
                download_url: a.browser_download_url,
                size_bytes: a.size,
            })
            .collect(),
    })
}

/// `true` if `candidate` is strictly newer than `current` (simple semver compare).
fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |s: &str| -> (u64, u64, u64) {
        let mut parts = s.split('.');
        let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let patch = parts
            .next()
            .and_then(|p| p.split('-').next())
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        (major, minor, patch)
    };
    parse(candidate) > parse(current)
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn version_compare() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        // pre-release suffix ignored in patch parse
        assert!(is_newer("0.2.0-beta", "0.1.0"));
    }
}
