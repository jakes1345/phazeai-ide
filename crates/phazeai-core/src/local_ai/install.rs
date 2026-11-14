//! Finding — or installing — llama.cpp's `llama-server`.
//!
//! Lookup order (first hit wins):
//! 1. `[local_ai] server_path` from config (an error if set but missing — the
//!    user asked for that exact binary);
//! 2. `llama-server` on `PATH` (winget, Homebrew, distro packages, own builds);
//! 3. a copy PhazeAI installed under `<data>/phazeai/llama.cpp/<tag>/`;
//! 4. otherwise [`install_latest`] downloads this platform's GPU build (Vulkan
//!    on Linux and Windows, Metal on Apple Silicon; the CPU build where there
//!    is none, or when asked for) from the newest numbered build (`b<N>`) on
//!    GitHub, verified against the
//!    SHA-256 `digest` GitHub publishes for each release asset. No digest → no
//!    install.
//!
//! The builds are found by listing releases, not through `releases/latest`:
//! since 2026-11-13 llama.cpp marks its `b<N>` builds as pre-releases, and
//! "latest" is a `v1.0.0` that holds no binaries.

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

/// The newest releases, pre-releases included (GitHub lists newest first).
pub const RELEASES_API: &str =
    "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=30";

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
    /// Which build PhazeAI installed; `None` for a binary the user supplied
    /// (config or `PATH`), whose build PhazeAI doesn't know.
    pub build: Option<Build>,
}

impl FoundServer {
    pub fn describe(&self) -> String {
        match &self.source {
            ServerSource::Config => format!("{} (from config)", self.path.display()),
            ServerSource::Path => format!("{} (on PATH)", self.path.display()),
            ServerSource::Installed(tag) => match self.build {
                Some(b) => format!("llama.cpp {tag}, {} build, installed by PhazeAI", b.label()),
                None => format!("llama.cpp {tag} installed by PhazeAI"),
            },
        }
    }
}

/// Which llama.cpp build an asset is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    /// Vulkan backend (AMD, Intel and NVIDIA GPUs on stock drivers) plus the
    /// CPU backends, loaded as separate libraries: with no usable Vulkan
    /// device it runs on the CPU like the CPU build.
    Vulkan,
    /// Apple Silicon builds, which always include Metal.
    Metal,
    Cpu,
}

impl Build {
    pub fn label(self) -> &'static str {
        match self {
            Build::Vulkan => "Vulkan GPU",
            Build::Metal => "Metal GPU",
            Build::Cpu => "CPU-only",
        }
    }

    pub fn uses_gpu(self) -> bool {
        !matches!(self, Build::Cpu)
    }

    /// Suffix of the install folder after the tag, so a GPU and a CPU build of
    /// the same release can sit side by side (`b12383-vulkan`, `b12383`).
    fn dir_suffix(self) -> &'static str {
        match self {
            Build::Vulkan => "-vulkan",
            Build::Metal | Build::Cpu => "",
        }
    }
}

/// Install folder name → (build number, build). Plain `b<N>` folders are the
/// platform's default build: Metal on Apple Silicon, the CPU build elsewhere
/// (all PhazeAI installed before GPU builds).
fn parse_install_dir(name: &str, os: &str, arch: &str) -> (u64, Build) {
    let (tag, build) = match name.strip_suffix(Build::Vulkan.dir_suffix()) {
        Some(tag) => (tag, Build::Vulkan),
        None if os == "macos" && arch == "aarch64" => (name, Build::Metal),
        None => (name, Build::Cpu),
    };
    (build_number(tag).unwrap_or(0), build)
}

/// Whether this platform has a GPU build PhazeAI can install.
pub fn gpu_build_available(os: &str, arch: &str) -> bool {
    asset_suffixes(os, arch).iter().any(|(_, b)| b.uses_gpu())
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
                build: None,
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
                    build: None,
                }));
            }
        }
    }
    Ok(find_installed(install_root))
}

/// The newest PhazeAI-installed copy under `install_root/<tag>[-vulkan]/`;
/// of two builds of the same release, the GPU one.
pub fn find_installed(install_root: &Path) -> Option<FoundServer> {
    find_installed_for(install_root, std::env::consts::OS, std::env::consts::ARCH)
}

