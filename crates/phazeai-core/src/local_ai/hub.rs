//! Any GGUF model on Hugging Face, sized to this machine.
//!
//! The curated [`super::catalog`] holds a few tested models. This module
//! opens the rest of the Hub: [`search`] finds GGUF repositories, [`listing`]
//! lists a repository's quantizations at a pinned commit with their exact
//! sizes and SHA-256s, [`choose`] picks the best one that fits this
//! machine's RAM, and [`pull`] downloads it (all shards of a split model),
//! verifies every byte, reads the GGUF header for the real KV-cache size and
//! records it, so the built-in server can run it like a catalog model.
//!
//! Installed Hub models live in `<models>/hub/<owner>__<repo>/`, each with a
//! `<file>.phaze.json` manifest. Their id is `hf:<owner>/<repo>:<file>`.
//! `HF_ENDPOINT` points at a mirror, and `HF_TOKEN` is sent for gated models.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::catalog::{fit_need, FitReport, COMPUTE_OVERHEAD, DEFAULT_CTX_SIZE, OS_IDE_RESERVE};
use super::download::{download_verified, DownloadProgress};
use super::hardware::{HardwareInfo, GIB};
use super::LocalAiError;

/// Prefix of a Hub model's id.
pub const ID_PREFIX: &str = "hf:";

/// The Hub's address: `HF_ENDPOINT` when set, else huggingface.co.
pub fn endpoint() -> String {
    std::env::var("HF_ENDPOINT")
        .ok()
        .map(|e| e.trim().trim_end_matches('/').to_string())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "https://huggingface.co".into())
}

fn token() -> Option<String> {
    std::env::var("HF_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Whether `endpoint` is on this machine, so a proxy can't reach it.
fn is_loopback(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .is_some_and(|h| {
            h == "localhost"
                || h.trim_matches(|c| c == '[' || c == ']')
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        })
}

/// A connection to the Hub (or a mirror).
#[derive(Debug, Clone)]
pub struct Hub {
    client: reqwest::Client,
    base: String,
}

impl Hub {
    /// The Hub at [`endpoint`], sending `HF_TOKEN` when set.
    pub fn from_env() -> Result<Self, LocalAiError> {
        Self::new(&endpoint(), token().as_deref())
    }

    /// The Hub at `base`. `token` is sent on every request; reqwest drops it
    /// when a download redirects to another host (the Hub's file CDN).
    pub fn new(base: &str, token: Option<&str>) -> Result<Self, LocalAiError> {
        let base = base.trim().trim_end_matches('/').to_string();
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(t) = token {
            let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {t}"))
                .map_err(|_| {
                    LocalAiError::Other("HF_TOKEN has characters a header can't hold".into())
                })?;
            value.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        let mut builder = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(60))
            .user_agent(concat!("PhazeAI/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers);
        if is_loopback(&base) {
            builder = builder.no_proxy();
        }
        Ok(Self {
            client: builder.build()?,
            base,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<(T, Option<String>), LocalAiError> {
        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(LocalAiError::Other(
                "Hugging Face refused access: the model may be gated or private. Accept its \
                 terms on huggingface.co and set HF_TOKEN."
                    .into(),
            ));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(LocalAiError::Other("not found on Hugging Face".into()));
        }
        if !status.is_success() {
            return Err(LocalAiError::Other(format!(
                "Hugging Face answered {status}"
            )));
        }
        let next = resp
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|v| v.to_str().ok())
            .and_then(next_link);
        Ok((resp.json().await?, next))
    }
}

/// The `rel="next"` address in a `Link` header.
fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (url, rest) = part.split_once(';')?;
        rest.contains("rel=\"next\"").then(|| {
            url.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
    })
}

/// A repository found by [`search`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubRepo {
    pub id: String,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub likes: u64,
}

/// GGUF repositories matching `query`, most downloaded first.
pub async fn search(hub: &Hub, query: &str, limit: usize) -> Result<Vec<HubRepo>, LocalAiError> {
    let url = reqwest::Url::parse_with_params(
        &format!("{}/api/models", hub.base),
        &[
            ("search", query.trim()),
            ("filter", "gguf"),
            ("sort", "downloads"),
            ("direction", "-1"),
            ("limit", &limit.clamp(1, 100).to_string()),
        ],
    )
    .map_err(|e| LocalAiError::Other(e.to_string()))?;
    let (repos, _) = hub.get_json::<Vec<HubRepo>>(url.as_str()).await?;
    Ok(repos)
}

/// One downloadable file of a model (a shard of a split model, or the whole).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubShard {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// One quantization of a model: a single GGUF, or the shards of a split one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubFile {
    /// The first shard's path, which names this quantization.
    pub path: String,
    /// `Q4_K_M`, `Q8_0`, `IQ4_XS`, `F16`, ... when the name says.
    pub quant: Option<String>,
    pub shards: Vec<HubShard>,
}

impl HubFile {
    pub fn size(&self) -> u64 {
        self.shards.iter().map(|s| s.size).sum()
    }
}

/// A repository's GGUF files at one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubListing {
    pub repo: String,
    /// The commit everything is pinned to.
    pub revision: String,
    pub files: Vec<HubFile>,
    /// Parameter count, when the Hub knows it.
    pub params: Option<u64>,
    /// Trained context length, when the Hub knows it.
    pub context_length: Option<u64>,
    pub license: Option<String>,
}

#[derive(Deserialize)]
struct ModelInfo {
    sha: Option<String>,
    #[serde(default)]
    gguf: Option<GgufInfo>,
    #[serde(default, rename = "cardData")]
    card_data: Option<CardData>,
}

#[derive(Deserialize)]
struct GgufInfo {
    total: Option<u64>,
    context_length: Option<u64>,
}

#[derive(Deserialize)]
struct CardData {
    license: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct TreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    lfs: Option<Lfs>,
}

#[derive(Deserialize)]
struct Lfs {
    oid: String,
    size: u64,
}

/// A repository name is `owner/name` with only the characters the Hub allows.
pub fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!((parts.next(), parts.next(), parts.next()), (Some(o), Some(n), None) if ok(o) && ok(n))
}

