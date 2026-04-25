use serde::{Deserialize, Serialize};

/// Returns the browser URL for OAuth sign-in.
pub fn login_url() -> &'static str {
    "https://app.phazeai.com/signin"
}

/// Keyring entry name used for the cloud API token. Reuses the existing
/// `phazeai` keyring service so the token sits next to provider API keys
/// (macOS Keychain / Linux Secret Service / Windows Credential Manager).
const TOKEN_KEYRING_ENTRY: &str = "cloud_api_token";

/// Cloud credentials. The email and other non-secret metadata are persisted
/// to `~/.config/phazeai/cloud.toml`. The API token is stored in the OS
/// keyring rather than plaintext on disk; the toml file holds only a marker
/// indicating that a token exists, so prior installs that wrote a plaintext
/// token are migrated transparently the next time `load` runs.
#[derive(Debug, Clone, Default)]
pub struct CloudCredentials {
    pub email: Option<String>,
    /// API token from https://app.phazeai.com/settings/tokens. Sourced from
    /// the OS keyring at load time; never serialised to disk.
    pub api_token: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CloudCredentialsOnDisk {
    email: Option<String>,
    /// Legacy field — older installs persisted the token here in plaintext.
    /// Reading it triggers a one-time migration into the OS keyring; the
    /// field is wiped from disk on the next save().
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_token: Option<String>,
}

impl CloudCredentials {
    pub fn load() -> Self {
        let path = credentials_path();
        let on_disk: CloudCredentialsOnDisk = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();

        // Token resolution order:
        //  1. OS keyring (the secure default).
        //  2. Legacy plaintext field on disk — migrate into keyring, then
        //     re-save without it so disk no longer holds the secret.
        let mut api_token = phazeai_core::llm::provider::keyring_get(TOKEN_KEYRING_ENTRY);
        if api_token.is_none() {
            if let Some(legacy) = on_disk.api_token.clone() {
                if !legacy.is_empty() {
                    if let Err(e) =
                        phazeai_core::llm::provider::keyring_set(TOKEN_KEYRING_ENTRY, &legacy)
                    {
                        // Keyring unavailable (headless CI, etc.) — fall back to
                        // returning the token in memory but do not rewrite disk.
                        tracing::warn!(
                            target: "phazeai_cloud::auth",
                            error = %e,
                            "keyring unavailable; cloud token remains in plaintext on disk"
                        );
                        api_token = Some(legacy);
                    } else {
                        api_token = Some(legacy);
                        // Re-save without the plaintext token now that it's in
                        // the keyring. Errors are non-fatal.
                        let migrated = Self {
                            email: on_disk.email.clone(),
                            api_token: api_token.clone(),
                        };
                        let _ = migrated.save();
                    }
                }
            }
        }

        Self {
            email: on_disk.email,
            api_token,
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        // Keyring write first; if it fails we surface the error rather than
        // silently downgrading to plaintext.
        if let Some(token) = self.api_token.as_deref() {
            if !token.is_empty() {
                phazeai_core::llm::provider::keyring_set(TOKEN_KEYRING_ENTRY, token)
                    .map_err(|e| anyhow::anyhow!("failed to store cloud token in keyring: {e}"))?;
            }
        } else {
            // Best-effort clear if the caller nulled the token.
            let _ = phazeai_core::llm::provider::keyring_delete(TOKEN_KEYRING_ENTRY);
        }

        let on_disk = CloudCredentialsOnDisk {
            email: self.email.clone(),
            api_token: None,
        };
        let path = credentials_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(&on_disk)?)?;
        Ok(())
    }

    pub fn is_authenticated(&self) -> bool {
        self.api_token
            .as_ref()
            .map(|t| !t.is_empty())
            .unwrap_or(false)
    }

    /// Clear the stored token (keyring + disk) and persist the change.
    pub fn logout(&mut self) -> anyhow::Result<()> {
        self.api_token = None;
        self.email = None;
        self.save()
    }
}

fn credentials_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("phazeai")
        .join("cloud.toml")
}

/// Live session with the PhazeAI Cloud backend.
#[derive(Debug, Clone)]
pub struct CloudSession {
    pub email: String,
    pub token: String,
    pub tier: crate::subscription::Tier,
    /// Remaining AI credits for this billing period (tokens).
    pub credits_remaining: u64,
}
