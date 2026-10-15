//! Finding — or installing — llama.cpp's `llama-server`.
//!
//! Lookup order (first hit wins):
//! 1. `[local_ai] server_path` from config (an error if set but missing — the
//!    user asked for that exact binary);
//! 2. `llama-server` on `PATH` (winget, Homebrew, distro packages, own builds);
//! 3. a copy PhazeAI installed under `<data>/phazeai/llama.cpp/<tag>/`;
//! 4. otherwise [`install_latest`] downloads the CPU build for this platform
//!    from the latest GitHub release, verified against the SHA-256 `digest`
//!    GitHub publishes for each release asset. No digest → no install.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Deserialize;

use super::download::{download_verified, DownloadProgress};
use super::LocalAiError;

/// File name of the server binary on this platform.
pub const SERVER_EXE: &str = if cfg!(windows) {
    "llama-server.exe"
} else {
    "llama-server"
};

pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerSource {
    Config,
    Path,
    /// Installed by PhazeAI; carries the release tag.
    Installed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundServer {
    pub path: PathBuf,
    pub source: ServerSource,
}

impl FoundServer {
    pub fn describe(&self) -> String {
        match &self.source {
            ServerSource::Config => format!("{} (from config)", self.path.display()),
            ServerSource::Path => format!("{} (on PATH)", self.path.display()),
            ServerSource::Installed(tag) => {
                format!("llama.cpp {tag} installed by PhazeAI")
            }
        }
    }
}

/// Locate `llama-server` without installing anything.
///
/// `path_env` is the value of `PATH` (passed in so tests can control it);
/// `install_root` is normally [`super::llama_cpp_dir`].
pub fn find_server_binary(
    config_path: Option<&Path>,
    path_env: Option<&OsStr>,
    install_root: &Path,
) -> Result<Option<FoundServer>, LocalAiError> {
    if let Some(p) = config_path.filter(|p| !p.as_os_str().is_empty()) {
        if is_executable(p) {
            return Ok(Some(FoundServer {
                path: p.to_path_buf(),
                source: ServerSource::Config,
            }));
        }
        return Err(LocalAiError::Other(format!(
            "[local_ai] server_path is set to {}, but that file doesn't exist or isn't executable.",
            p.display()
        )));
    }
    if let Some(path_env) = path_env {
        for dir in std::env::split_paths(path_env) {
            let candidate = dir.join(SERVER_EXE);
            if is_executable(&candidate) {
                return Ok(Some(FoundServer {
                    path: candidate,
                    source: ServerSource::Path,
                }));
            }
        }
    }
    Ok(find_installed(install_root))
}

/// The newest PhazeAI-installed copy under `install_root/<tag>/`.
pub fn find_installed(install_root: &Path) -> Option<FoundServer> {
    let mut tags: Vec<(String, PathBuf)> = std::fs::read_dir(install_root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Skip in-progress extractions and download caches.
            (!name.starts_with('.') && !name.ends_with(".tmp")).then(|| (name, e.path()))
        })
        .collect();
    // llama.cpp tags are `b<build number>`; newest = highest number.
    tags.sort_by_key(|(name, _)| {
        (
            name.trim_start_matches('b').parse::<u64>().unwrap_or(0),
            name.clone(),
        )
    });
    tags.into_iter().rev().find_map(|(tag, dir)| {
        find_file(&dir, SERVER_EXE, 4).map(|path| FoundServer {
            path,
            source: ServerSource::Installed(tag),
        })
    })
}