/// The GGUF files of `repo` at its latest commit.
pub async fn listing(hub: &Hub, repo: &str) -> Result<HubListing, LocalAiError> {
    if !valid_repo(repo) {
        return Err(LocalAiError::Other(format!(
            "\"{repo}\" isn't a Hugging Face repository name (owner/name)"
        )));
    }
    let base = &hub.base;
    let (info, _) = hub
        .get_json::<ModelInfo>(&format!("{base}/api/models/{repo}"))
        .await?;
    let revision = info
        .sha
        .filter(|s| s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| LocalAiError::Other("Hugging Face gave no commit for it".into()))?;
    let mut entries: Vec<TreeEntry> = Vec::new();
    let mut url = Some(format!(
        "{base}/api/models/{repo}/tree/{revision}?recursive=true"
    ));
    let mut pages = 0;
    while let Some(u) = url.take() {
        let (page, next) = hub.get_json::<Vec<TreeEntry>>(&u).await?;
        entries.extend(page);
        pages += 1;
        // Follow the Hub's own next-page link, on the Hub only.
        url = next.filter(|n| n.starts_with(&format!("{base}/")) && pages < 50);
    }
    let license = info
        .card_data
        .and_then(|c| c.license)
        .and_then(|l| match l {
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Array(a) => a.first().and_then(|v| v.as_str()).map(str::to_string),
            _ => None,
        });
    Ok(HubListing {
        repo: repo.to_string(),
        revision,
        files: group_files(entries),
        params: info.gguf.as_ref().and_then(|g| g.total),
        context_length: info.gguf.as_ref().and_then(|g| g.context_length),
        license,
    })
}

/// `name-00002-of-00003.gguf` → (`name`, 2, 3).
fn split_part(path: &str) -> Option<(&str, u32, u32)> {
    let stem = path.strip_suffix(".gguf")?;
    let (rest, total) = stem.rsplit_once("-of-")?;
    let (prefix, index) = rest.rsplit_once('-')?;
    if index.len() != 5 || total.len() != 5 {
        return None;
    }
    Some((prefix, index.parse().ok()?, total.parse().ok()?))
}