fn find_installed_for(install_root: &Path, os: &str, arch: &str) -> Option<FoundServer> {
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
    // llama.cpp tags are `b<build number>`; newest = highest number, and the
    // GPU build first within one release.
    tags.sort_by_key(|(name, _)| {
        let (n, build) = parse_install_dir(name, os, arch);
        (n, build.uses_gpu(), name.clone())
    });
    tags.into_iter().rev().find_map(|(name, dir)| {
        let (_, build) = parse_install_dir(&name, os, arch);
        let tag = name
            .strip_suffix(build.dir_suffix())
            .unwrap_or(&name)
            .to_string();
        find_file(&dir, SERVER_EXE, 4).map(|path| FoundServer {
            path,
            source: ServerSource::Installed(tag),
            build: Some(build),
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    /// `"sha256:<hex>"` — GitHub computes this for every uploaded asset. Absent
    /// on old assets and some API mirrors.
    #[serde(default)]
    pub digest: Option<String>,
}

/// Name suffixes of a platform's builds, GPU first, in preference order.
/// llama.cpp has shipped both `.zip` and `.tar.gz` archives for Linux and macOS,
/// so both are accepted. CUDA builds are not offered: Vulkan runs on NVIDIA
/// cards too, without the CUDA runtime.
pub fn asset_suffixes(os: &str, arch: &str) -> &'static [(&'static str, Build)] {
    use Build::{Cpu, Metal, Vulkan};
    match (os, arch) {
        ("windows", "x86_64") => &[
            ("-bin-win-vulkan-x64.zip", Vulkan),
            ("-bin-win-cpu-x64.zip", Cpu),
        ],
        ("windows", "aarch64") => &[("-bin-win-cpu-arm64.zip", Cpu)],
        ("linux", "x86_64") => &[
            ("-bin-ubuntu-vulkan-x64.tar.gz", Vulkan),
            ("-bin-ubuntu-vulkan-x64.zip", Vulkan),
            ("-bin-ubuntu-x64.tar.gz", Cpu),
            ("-bin-ubuntu-x64.zip", Cpu),
        ],
        ("linux", "aarch64") => &[
            ("-bin-ubuntu-vulkan-arm64.tar.gz", Vulkan),
            ("-bin-ubuntu-vulkan-arm64.zip", Vulkan),
            ("-bin-ubuntu-arm64.tar.gz", Cpu),
            ("-bin-ubuntu-arm64.zip", Cpu),
        ],
        ("macos", "aarch64") => &[
            ("-bin-macos-arm64.tar.gz", Metal),
            ("-bin-macos-arm64.zip", Metal),
        ],
        ("macos", "x86_64") => &[("-bin-macos-x64.tar.gz", Cpu), ("-bin-macos-x64.zip", Cpu)],
        _ => &[],
    }
}

/// The release asset to install on `os`/`arch`, if the release has one: the
/// GPU build when `allow_gpu` and the release has it, else the CPU build.
pub fn select_asset<'a>(
    release: &'a Release,
    os: &str,
    arch: &str,
    allow_gpu: bool,
) -> Option<(&'a ReleaseAsset, Build)> {
    asset_suffixes(os, arch)
        .iter()
        .filter(|(_, build)| allow_gpu || !build.uses_gpu() || *build == Build::Metal)
        .find_map(|(suffix, build)| {
            release
                .assets
                .iter()
                .find(|a| a.name.starts_with("llama-") && a.name.ends_with(suffix))
                .map(|a| (a, *build))
        })
}

/// The build number of a llama.cpp release tag: `"b12383"` → `12383`. Any
/// other tag (`"v1.0.0"`, `"nightly"`) → `None`.
pub fn build_number(tag: &str) -> Option<u64> {
    let digits = tag.strip_prefix('b')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// A release endpoint's answer: the list (`/releases`) or one release
/// (`/releases/latest`, `/releases/tags/<tag>`, mirrors).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ReleaseAnswer {
    Many(Vec<Release>),
    One(Release),
}

impl ReleaseAnswer {
    fn into_vec(self) -> Vec<Release> {
        match self {
            ReleaseAnswer::Many(v) => v,
            ReleaseAnswer::One(r) => vec![r],
        }
    }
}

/// What [`choose_release`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum ReleaseChoice<'a> {
    /// The newest numbered build with this platform's asset and a SHA-256 digest.
    Ready {
        release: &'a Release,
        asset: &'a ReleaseAsset,
        build: Build,
        sha256: String,
    },
    /// Numbered builds have this platform's asset, but none has a digest; carries
    /// the newest such asset's name.
    NoDigest(String),
    /// No numbered build has an asset for this platform; carries the newest tag
    /// seen, if any.
    NoAsset(Option<String>),
}