fn find_file(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let direct = dir.join(name);
    if is_executable(&direct) {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    subdirs.sort();
    subdirs
        .into_iter()
        .find_map(|d| find_file(&d, name, depth - 1))
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// What to tell a user who has to install llama.cpp by hand.
pub fn manual_install_hint(os: &str) -> String {
    let cmd = match os {
        "windows" => "winget install llama.cpp",
        "macos" => "brew install llama.cpp",
        _ => "brew install llama.cpp  (Homebrew on Linux), or build it from https://github.com/ggml-org/llama.cpp",
    };
    format!(
        "Install llama.cpp yourself with `{cmd}`, then restart PhazeAI — or set \
         `server_path` under `[local_ai]` in config.toml to your llama-server binary."
    )
}

// ── GitHub release metadata ─────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    /// `"sha256:<hex>"` — GitHub computes this for every uploaded asset. Absent
    /// on old assets and some API mirrors.
    #[serde(default)]
    pub digest: Option<String>,
}

/// Name suffixes of the CPU-only build for a platform, in preference order.
/// llama.cpp has shipped both `.zip` and `.tar.gz` archives for Linux and macOS,
/// so both are accepted.
pub fn asset_suffixes(os: &str, arch: &str) -> &'static [&'static str] {
    match (os, arch) {
        ("windows", "x86_64") => &["-bin-win-cpu-x64.zip"],
        ("windows", "aarch64") => &["-bin-win-cpu-arm64.zip"],
        ("linux", "x86_64") => &["-bin-ubuntu-x64.tar.gz", "-bin-ubuntu-x64.zip"],
        ("linux", "aarch64") => &["-bin-ubuntu-arm64.tar.gz", "-bin-ubuntu-arm64.zip"],
        ("macos", "aarch64") => &["-bin-macos-arm64.tar.gz", "-bin-macos-arm64.zip"],
        ("macos", "x86_64") => &["-bin-macos-x64.tar.gz", "-bin-macos-x64.zip"],
        _ => &[],
    }
}

/// The release asset to install on `os`/`arch`, if the release has one.
pub fn select_asset<'a>(release: &'a Release, os: &str, arch: &str) -> Option<&'a ReleaseAsset> {
    asset_suffixes(os, arch).iter().find_map(|suffix| {
        release
            .assets
            .iter()
            .find(|a| a.name.starts_with("llama-") && a.name.ends_with(suffix))
    })
}