/// The model files of a repository tree: GGUFs only, vision projectors left
/// out, split models grouped (and only complete ones kept), smallest first.
fn group_files(entries: Vec<TreeEntry>) -> Vec<HubFile> {
    use std::collections::BTreeMap;
    let mut single = Vec::new();
    let mut split: BTreeMap<String, Vec<(u32, u32, HubShard)>> = BTreeMap::new();
    for e in entries {
        if e.kind != "file" || !e.path.to_ascii_lowercase().ends_with(".gguf") {
            continue;
        }
        let name = e
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&e.path)
            .to_ascii_lowercase();
        if name.contains("mmproj") || name.contains("imatrix") {
            continue;
        }
        if rel_path(&e.path).is_none() {
            continue;
        }
        let Some(lfs) = e.lfs else { continue };
        if lfs.oid.len() != 64 || !lfs.oid.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let shard = HubShard {
            path: e.path.clone(),
            size: if lfs.size > 0 { lfs.size } else { e.size },
            sha256: lfs.oid.to_ascii_lowercase(),
        };
        match split_part(&e.path) {
            Some((prefix, i, n)) => split
                .entry(prefix.to_string())
                .or_default()
                .push((i, n, shard)),
            None => single.push(HubFile {
                quant: quant_of(&e.path),
                path: e.path,
                shards: vec![shard],
            }),
        }
    }
    for (_, mut parts) in split {
        parts.sort_by_key(|(i, _, _)| *i);
        let n = parts[0].1;
        let complete = parts.len() as u32 == n
            && parts
                .iter()
                .enumerate()
                .all(|(k, (i, t, _))| *i == k as u32 + 1 && *t == n);
        if !complete {
            continue;
        }
        let shards: Vec<HubShard> = parts.into_iter().map(|(_, _, s)| s).collect();
        single.push(HubFile {
            path: shards[0].path.clone(),
            quant: quant_of(&shards[0].path),
            shards,
        });
    }
    single.sort_by_key(|f| f.size());
    single
}

/// The quantization a file name states, e.g. `Q4_K_M`.
pub fn quant_of(path: &str) -> Option<String> {
    const QUANTS: &[&str] = &[
        "IQ1_S", "IQ1_M", "IQ2_XXS", "IQ2_XS", "IQ2_S", "IQ2_M", "IQ3_XXS", "IQ3_XS", "IQ3_S",
        "IQ3_M", "IQ4_XS", "IQ4_NL", "Q2_K_S", "Q2_K_L", "Q2_K", "Q3_K_S", "Q3_K_M", "Q3_K_L",
        "Q3_K_XL", "Q4_0_4_4", "Q4_0_4_8", "Q4_0_8_8", "Q4_0", "Q4_1", "Q4_K_S", "Q4_K_M",
        "Q4_K_L", "Q4_K_XL", "Q5_0", "Q5_1", "Q5_K_S", "Q5_K_M", "Q5_K_L", "Q5_K_XL", "Q6_K_L",
        "Q6_K_XL", "Q6_K", "Q8_0", "Q8_K_XL", "BF16", "F16", "F32", "MXFP4",
    ];
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_uppercase();
    let stem = name.strip_suffix(".GGUF").unwrap_or(&name);
    let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
    QUANTS
        .iter()
        .filter(|q| {
            stem.match_indices(*q).any(|(i, _)| {
                boundary(stem[..i].chars().last()) && boundary(stem[i + q.len()..].chars().next())
            })
        })
        .max_by_key(|q| q.len())
        .map(|q| q.to_string())
}

/// Rough bits per weight of a quantization, for guessing a parameter count.
fn bits_of(quant: &str) -> f64 {
    let q = quant.to_ascii_uppercase();
    match q.as_str() {
        "F32" => 32.0,
        "F16" | "BF16" => 16.0,
        "Q8_0" | "Q8_K_XL" => 8.5,
        _ if q.starts_with("Q6") => 6.6,
        _ if q.starts_with("Q5") => 5.7,
        _ if q.starts_with("Q4") || q.starts_with("IQ4") || q == "MXFP4" => 4.8,
        _ if q.starts_with("Q3") || q.starts_with("IQ3") => 3.9,
        _ if q.starts_with("Q2") || q.starts_with("IQ2") => 2.9,
        _ if q.starts_with("IQ1") => 1.9,
        _ => 4.8,
    }
}

/// KV-cache bytes per token before the file is downloaded: from the Hub's
/// parameter count, else from the file's size and quantization. About 16
/// KiB per billion parameters covers the grouped-query models GGUF repos
/// mostly hold (Qwen2.5 7B needs 7 KiB per billion, Llama 3 8B 16 KiB), and
/// it is replaced by the header's exact figure once downloaded.
pub fn estimated_kv_bytes(file: &HubFile, params: Option<u64>) -> u64 {
    let params = params.unwrap_or_else(|| {
        let bits = file.quant.as_deref().map(bits_of).unwrap_or(4.8);
        (file.size() as f64 * 8.0 / bits) as u64
    });
    let billions = params as f64 / 1e9;
    ((billions * 16.0 * 1024.0) as u64).max(32 * 1024)
}