/// Pick the release to install from `releases`: among `b<N>` tags with a build
/// for `os`/`arch` (the GPU build when `allow_gpu`, see [`select_asset`]), the
/// highest `N` whose asset carries a valid SHA-256 digest. Other tags
/// (`v1.0.0`, which holds only `nightly-tag.txt`) are skipped.
pub fn choose_release<'a>(
    releases: &'a [Release],
    os: &str,
    arch: &str,
    allow_gpu: bool,
) -> ReleaseChoice<'a> {
    let first = choose_with(releases, os, arch, allow_gpu);
    if !allow_gpu || matches!(first, ReleaseChoice::Ready { .. }) {
        return first;
    }
    // No verified GPU build anywhere: a verified CPU build beats nothing.
    match choose_with(releases, os, arch, false) {
        ready @ ReleaseChoice::Ready { .. } => ready,
        _ => first,
    }
}

fn choose_with<'a>(
    releases: &'a [Release],
    os: &str,
    arch: &str,
    allow_gpu: bool,
) -> ReleaseChoice<'a> {
    let mut numbered: Vec<(u64, &Release)> = releases
        .iter()
        .filter_map(|r| build_number(&r.tag_name).map(|n| (n, r)))
        .collect();
    numbered.sort_by(|a, b| b.0.cmp(&a.0));
    let with_asset: Vec<(&Release, &ReleaseAsset, Build)> = numbered
        .iter()
        .filter_map(|(_, r)| select_asset(r, os, arch, allow_gpu).map(|(a, b)| (*r, a, b)))
        .collect();
    let Some((_, newest_asset, _)) = with_asset.first() else {
        let newest = numbered
            .first()
            .map(|(_, r)| r.tag_name.clone())
            .or_else(|| releases.first().map(|r| r.tag_name.clone()));
        return ReleaseChoice::NoAsset(newest);
    };
    for (release, asset, build) in &with_asset {
        if let Some(sha256) = parse_sha256_digest(asset.digest.as_deref()) {
            return ReleaseChoice::Ready {
                release,
                asset,
                build: *build,
                sha256,
            };
        }
    }
    ReleaseChoice::NoDigest(newest_asset.name.clone())
}