/// `"sha256:<64 hex>"` → lower-case hex. Anything else (missing, other
/// algorithm, malformed) → `None`.
pub fn parse_sha256_digest(digest: Option<&str>) -> Option<String> {
    let hex = digest?.trim().strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// Download, verify and unpack the latest llama.cpp CPU build into
/// `install_root/<tag>/`; returns the installed server.
pub async fn install_latest(
    install_root: &Path,
    cancel: Arc<AtomicBool>,
    on_progress: impl FnMut(DownloadProgress),
) -> Result<FoundServer, LocalAiError> {
    let client = super::download::http_client()?;
    install_from_release_api(
        &client,
        LATEST_RELEASE_API,
        install_root,
        std::env::consts::OS,
        std::env::consts::ARCH,
        cancel,
        on_progress,
    )
    .await
}

/// [`install_latest`] against any GitHub-compatible release endpoint.
pub async fn install_from_release_api(
    client: &reqwest::Client,
    api_url: &str,
    install_root: &Path,
    os: &str,
    arch: &str,
    cancel: Arc<AtomicBool>,
    on_progress: impl FnMut(DownloadProgress),
) -> Result<FoundServer, LocalAiError> {
    let resp = client
        .get(api_url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| {
            LocalAiError::Other(format!(
                "could not reach GitHub to download llama.cpp ({e}). {}",
                manual_install_hint(os)
            ))
        })?;
    if !resp.status().is_success() {
        return Err(LocalAiError::Other(format!(
            "GitHub answered HTTP {} for the llama.cpp release list. {}",
            resp.status(),
            manual_install_hint(os)
        )));
    }
    let release: Release = resp.json().await?;
    let asset = select_asset(&release, os, arch).ok_or_else(|| {
        LocalAiError::Other(format!(
            "llama.cpp {} has no CPU build for {os}/{arch}. {}",
            release.tag_name,
            manual_install_hint(os)
        ))
    })?;
    let sha = parse_sha256_digest(asset.digest.as_deref()).ok_or_else(|| {
        LocalAiError::Other(format!(
            "GitHub did not publish a SHA-256 digest for {}, so PhazeAI won't install it \
             unverified. {}",
            asset.name,
            manual_install_hint(os)
        ))
    })?;
    if release.tag_name.is_empty()
        || release
            .tag_name
            .contains(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
    {
        return Err(LocalAiError::Other(format!(
            "unexpected release tag {:?}",
            release.tag_name
        )));
    }

    let cache = install_root.join(".downloads");
    let archive = cache.join(&asset.name);
    download_verified(
        client,
        &asset.browser_download_url,
        &archive,
        asset.size,
        &sha,
        cancel,
        on_progress,
    )
    .await?;

    let final_dir = install_root.join(&release.tag_name);
    let tmp_dir = install_root.join(format!("{}.tmp", release.tag_name));
    let archive2 = archive.clone();
    let tmp2 = tmp_dir.clone();
    tokio::task::spawn_blocking(move || -> Result<(), LocalAiError> {
        if tmp2.exists() {
            std::fs::remove_dir_all(&tmp2)?;
        }
        std::fs::create_dir_all(&tmp2)?;
        extract_archive(&archive2, &tmp2)
    })
    .await
    .map_err(|e| LocalAiError::Other(format!("extraction task failed: {e}")))??;

    let Some(bin) = find_file(&tmp_dir, SERVER_EXE, 4) else {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(LocalAiError::Other(format!(
            "{} did not contain {SERVER_EXE}. {}",
            asset.name,
            manual_install_hint(os)
        )));
    };
    drop(bin);
    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir)?;
    }
    std::fs::rename(&tmp_dir, &final_dir)?;
    let _ = std::fs::remove_file(&archive);
    let path = find_file(&final_dir, SERVER_EXE, 4)
        .ok_or_else(|| LocalAiError::Other(format!("{SERVER_EXE} vanished after install")))?;
    Ok(FoundServer {
        path,
        source: ServerSource::Installed(release.tag_name),
    })
}

/// Unpack a `.zip` or `.tar.gz` into `dest`. Entries that would escape `dest`
/// are skipped (zip) or rejected (tar). On Unix the server binary is made
/// executable even if the archive lost its mode bits.
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<(), LocalAiError> {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".zip") {
        extract_zip(archive, dest)?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let f = std::fs::File::open(archive)?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(f));
        tar.set_preserve_permissions(true);
        tar.unpack(dest)?;
    } else {
        return Err(LocalAiError::Other(format!(
            "don't know how to unpack {}",
            archive.display()
        )));
    }
    #[cfg(unix)]
    if let Some(bin) = find_file_any(dest, SERVER_EXE, 4) {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&bin)?.permissions();
        perm.set_mode(perm.mode() | 0o755);
        std::fs::set_permissions(&bin, perm)?;
    }
    Ok(())
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), LocalAiError> {
    let f = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(f)
        .map_err(|e| LocalAiError::Other(format!("bad zip {}: {e}", archive.display())))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| LocalAiError::Other(format!("bad zip entry: {e}")))?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut w = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut w)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