/// The context a Hub model runs with: its trained length, capped at the
/// default PhazeAI uses for catalog models.
pub fn default_ctx(context_length: Option<u64>) -> u32 {
    context_length
        .map(|c| c.min(DEFAULT_CTX_SIZE as u64) as u32)
        .filter(|c| *c >= 2048)
        .unwrap_or(DEFAULT_CTX_SIZE)
}

/// Whether a quantization of `listing` fits this machine, at `ctx`.
pub fn fit_file(listing: &HubListing, file: &HubFile, hw: &HardwareInfo, ctx: u32) -> FitReport {
    let need =
        file.size() + estimated_kv_bytes(file, listing.params) * ctx as u64 + COMPUTE_OVERHEAD;
    fit_need(file.size(), need, need + OS_IDE_RESERVE, hw)
}

/// How good a quantization is, for picking: Q4_K_M and up are the usual
/// sweet spot; below 4 bits quality drops quickly.
fn quality(quant: Option<&str>) -> u8 {
    let Some(q) = quant else { return 3 };
    let q = q.to_ascii_uppercase();
    match q.as_str() {
        "Q4_K_M" => 9,
        "Q5_K_M" => 10,
        "Q6_K" | "Q6_K_L" | "Q6_K_XL" => 11,
        "Q8_0" | "Q8_K_XL" => 12,
        "F16" | "BF16" => 13,
        "F32" => 1,
        _ if q.starts_with("Q5") => 9,
        _ if q.starts_with("Q4") || q.starts_with("IQ4") || q == "MXFP4" => 8,
        _ if q.starts_with("Q3") || q.starts_with("IQ3") => 5,
        _ if q.starts_with("Q2") || q.starts_with("IQ2") => 3,
        _ => 2,
    }
}

/// The quantization to download on this machine: the best quality that fits
/// comfortably, but no bigger than Q8_0 unless nothing else is offered (F16
/// runs at half the speed for almost no gain). Else the best that runs at
/// all. `None` when every file is too big.
pub fn choose<'a>(listing: &'a HubListing, hw: &HardwareInfo) -> Option<&'a HubFile> {
    use super::catalog::Fit;
    let ctx = default_ctx(listing.context_length);
    let rank = |f: &&HubFile| {
        let q = quality(f.quant.as_deref());
        (if q >= 13 { 0 } else { q }, std::cmp::Reverse(f.size()))
    };
    let with_fit: Vec<(&HubFile, Fit)> = listing
        .files
        .iter()
        .map(|f| (f, fit_file(listing, f, hw, ctx).fit))
        .collect();
    let best = |want: Fit| {
        with_fit
            .iter()
            .filter(|(_, fit)| *fit == want)
            .map(|(f, _)| *f)
            .max_by_key(rank)
    };
    best(Fit::Fits).or_else(|| best(Fit::Tight))
}

/// A downloaded Hub model, as recorded in its manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledModel {
    /// `hf:<repo>:<file>`.
    pub id: String,
    pub repo: String,
    pub revision: String,
    /// The first shard's path in the repository.
    pub file: String,
    pub quant: Option<String>,
    pub shards: Vec<HubShard>,
    /// The name from the GGUF header, else the file name.
    pub display_name: String,
    /// From the GGUF header; an estimate if the header lacked it.
    pub kv_bytes_per_token: u64,
    /// The context PhazeAI runs it with unless `[local_ai] ctx_size` says.
    pub ctx_size: u32,
    pub license: Option<String>,
}

impl InstalledModel {
    pub fn size(&self) -> u64 {
        self.shards.iter().map(|s| s.size).sum()
    }

    /// The folder its files are in.
    pub fn dir(&self, models_dir: &Path) -> PathBuf {
        repo_dir(models_dir, &self.repo)
    }

    /// The file to pass to llama-server (the first shard; it finds the rest).
    pub fn path(&self, models_dir: &Path) -> PathBuf {
        let dir = self.dir(models_dir);
        match rel_path(&self.file) {
            Some(rel) => dir.join(rel),
            None => dir.join(base_name(&self.file)),
        }
    }

    /// Whether every shard is on disk at its full size.
    pub fn is_complete(&self, models_dir: &Path) -> bool {
        let dir = self.dir(models_dir);
        self.shards.iter().all(|s| {
            rel_path(&s.path)
                .is_some_and(|r| std::fs::metadata(dir.join(r)).is_ok_and(|m| m.len() == s.size))
        })
    }