/// `"sha256:<64 hex>"` → lower-case hex. Anything else (missing, other
/// algorithm, malformed) → `None`.
pub fn parse_sha256_digest(digest: Option<&str>) -> Option<String> {
    let hex = digest?.trim().strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// Download, verify and unpack the newest llama.cpp build into
/// `install_root/<tag>[-vulkan]/`; returns the installed server. With
/// `allow_gpu` it is this platform's GPU build where there is one. Without,
/// the CPU build, and PhazeAI's own GPU installs are removed so the CPU build
/// is the one that runs (the way back when a GPU build won't start).
pub async fn install_latest(
    install_root: &Path,
    allow_gpu: bool,
    cancel: Arc<AtomicBool>,
    on_progress: impl FnMut(DownloadProgress),
) -> Result<FoundServer, LocalAiError> {
    let client = super::download::http_client()?;
    install_from_release_api(
        &client,
        RELEASES_API,
        install_root,
        std::env::consts::OS,
        std::env::consts::ARCH,
        allow_gpu,
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
    allow_gpu: bool,
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
    let releases = resp.json::<ReleaseAnswer>().await?.into_vec();
    let (release, asset, build, sha) = match choose_release(&releases, os, arch, allow_gpu) {
        ReleaseChoice::Ready {
            release,
            asset,
            build,
            sha256,
        } => (release, asset, build, sha256),
        ReleaseChoice::NoDigest(name) => {
            return Err(LocalAiError::Other(format!(
                "GitHub did not publish a SHA-256 digest for {name}, so PhazeAI won't install \
                 it unverified. {}",
                manual_install_hint(os)
            )))
        }
        ReleaseChoice::NoAsset(newest) => {
            let seen = newest.map_or_else(
                || "GitHub listed no llama.cpp releases".to_string(),
                |tag| format!("no llama.cpp build up to {tag} has one"),
            );
            return Err(LocalAiError::Other(format!(
                "no llama.cpp build for {os}/{arch}: {seen}. {}",
                manual_install_hint(os)
            )));
        }
    };
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

    let dir_name = format!("{}{}", release.tag_name, build.dir_suffix());
    let final_dir = install_root.join(&dir_name);
    let tmp_dir = install_root.join(format!("{dir_name}.tmp"));
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
    if !allow_gpu {
        remove_gpu_installs(install_root);
    }
    let path = find_file(&final_dir, SERVER_EXE, 4)
        .ok_or_else(|| LocalAiError::Other(format!("{SERVER_EXE} vanished after install")))?;
    Ok(FoundServer {
        path,
        source: ServerSource::Installed(release.tag_name.clone()),
        build: Some(build),
    })
}

/// Delete PhazeAI's own `*-vulkan` installs, so [`find_installed`] picks the
/// CPU build. Only folders PhazeAI created under its install root.
fn remove_gpu_installs(install_root: &Path) {
    let Ok(entries) = std::fs::read_dir(install_root) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(Build::Vulkan.dir_suffix())
            && build_number(name.trim_end_matches(Build::Vulkan.dir_suffix())).is_some()
        {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
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
    fn selects_the_gpu_build_first_and_the_cpu_build_when_asked() {
        let r = release();
        let pick = |os, arch| select_asset(&r, os, arch, true).map(|(a, b)| (a.name.as_str(), b));
        assert_eq!(
            pick("linux", "x86_64"),
            Some(("llama-b6500-bin-ubuntu-vulkan-x64.zip", Build::Vulkan))
        );
        assert_eq!(
            pick("windows", "x86_64"),
            Some(("llama-b6500-bin-win-vulkan-x64.zip", Build::Vulkan))
        );
        // No GPU build for these: the CPU build.
        assert_eq!(
            pick("windows", "aarch64"),
            Some(("llama-b6500-bin-win-cpu-arm64.zip", Build::Cpu))
        );
        assert_eq!(
            pick("macos", "x86_64"),
            Some(("llama-b6500-bin-macos-x64.zip", Build::Cpu))
        );
        // Apple Silicon builds always carry Metal.
        assert_eq!(
            pick("macos", "aarch64"),
            Some(("llama-b6500-bin-macos-arm64.zip", Build::Metal))
        );
        assert!(gpu_build_available("linux", "x86_64"));
        assert!(gpu_build_available("macos", "aarch64"));
        assert!(!gpu_build_available("windows", "aarch64"));
    }

    #[test]
    fn selects_the_cpu_build_per_platform() {
        let r = release();
        let pick = |os, arch| select_asset(&r, os, arch, false).map(|(a, _)| a.name.as_str());
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
            select_asset(&r, "linux", "x86_64", false).map(|(a, _)| a.name.as_str()),
            Some("llama-b6500-bin-ubuntu-x64.zip")
        );
    }

    #[test]
    fn build_numbers() {
        assert_eq!(build_number("b12383"), Some(12383));
        assert_eq!(build_number("b1"), Some(1));
        for tag in ["v1.0.0", "b", "b12a", "B100", "nightly", "", "b-1"] {
            assert_eq!(build_number(tag), None, "{tag}");
        }
    }

    fn linux_build(tag: &str, digest: Option<&str>) -> Release {
        let name = format!("llama-{tag}-bin-ubuntu-x64.tar.gz");
        Release {
            tag_name: tag.into(),
            assets: vec![ReleaseAsset {
                browser_download_url: format!(
                    "https://github.com/ggml-org/llama.cpp/releases/download/{tag}/{name}"
                ),
                name,
                size: 1,
                digest: digest.map(str::to_string),
            }],
        }
    }

    /// The `/releases` list as GitHub returned it on 2026-11-14: a `v1.0.0`
    /// holding only `nightly-tag.txt` (what `releases/latest` points at) next
    /// to the `b<N>` pre-releases that carry the builds.
    #[test]
    fn skips_the_binaryless_v1_release_and_takes_the_newest_build() {
        let d = |c: char| format!("sha256:{}", c.to_string().repeat(64));
        let releases = vec![
            linux_build("b12381", Some(&d('a'))),
            Release {
                tag_name: "v1.0.0".into(),
                assets: vec![ReleaseAsset {
                    name: "nightly-tag.txt".into(),
                    browser_download_url: "https://example.invalid/nightly-tag.txt".into(),
                    size: 7,
                    digest: Some(d('f')),
                }],
            },
            linux_build("b12383", Some(&d('b'))),
            linux_build("b9999", Some(&d('c'))),
        ];
        match choose_release(&releases, "linux", "x86_64", true) {
            ReleaseChoice::Ready {
                release, sha256, ..
            } => {
                assert_eq!(release.tag_name, "b12383");
                assert_eq!(sha256, "b".repeat(64));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_undigested_newest_build_falls_back_to_the_next_verified_one() {
        let ok = format!("sha256:{}", "e".repeat(64));
        let releases = vec![linux_build("b200", None), linux_build("b199", Some(&ok))];
        assert!(matches!(
            choose_release(&releases, "linux", "x86_64", true),
            ReleaseChoice::Ready { release, .. } if release.tag_name == "b199"
        ));

        let none = vec![
            linux_build("b200", None),
            linux_build("b199", Some("md5:00")),
        ];
        assert_eq!(
            choose_release(&none, "linux", "x86_64", true),
            ReleaseChoice::NoDigest("llama-b200-bin-ubuntu-x64.tar.gz".into())
        );
    }

    #[test]
    fn no_build_for_the_platform_names_what_was_seen() {
        let releases = vec![linux_build("b300", None)];
        assert_eq!(
            choose_release(&releases, "windows", "riscv64", true),
            ReleaseChoice::NoAsset(Some("b300".into()))
        );
        let only_v1 = vec![Release {
            tag_name: "v1.0.0".into(),
            assets: vec![],
        }];
        assert_eq!(
            choose_release(&only_v1, "linux", "x86_64", true),
            ReleaseChoice::NoAsset(Some("v1.0.0".into()))
        );
        assert_eq!(
            choose_release(&[], "linux", "x86_64", true),
            ReleaseChoice::NoAsset(None)
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
    fn installed_gpu_build_wins_within_a_release_and_newer_builds_win_overall() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cpu = root.join("b1200").join(SERVER_EXE);
        let gpu = root.join("b1200-vulkan").join(SERVER_EXE);
        let old_gpu = root.join("b900-vulkan").join(SERVER_EXE);
        for p in [&cpu, &gpu, &old_gpu] {
            make_exe(p);
        }
        let found = find_installed_for(root, "linux", "x86_64").unwrap();
        assert_eq!(found.path, gpu);
        assert_eq!(found.source, ServerSource::Installed("b1200".into()));
        assert_eq!(found.build, Some(Build::Vulkan));
        assert!(
            found.describe().contains("b1200, Vulkan GPU build"),
            "{}",
            found.describe()
        );

        // The CPU fallback removes PhazeAI's GPU installs; the CPU build is left.
        remove_gpu_installs(root);
        assert!(!root.join("b1200-vulkan").exists() && !root.join("b900-vulkan").exists());
        let found = find_installed_for(root, "linux", "x86_64").unwrap();
        assert_eq!((found.path, found.build), (cpu, Some(Build::Cpu)));

        // A plain folder on Apple Silicon is the Metal build.
        assert_eq!(
            find_installed_for(root, "macos", "aarch64").unwrap().build,
            Some(Build::Metal)
        );
    }

    #[test]
    fn an_unverified_gpu_build_falls_back_to_a_verified_cpu_build() {
        let ok = format!("sha256:{}", "a".repeat(64));
        let mut r = linux_build("b500", Some(&ok)); // the CPU build, verified
        r.assets.push(ReleaseAsset {
            name: "llama-b500-bin-ubuntu-vulkan-x64.tar.gz".into(),
            browser_download_url: "https://example.invalid/vk.tar.gz".into(),
            size: 1,
            digest: None,
        });
        let releases = vec![r];
        match choose_release(&releases, "linux", "x86_64", true) {
            ReleaseChoice::Ready { asset, build, .. } => {
                assert_eq!(build, Build::Cpu);
                assert_eq!(asset.name, "llama-b500-bin-ubuntu-x64.tar.gz");
            }
            other => panic!("{other:?}"),
        }
        // Once verified, the GPU build is chosen.
        let mut releases = releases;
        releases[0].assets[1].digest = Some(format!("sha256:{}", "b".repeat(64)));
        assert!(matches!(
            choose_release(&releases, "linux", "x86_64", true),
            ReleaseChoice::Ready {
                build: Build::Vulkan,
                ..
            }
        ));
        assert!(matches!(
            choose_release(&releases, "linux", "x86_64", false),
            ReleaseChoice::Ready {
                build: Build::Cpu,
                ..
            }
        ));
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
            (
                // `/releases` shape: a binaryless `v1.0.0` first, then the build.
                "/list".to_string(),
                format!(
                    r#"[{{"tag_name":"v1.0.0","assets":[]}},{}]"#,
                    make_json(Some(&format!("sha256:{good}")))
                )
                .into_bytes(),
            ),
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
            true,
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
            true,
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
            true,
            cancel(),
            |_| {},
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("no llama.cpp build for windows/riscv64"),
            "{err}"
        );

        let found = install_from_release_api(
            &client,
            &format!("{base}/list"),
            &root,
            "linux",
            "x86_64",
            true,
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