/// Like `find_file` but ignores the executable bit (used before it is set).
#[cfg(unix)]
fn find_file_any(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .find_map(|e| find_file_any(&e.path(), name, depth - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Shaped like `GET /repos/ggml-org/llama.cpp/releases/latest` (trimmed to
    /// the fields we read plus a few we ignore). Names follow llama.cpp's
    /// release naming; the digests are made up for the parser test.
    const RELEASE_JSON: &str = r#"{
      "url": "https://api.github.com/repos/ggml-org/llama.cpp/releases/1",
      "tag_name": "b6500",
      "name": "b6500",
      "draft": false,
      "prerelease": false,
      "assets": [
        {"name": "cudart-llama-bin-win-cuda-12.4-x64.zip", "size": 10, "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/cudart-llama-bin-win-cuda-12.4-x64.zip"},
        {"name": "llama-b6500-bin-macos-arm64.zip", "size": 11, "digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/llama-b6500-bin-macos-arm64.zip"},
        {"name": "llama-b6500-bin-macos-x64.zip", "size": 12, "digest": null,
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/llama-b6500-bin-macos-x64.zip"},
        {"name": "llama-b6500-bin-ubuntu-x64.tar.gz", "size": 13, "digest": "sha256:ABCDEFabcdef0000000000000000000000000000000000000000000000000000",
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/llama-b6500-bin-ubuntu-x64.tar.gz"},
        {"name": "llama-b6500-bin-ubuntu-x64.zip", "size": 14, "digest": "sha256:4444444444444444444444444444444444444444444444444444444444444444",
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/llama-b6500-bin-ubuntu-x64.zip"},
        {"name": "llama-b6500-bin-ubuntu-vulkan-x64.zip", "size": 15, "digest": "sha256:5555555555555555555555555555555555555555555555555555555555555555",
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/llama-b6500-bin-ubuntu-vulkan-x64.zip"},
        {"name": "llama-b6500-bin-win-cpu-arm64.zip", "size": 16, "digest": "sha256:6666666666666666666666666666666666666666666666666666666666666666",
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/llama-b6500-bin-win-cpu-arm64.zip"},
        {"name": "llama-b6500-bin-win-cpu-x64.zip", "size": 17, "digest": "sha256:7777777777777777777777777777777777777777777777777777777777777777",
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/llama-b6500-bin-win-cpu-x64.zip"},
        {"name": "llama-b6500-bin-win-cuda-12.4-x64.zip", "size": 18, "digest": "sha256:8888888888888888888888888888888888888888888888888888888888888888",
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/llama-b6500-bin-win-cuda-12.4-x64.zip"},
        {"name": "llama-b6500-bin-win-vulkan-x64.zip", "size": 19,
         "browser_download_url": "https://github.com/ggml-org/llama.cpp/releases/download/b6500/llama-b6500-bin-win-vulkan-x64.zip"}
      ]
    }"#;

    fn release() -> Release {
        serde_json::from_str(RELEASE_JSON).unwrap()
    }

    #[test]
    fn selects_the_cpu_build_per_platform() {
        let r = release();
        let pick = |os, arch| select_asset(&r, os, arch).map(|a| a.name.as_str());
        assert_eq!(
            pick("windows", "x86_64"),
            Some("llama-b6500-bin-win-cpu-x64.zip")
        );
        assert_eq!(
            pick("windows", "aarch64"),
            Some("llama-b6500-bin-win-cpu-arm64.zip")
        );
        // tar.gz preferred when both exist; never the Vulkan build.
        assert_eq!(
            pick("linux", "x86_64"),
            Some("llama-b6500-bin-ubuntu-x64.tar.gz")
        );
        assert_eq!(
            pick("macos", "aarch64"),
            Some("llama-b6500-bin-macos-arm64.zip")
        );
        assert_eq!(
            pick("macos", "x86_64"),
            Some("llama-b6500-bin-macos-x64.zip")
        );
        assert_eq!(pick("linux", "aarch64"), None);
        assert_eq!(pick("freebsd", "x86_64"), None);
    }

    #[test]
    fn zip_only_linux_release_still_resolves() {
        let mut r = release();
        r.assets.retain(|a| !a.name.ends_with(".tar.gz"));
        assert_eq!(
            select_asset(&r, "linux", "x86_64").map(|a| a.name.as_str()),
            Some("llama-b6500-bin-ubuntu-x64.zip")
        );
    }

    #[test]
    fn digest_parsing() {
        let r = release();
        let get = |name: &str| {
            let a = r.assets.iter().find(|a| a.name == name).unwrap();
            parse_sha256_digest(a.digest.as_deref())
        };
        assert_eq!(
            get("llama-b6500-bin-win-cpu-x64.zip").unwrap(),
            "7".repeat(64)
        );
        assert_eq!(
            get("llama-b6500-bin-ubuntu-x64.tar.gz").unwrap(),
            "abcdefabcdef0000000000000000000000000000000000000000000000000000"
        );
        // null and missing digests → refuse.
        assert_eq!(get("llama-b6500-bin-macos-x64.zip"), None);
        assert_eq!(get("llama-b6500-bin-win-vulkan-x64.zip"), None);
        assert_eq!(parse_sha256_digest(Some("sha512:abcd")), None);
        assert_eq!(parse_sha256_digest(Some("sha256:abcd")), None);
        assert_eq!(
            parse_sha256_digest(Some(&format!("sha256:{}", "g".repeat(64)))),
            None
        );
    }

    #[test]
    fn manual_hints_name_real_commands() {
        assert!(manual_install_hint("windows").contains("winget install llama.cpp"));
        assert!(manual_install_hint("macos").contains("brew install llama.cpp"));
        assert!(manual_install_hint("linux").contains("server_path"));
    }

    fn make_exe(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn discovery_order_config_then_path_then_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_bin = tmp.path().join("custom").join(SERVER_EXE);
        let path_dir = tmp.path().join("bin");
        let path_bin = path_dir.join(SERVER_EXE);
        let root = tmp.path().join("llama.cpp");
        let old = root.join("b900").join("build/bin").join(SERVER_EXE);
        let new = root.join("b1200").join(SERVER_EXE);
        for p in [&cfg_bin, &path_bin, &old, &new] {
            make_exe(p);
        }
        let empty_dir = tmp.path().join("empty");
        std::fs::create_dir_all(&empty_dir).unwrap();
        let path_env = std::env::join_paths([empty_dir.clone(), path_dir.clone()]).unwrap();

        let found = find_server_binary(Some(&cfg_bin), Some(&path_env), &root)
            .unwrap()
            .unwrap();
        assert_eq!(found.source, ServerSource::Config);
        assert_eq!(found.path, cfg_bin);

        let found = find_server_binary(None, Some(&path_env), &root)
            .unwrap()
            .unwrap();
        assert_eq!(found.source, ServerSource::Path);
        assert_eq!(found.path, path_bin);

        // Without PATH hits, the newest installed tag wins (b1200 > b900 numerically).
        let found = find_server_binary(None, Some(empty_dir.as_os_str()), &root)
            .unwrap()
            .unwrap();
        assert_eq!(found.source, ServerSource::Installed("b1200".into()));
        assert_eq!(found.path, new);

        std::fs::remove_dir_all(root.join("b1200")).unwrap();
        let found = find_server_binary(None, None, &root).unwrap().unwrap();
        assert_eq!(found.path, old);

        // Nothing anywhere.
        assert_eq!(
            find_server_binary(None, None, &tmp.path().join("none")).unwrap(),
            None
        );
    }

    #[test]
    fn missing_configured_binary_is_an_error_not_a_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let err = find_server_binary(Some(&tmp.path().join("nope")), None, tmp.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("server_path"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_files_on_path_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(SERVER_EXE), b"not exec").unwrap();
        let found =
            find_server_binary(None, Some(tmp.path().as_os_str()), &tmp.path().join("x")).unwrap();
        assert_eq!(found, None);
    }

    #[test]
    fn in_progress_extractions_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        make_exe(&tmp.path().join("b5.tmp").join(SERVER_EXE));
        assert_eq!(find_installed(tmp.path()), None);
    }

    fn build_zip(path: &Path, with_traversal: bool) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o644);
        z.start_file(format!("build/bin/{SERVER_EXE}"), opts)
            .unwrap();
        z.write_all(b"binary").unwrap();
        z.start_file("build/bin/libllama.so", opts).unwrap();
        z.write_all(b"lib").unwrap();
        if with_traversal {
            z.start_file("../escape.txt", opts).unwrap();
            z.write_all(b"bad").unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn zip_extraction_sets_exec_bit_and_blocks_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("llama-b1-bin-ubuntu-x64.zip");
        build_zip(&archive, true);
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        extract_archive(&archive, &dest).unwrap();
        assert!(!tmp.path().join("escape.txt").exists());
        let found = find_installed(tmp.path());
        let bin = dest.join("build/bin").join(SERVER_EXE);
        assert_eq!(std::fs::read(&bin).unwrap(), b"binary");
        assert!(is_executable(&bin));
        assert!(found.is_some());
    }

    #[test]
    fn tar_gz_extraction() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("llama-b1-bin-ubuntu-x64.tar.gz");
        {
            let f = std::fs::File::create(&archive).unwrap();
            let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            let mut h = tar::Header::new_gnu();
            h.set_size(6);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, format!("llama-b1/{SERVER_EXE}"), &b"binary"[..])
                .unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        extract_archive(&archive, &dest).unwrap();
        let bin = dest.join("llama-b1").join(SERVER_EXE);
        assert!(is_executable(&bin));
    }

    /// Installs from a local HTTP server that mimics the GitHub API + asset
    /// download, exercising the whole pipeline: JSON → asset pick → verified
    /// download → extract → discover.
    #[tokio::test]
    async fn install_pipeline_against_local_server() {
        use sha2::Digest;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("asset.zip");
        build_zip(&zip_path, false);
        let zip_bytes = std::fs::read(&zip_path).unwrap();
        let good = super::super::download::hex(&sha2::Sha256::digest(&zip_bytes));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let asset_name = "llama-b7000-bin-ubuntu-x64.zip";
        let make_json = |digest: Option<&str>| {
            serde_json::json!({
                "tag_name": "b7000",
                "assets": [{
                    "name": asset_name,
                    "size": zip_bytes.len(),
                    "digest": digest,
                    "browser_download_url": format!("{base}/dl/{asset_name}"),
                }]
            })
            .to_string()
        };
        let routes = std::collections::HashMap::from([
            (
                "/good".to_string(),
                make_json(Some(&format!("sha256:{good}"))).into_bytes(),
            ),
            (
                "/bad".to_string(),
                make_json(Some(&format!("sha256:{}", "0".repeat(64)))).into_bytes(),
            ),
            ("/nodigest".to_string(), make_json(None).into_bytes()),
            (format!("/dl/{asset_name}"), zip_bytes.clone()),
        ]);
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = match routes.get(&path) {
                    Some(b) => ("200 OK", b.clone()),
                    None => ("404 Not Found", Vec::new()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
                let _ = s.shutdown().await;
            }
        });

        let client = reqwest::Client::new();
        let root = tmp.path().join("llama.cpp");
        let cancel = || Arc::new(AtomicBool::new(false));

        let err = install_from_release_api(
            &client,
            &format!("{base}/nodigest"),
            &root,
            "linux",
            "x86_64",
            cancel(),
            |_| {},
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("digest") && err.contains("server_path"),
            "{err}"
        );

        let err = install_from_release_api(
            &client,
            &format!("{base}/bad"),
            &root,
            "linux",
            "x86_64",
            cancel(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LocalAiError::HashMismatch { .. }), "{err:?}");
        assert!(!root.join("b7000").exists());

        let err = install_from_release_api(
            &client,
            &format!("{base}/good"),
            &root,
            "windows",
            "riscv64",
            cancel(),
            |_| {},
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("no CPU build"), "{err}");

        let found = install_from_release_api(
            &client,
            &format!("{base}/good"),
            &root,
            "linux",
            "x86_64",
            cancel(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(found.source, ServerSource::Installed("b7000".into()));
        assert_eq!(found.path, root.join("b7000/build/bin").join(SERVER_EXE));
        assert!(is_executable(&found.path));
        assert!(!root.join(".downloads").join(asset_name).exists());
        assert_eq!(find_installed(&root), Some(found));
    }
}