    /// Estimated resident memory while serving with `ctx` tokens of context.
    pub fn runtime_bytes(&self, ctx: u32) -> u64 {
        self.size() + self.kv_bytes_per_token * ctx as u64 + COMPUTE_OVERHEAD
    }

    pub fn fit(&self, hw: &HardwareInfo, ctx: u32) -> FitReport {
        let need = self.runtime_bytes(ctx);
        fit_need(self.size(), need, need + OS_IDE_RESERVE, hw)
    }
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A repository path as a relative path on disk, when every part of it is a
/// plain name (no `..`, no drive letters, no backslashes). Files keep the
/// repository's folders, so two quantizations can't overwrite each other.
fn rel_path(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.contains(['\\', ':', '\0'])
            || part.chars().any(char::is_control)
        {
            return None;
        }
        out.push(part);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// `<base>/<repo>/resolve/<revision>/<path>`, each part escaped.
fn resolve_url(
    base: &str,
    repo: &str,
    revision: &str,
    path: &str,
) -> Result<reqwest::Url, LocalAiError> {
    let bad = || LocalAiError::Other(format!("can't build a download address from {base}"));
    let mut url = reqwest::Url::parse(base).map_err(|_| bad())?;
    {
        let mut seg = url.path_segments_mut().map_err(|_| bad())?;
        seg.pop_if_empty();
        seg.extend(repo.split('/'));
        seg.push("resolve");
        seg.push(revision);
        seg.extend(path.split('/'));
    }
    Ok(url)
}

/// `<models>/hub/<owner>__<name>`.
pub fn repo_dir(models_dir: &Path, repo: &str) -> PathBuf {
    models_dir.join("hub").join(repo.replace('/', "__"))
}

/// `<file>.phaze.json`, next to the model's first shard.
fn manifest_path(models_dir: &Path, repo: &str, file: &str) -> Option<PathBuf> {
    let mut p = repo_dir(models_dir, repo)
        .join(rel_path(file)?)
        .into_os_string();
    p.push(".phaze.json");
    Some(PathBuf::from(p))
}

/// The id of `file` in `repo`.
pub fn model_id(repo: &str, file: &str) -> String {
    format!("{ID_PREFIX}{repo}:{file}")
}

/// (`repo`, `file`) from an id, when it is a Hub id.
pub fn parse_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix(ID_PREFIX)?;
    let (repo, file) = rest.split_once(':')?;
    (valid_repo(repo) && file.ends_with(".gguf") && rel_path(file).is_some())
        .then_some((repo, file))
}

/// Every Hub model downloaded into `models_dir`, complete ones only,
/// smallest first.
pub fn installed(models_dir: &Path) -> Vec<InstalledModel> {
    let mut out = Vec::new();
    let Ok(repos) = std::fs::read_dir(models_dir.join("hub")) else {
        return out;
    };
    for repo in repos.flatten() {
        let mut manifests = Vec::new();
        collect_manifests(&repo.path(), 0, &mut manifests);
        for path in manifests {
            let Some(m) = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<InstalledModel>(&b).ok())
            else {
                continue;
            };
            // A manifest only counts where it belongs, when it names itself
            // and every shard has a safe path and is all there.
            if !valid_repo(&m.repo)
                || repo_dir(models_dir, &m.repo) != repo.path()
                || manifest_path(models_dir, &m.repo, &m.file).as_deref() != Some(path.as_path())
                || m.id != model_id(&m.repo, &m.file)
                || m.shards.first().is_none_or(|s| s.path != m.file)
                || !m.is_complete(models_dir)
            {
                continue;
            }
            out.push(m);
        }
    }
    out.sort_by_key(|m| m.size());
    out
}

fn collect_manifests(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(kind) = e.file_type() else { continue };
        let path = e.path();
        if kind.is_dir() && depth < 8 {
            collect_manifests(&path, depth + 1, out);
        } else if kind.is_file() && path.to_string_lossy().ends_with(".phaze.json") {
            out.push(path);
        }
    }
}

/// The installed Hub model with this id.
pub fn find_installed(models_dir: &Path, id: &str) -> Option<InstalledModel> {
    installed(models_dir).into_iter().find(|m| m.id == id)
}

/// Download `file` of `listing` into `models_dir`, every shard verified by
/// size and SHA-256, then record it. Partial shards resume on the next pull;
/// `cancel` stops it. Progress counts bytes across all shards.
pub async fn pull(
    hub: &Hub,
    listing: &HubListing,
    file: &HubFile,
    models_dir: &Path,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<InstalledModel, LocalAiError> {
    if !valid_repo(&listing.repo) {
        return Err(LocalAiError::Other(
            "not a Hugging Face repository name".into(),
        ));
    }
    let dir = repo_dir(models_dir, &listing.repo);
    let mut dests = Vec::with_capacity(file.shards.len());
    for shard in &file.shards {
        let rel = rel_path(&shard.path)
            .ok_or_else(|| LocalAiError::Other(format!("unsafe file name {:?}", shard.path)))?;
        dests.push(dir.join(rel));
    }
    let manifest = manifest_path(models_dir, &listing.repo, &file.path)
        .ok_or_else(|| LocalAiError::Other(format!("unsafe file name {:?}", file.path)))?;
    std::fs::create_dir_all(&dir)?;
    let total = file.size();
    let free = super::hardware::free_disk_space(&dir);
    let have: u64 = dests
        .iter()
        .map(|d| {
            let done = std::fs::metadata(d).map(|m| m.len()).unwrap_or(0);
            let part = std::fs::metadata(super::download::part_path(d))
                .map(|m| m.len())
                .unwrap_or(0);
            done.max(part)
        })
        .sum();
    super::download::check_disk(total.saturating_sub(have), total, free)?;
    let mut done_before = 0u64;
    for (shard, dest) in file.shards.iter().zip(&dests) {
        let url = resolve_url(&hub.base, &listing.repo, &listing.revision, &shard.path)?;
        let offset = done_before;
        download_verified(
            &hub.client,
            url.as_str(),
            dest,
            shard.size,
            &shard.sha256,
            cancel.clone(),
            |p| {
                on_progress(DownloadProgress {
                    downloaded: offset + p.downloaded,
                    total,
                    ..p
                })
            },
        )
        .await?;
        done_before += shard.size;
    }
    let meta = super::gguf::read_meta(&dests[0]).ok();
    let kv = meta
        .as_ref()
        .and_then(|m| m.kv_bytes_per_token())
        .unwrap_or_else(|| estimated_kv_bytes(file, listing.params));
    let context = meta
        .as_ref()
        .and_then(|m| m.context_length)
        .or(listing.context_length);
    let model = InstalledModel {
        id: model_id(&listing.repo, &file.path),
        repo: listing.repo.clone(),
        revision: listing.revision.clone(),
        file: file.path.clone(),
        quant: file.quant.clone(),
        shards: file.shards.clone(),
        display_name: meta
            .and_then(|m| m.name)
            .filter(|n| !n.trim().is_empty())
            .map(|n| match &file.quant {
                Some(q) => format!("{} ({q})", n.trim()),
                None => n.trim().to_string(),
            })
            .unwrap_or_else(|| base_name(&file.path).to_string()),
        kv_bytes_per_token: kv,
        ctx_size: default_ctx(context),
        license: listing.license.clone(),
    };
    let tmp = manifest.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&model).map_err(|e| LocalAiError::Other(e.to_string()))?,
    )?;
    std::fs::rename(&tmp, &manifest)?;
    Ok(model)
}

/// Delete an installed Hub model's files and manifest, and any folders that
/// leaves empty.
pub fn remove(models_dir: &Path, model: &InstalledModel) -> std::io::Result<()> {
    let dir = model.dir(models_dir);
    let mut folders = Vec::new();
    for s in &model.shards {
        let Some(rel) = rel_path(&s.path) else {
            continue;
        };
        let p = dir.join(rel);
        for f in [p.clone(), super::download::part_path(&p)] {
            if f.exists() {
                std::fs::remove_file(&f)?;
            }
        }
        folders.extend(p.parent().map(Path::to_path_buf));
    }
    if let Some(manifest) = manifest_path(models_dir, &model.repo, &model.file) {
        if manifest.exists() {
            std::fs::remove_file(manifest)?;
        }
    }
    // Deepest first; remove_dir only removes empty folders.
    folders.sort_by_key(|f| std::cmp::Reverse(f.components().count()));
    for mut f in folders {
        while f.starts_with(&dir) && std::fs::remove_dir(&f).is_ok() {
            if !f.pop() {
                break;
            }
        }
    }
    let _ = std::fs::remove_dir(&dir);
    Ok(())
}

/// A file size as RAM is shown (GiB), for lists.
pub fn gib(n: u64) -> String {
    format!("{:.1} GiB", n as f64 / GIB as f64)
}

#[cfg(test)]
#[path = "hub_tests.rs"]
mod tests;
