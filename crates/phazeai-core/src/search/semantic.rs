//! Meaning-based matching: a vector for every chunk of the keyword index.
//!
//! When `[search] embedding_model` is set, every chunk that the keyword index
//! holds is also sent to that embedding model, and a search embeds the query
//! and compares it with every chunk (cosine similarity). The two rankings are
//! merged with reciprocal rank fusion, so a query that shares no words with
//! the code ("where do we back off after a failed request") still finds it,
//! and an exact identifier still wins.
//!
//! Vectors follow the keyword index: a sync walks the index's files, re-chunks
//! any file whose (mtime, size) differs from when it was embedded, and only
//! sends chunks whose text it has never embedded before. Vectors are kept on
//! disk under the cache directory, keyed by a hash of the chunk's text (with
//! its path), so reopening a project or switching back to a branch costs no
//! requests. Embedding runs in the background; until it is done, files not
//! embedded yet are found by keywords only.

use super::chunk;
use super::embed::{self, EmbeddingConfig};
use super::index::Stamp;
use super::scan;
use super::CodeSearch;
use crate::privacy;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// A chunk's identity: the first 16 bytes of the SHA-256 of what was embedded.
pub(crate) type ChunkHash = [u8; 16];
/// Vectors by chunk hash, as kept on disk.
pub(crate) type VectorCache = HashMap<ChunkHash, Arc<[f32]>>;
/// A file waiting for some of its chunk vectors: (path, stamp, chunks).
type PendingFile = (Arc<Path>, Stamp, Vec<(u32, u32, ChunkHash)>);

/// Vectors kept on disk beyond the ones the index uses now (old branches,
/// deleted files), so the file stays bounded.
const MAX_SPARE_VECTORS: usize = 50_000;
/// After a failed request, wait this long before a search tries again.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Save the vectors every this many requests during a long first sync.
const SAVE_EVERY_BATCHES: usize = 20;
const CACHE_MAGIC: &[u8; 8] = b"PHZEMB01";

/// What instruction prefixes a model was trained with, as (query, document).
/// Models outside these families take the text as it is.
pub(crate) fn prefixes(model: &str) -> (&'static str, &'static str) {
    let m = model.to_ascii_lowercase();
    if m.contains("nomic-embed") {
        ("search_query: ", "search_document: ")
    } else if m.contains("e5-") || m.starts_with("e5") || m.contains("/e5") {
        ("query: ", "passage: ")
    } else if m.contains("mxbai-embed") || (m.contains("bge-") && m.contains("en")) {
        (
            "Represent this sentence for searching relevant passages: ",
            "",
        )
    } else {
        ("", "")
    }
}

/// The text embedded for a chunk: its path, then its lines.
pub(crate) fn chunk_text(doc_prefix: &str, rel_path: &str, lines: &[&str]) -> String {
    let mut s = String::with_capacity(doc_prefix.len() + rel_path.len() + 64 * lines.len());
    s.push_str(doc_prefix);
    s.push_str(rel_path);
    s.push('\n');
    for line in lines {
        s.push_str(line);
        s.push('\n');
    }
    s
}

pub(crate) fn hash_text(text: &str) -> ChunkHash {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// Scale `v` to length 1, so a dot product is the cosine. `None` for a zero
/// or non-finite vector.
pub(crate) fn normalize(mut v: Vec<f32>) -> Option<Vec<f32>> {
    let norm = v
        .iter()
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    for x in &mut v {
        *x = (f64::from(*x) / norm) as f32;
    }
    Some(v)
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[derive(Clone)]
struct ChunkVec {
    start: u32,
    end: u32,
    vec: Arc<[f32]>,
}

struct FileVecs {
    stamp: Stamp,
    chunks: Vec<ChunkVec>,
}

#[derive(Default)]
struct Store {
    /// The root the vectors below belong to.
    root: Option<PathBuf>,
    files: HashMap<Arc<Path>, FileVecs>,
    /// Every vector known, by chunk hash (normalized).
    cache: VectorCache,
    dim: usize,
    /// Whether `cache` was loaded from disk for `root`.
    loaded: bool,
    /// Chunks of the index the last sync saw.
    total_chunks: usize,
}

/// A file the semantic ranking proposes: its best chunk.
pub(crate) struct SemanticCandidate {
    pub path: Arc<Path>,
    pub stamp: Stamp,
    pub start: u32,
    pub end: u32,
    pub similarity: f32,
}

/// How far meaning-based matching has got.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct EmbeddingStatus {
    pub model: String,
    /// Chunks with a vector.
    pub ready: usize,
    /// Chunks in the keyword index at the last sync.
    pub total: usize,
    /// A sync is running.
    pub syncing: bool,
    /// Why the last request failed, if it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl EmbeddingStatus {
    /// One line for the agent when matching by meaning is not complete:
    /// failing, or still embedding. `None` when every chunk has a vector.
    pub fn note(&self) -> Option<String> {
        if let Some(e) = &self.error {
            let e: String = e.chars().take(160).collect();
            return Some(format!(
                "Matching by meaning is unavailable ({e}); these results match keywords only."
            ));
        }
        if self.total == 0 || self.ready >= self.total {
            return None;
        }
        Some(format!(
            "Matching by meaning covers {}% of the code so far; the rest matched by keywords only.",
            self.ready * 100 / self.total
        ))
    }
}

pub(crate) struct Semantic {
    cfg: EmbeddingConfig,
    store: RwLock<Store>,
    syncing: AtomicBool,
    last_error: Mutex<Option<(String, Instant)>>,
    /// Where vectors are kept between runs; `None` keeps them in memory only.
    cache_dir: Option<PathBuf>,
}

/// Clears `syncing` however a sync ends.
struct SyncFlag<'a>(&'a AtomicBool);

impl Drop for SyncFlag<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn default_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| {
        d.join(crate::constants::paths::CONFIG_DIR)
            .join("embeddings")
    })
}

fn hex16(bytes: &[u8]) -> String {
    bytes.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

impl Semantic {
    pub(crate) fn new(cfg: EmbeddingConfig) -> Self {
        Self::with_cache_dir(cfg, default_cache_dir())
    }

    pub(crate) fn with_cache_dir(cfg: EmbeddingConfig, cache_dir: Option<PathBuf>) -> Self {
        Self {
            cfg,
            store: RwLock::default(),
            syncing: AtomicBool::new(false),
            last_error: Mutex::new(None),
            cache_dir,
        }
    }

    pub(crate) fn config(&self) -> &EmbeddingConfig {
        &self.cfg
    }

    fn cache_file(&self, root: &Path) -> Option<PathBuf> {
        let dir = self.cache_dir.as_ref()?;
        let space = Sha256::digest(self.cfg.space_key().as_bytes());
        let root = Sha256::digest(root.to_string_lossy().as_bytes());
        Some(dir.join(format!("{}-{}.bin", hex16(&root), hex16(&space))))
    }

    pub(crate) fn status(&self) -> EmbeddingStatus {
        let st = self.store.read().unwrap_or_else(|e| e.into_inner());
        let ready = st.files.values().map(|f| f.chunks.len()).sum();
        EmbeddingStatus {
            model: self.cfg.model.clone(),
            ready,
            total: st.total_chunks.max(ready),
            syncing: self.syncing.load(Ordering::Acquire),
            error: self
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(|(e, _)| e.clone()),
        }
    }

    /// Forget the vectors of the current root (the workspace changed). The
    /// on-disk cache is kept.
    pub(crate) fn clear(&self) {
        let mut st = self.store.write().unwrap_or_else(|e| e.into_inner());
        *st = Store::default();
    }

    /// A failed request is recent enough that a search should not try again.
    pub(crate) fn backing_off(&self) -> bool {
        self.last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() < RETRY_AFTER)
    }

    /// Some chunk has a vector.
    pub(crate) fn has_vectors(&self) -> bool {
        let st = self.store.read().unwrap_or_else(|e| e.into_inner());
        st.files.values().any(|f| !f.chunks.is_empty())
    }

    /// A search's query embedding worked: the endpoint is answering.
    pub(crate) fn note_ok(&self) {
        self.set_error(None);
    }

    /// A search's query embedding failed.
    pub(crate) fn note_error(&self, e: String) {
        tracing::debug!("query embedding failed: {e}");
        self.set_error(Some(e));
    }

    fn set_error(&self, e: Option<String>) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = e.map(|e| (e, Instant::now()));
    }

    /// Embed `query` and rank every embedded file by its most similar chunk.
    /// Files the keyword index no longer holds at the same stamp are left out.
    pub(crate) async fn rank(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SemanticCandidate>, String> {
        let (query_prefix, _) = prefixes(&self.cfg.model);
        let text = format!("{query_prefix}{query}");
        let vec = embed::embed(&self.cfg, &[text], &[])
            .await?
            .into_iter()
            .next()
            .and_then(normalize)
            .ok_or_else(|| "the embedding endpoint sent an empty vector".to_string())?;
        Ok(self.rank_vector(&vec, limit))
    }

    pub(crate) fn rank_vector(&self, q: &[f32], limit: usize) -> Vec<SemanticCandidate> {
        let st = self.store.read().unwrap_or_else(|e| e.into_inner());
        if st.dim != q.len() {
            return Vec::new();
        }
        let mut out: Vec<SemanticCandidate> = st
            .files
            .iter()
            .filter_map(|(path, f)| {
                let best = f
                    .chunks
                    .iter()
                    .map(|c| (dot(q, &c.vec), c))
                    .max_by(|a, b| a.0.total_cmp(&b.0))?;
                Some(SemanticCandidate {
                    path: Arc::clone(path),
                    stamp: f.stamp,
                    start: best.1.start,
                    end: best.1.end,
                    similarity: best.0,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.similarity
                .total_cmp(&a.similarity)
                .then_with(|| a.path.cmp(&b.path))
        });
        out.truncate(limit);
        out
    }

    /// Start a sync on a thread of its own unless one is running or a recent
    /// request failed. Returns at once, from any thread: the sync does not
    /// depend on the caller's runtime, so it carries on after an agent run (and
    /// its runtime) ends.
    pub(crate) fn spawn_sync(self: &Arc<Self>, search: &CodeSearch) {
        if self.backing_off() || self.syncing.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = Arc::clone(self);
        let search = search.clone();
        let started = std::thread::Builder::new()
            .name("code-search-embed".into())
            .spawn(move || {
                let _flag = SyncFlag(&this.syncing);
                let result = block_on(this.sync_locked(&search));
                if let Some(Err(e)) = result {
                    tracing::debug!("embedding sync stopped: {e}");
                }
            });
        if started.is_err() {
            self.syncing.store(false, Ordering::Release);
        }
    }

    /// [`rank`](Self::rank) for a thread outside any async runtime, waiting
    /// at most `timeout`.
    pub(crate) fn rank_blocking(
        &self,
        query: &str,
        limit: usize,
        timeout: Duration,
    ) -> Result<Vec<SemanticCandidate>, String> {
        block_on(async { tokio::time::timeout(timeout, self.rank(query, limit)).await })
            .ok_or_else(|| "could not start the embedding request".to_string())?
            .map_err(|_| {
                format!(
                    "the embedding endpoint took over {}s to answer",
                    timeout.as_secs()
                )
            })?
    }

    /// Bring the vectors in line with the keyword index and wait for it.
    pub(crate) async fn sync(&self, search: &CodeSearch) -> Result<(), String> {
        while self.syncing.swap(true, Ordering::AcqRel) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _flag = SyncFlag(&self.syncing);
        self.sync_locked(search).await
    }

    async fn sync_locked(&self, search: &CodeSearch) -> Result<(), String> {
        let (root, generation, files) = search.file_stamps();
        self.prepare(&root, &files).await;

        let (_, doc_prefix) = prefixes(&self.cfg.model);
        let policy = privacy::active();
        let limits = search.limits();
        // Files to (re-)embed, read and chunked off the async threads.
        let stale: Vec<(Arc<Path>, Stamp)> = {
            let st = self.store.read().unwrap_or_else(|e| e.into_inner());
            files
                .iter()
                .filter(|(p, s)| st.files.get(p).is_none_or(|f| f.stamp != *s))
                .cloned()
                .collect()
        };
        let total_chunks = search.status().chunks;
        self.store
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .total_chunks = total_chunks;
        if stale.is_empty() {
            // Nothing was asked of the endpoint, so a failure a search saw
            // stands until a request succeeds.
            return Ok(());
        }
        let root2 = root.clone();
        let doc_prefix = doc_prefix.to_string();
        let parsed = tokio::task::spawn_blocking(move || {
            stale
                .into_iter()
                .filter(|(p, _)| !policy.is_denied(p))
                .filter_map(|(path, stamp)| {
                    let text = scan::read_checked(&path, stamp, limits.max_file_bytes)?;
                    let rel = path
                        .strip_prefix(&root2)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    let lines: Vec<&str> = text.lines().collect();
                    let chunks: Vec<(u32, u32, String)> = chunk::split(&lines)
                        .into_iter()
                        .map(|s| {
                            let t = chunk_text(&doc_prefix, &rel, &lines[s.start - 1..s.end]);
                            (s.start as u32, s.end as u32, t)
                        })
                        .collect();
                    Some((path, stamp, chunks))
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| e.to_string())?;

        // Files whose every chunk is already known are done without a request.
        let mut pending: Vec<PendingFile> = Vec::new();
        let mut wanted: Vec<(ChunkHash, String, String)> = Vec::new();
        let mut asked: HashSet<ChunkHash> = HashSet::new();
        {
            let mut st = self.store.write().unwrap_or_else(|e| e.into_inner());
            for (path, stamp, chunks) in parsed {
                let hashed: Vec<(u32, u32, ChunkHash)> = chunks
                    .iter()
                    .map(|(s, e, t)| (*s, *e, hash_text(t)))
                    .collect();
                for ((_, _, text), (_, _, h)) in chunks.into_iter().zip(&hashed) {
                    if !st.cache.contains_key(h) && asked.insert(*h) {
                        wanted.push((*h, text, path.to_string_lossy().into_owned()));
                    }
                }
                if !install(&mut st, &path, stamp, &hashed) {
                    pending.push((path, stamp, hashed));
                }
            }
        }

        let mut batches = 0usize;
        for batch in wanted.chunks(embed::BATCH) {
            if search.generation() != generation {
                return Err("the workspace changed".into());
            }
            let texts: Vec<String> = batch.iter().map(|(_, t, _)| t.clone()).collect();
            let mut files: Vec<String> = batch.iter().map(|(_, _, f)| f.clone()).collect();
            files.dedup();
            let vectors = match embed::embed(&self.cfg, &texts, &files).await {
                Ok(v) => v,
                Err(e) => {
                    self.set_error(Some(e.clone()));
                    self.save();
                    return Err(e);
                }
            };
            let mut st = self.store.write().unwrap_or_else(|e| e.into_inner());
            for ((h, _, _), v) in batch.iter().zip(vectors) {
                let Some(v) = normalize(v) else { continue };
                if st.dim == 0 {
                    st.dim = v.len();
                }
                if v.len() == st.dim {
                    st.cache.insert(*h, v.into());
                }
            }
            pending.retain(|(path, stamp, hashed)| !install(&mut st, path, *stamp, hashed));
            drop(st);
            batches += 1;
            if batches.is_multiple_of(SAVE_EVERY_BATCHES) {
                self.save();
            }
        }
        self.set_error(None);
        self.save();
        Ok(())
    }

    /// Switch the store to `root` (loading its cached vectors once) and drop
    /// files the index no longer holds.
    async fn prepare(&self, root: &Path, files: &[(Arc<Path>, Stamp)]) {
        let needs_load = {
            let mut st = self.store.write().unwrap_or_else(|e| e.into_inner());
            if st.root.as_deref() != Some(root) {
                *st = Store {
                    root: Some(root.to_path_buf()),
                    ..Store::default()
                };
            }
            !st.loaded
        };
        if needs_load {
            let path = self.cache_file(root);
            let loaded = match path {
                Some(p) => tokio::task::spawn_blocking(move || load_cache(&p))
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let mut st = self.store.write().unwrap_or_else(|e| e.into_inner());
            if st.root.as_deref() == Some(root) && !st.loaded {
                if let Some((dim, cache)) = loaded {
                    if st.dim == 0 || st.dim == dim {
                        st.dim = dim;
                        for (h, v) in cache {
                            st.cache.entry(h).or_insert(v);
                        }
                    }
                }
                st.loaded = true;
            }
        }
        let live: HashSet<&Arc<Path>> = files.iter().map(|(p, _)| p).collect();
        let mut st = self.store.write().unwrap_or_else(|e| e.into_inner());
        st.files.retain(|p, _| live.contains(p));
    }

    /// Write the vectors to disk: every one the index uses, plus spares up to
    /// [`MAX_SPARE_VECTORS`]. Failures are logged and otherwise ignored.
    fn save(&self) {
        let st = self.store.read().unwrap_or_else(|e| e.into_inner());
        let (Some(root), true) = (st.root.as_deref(), st.dim > 0) else {
            return;
        };
        let Some(path) = self.cache_file(root) else {
            return;
        };
        let live: HashSet<*const f32> = st
            .files
            .values()
            .flat_map(|f| f.chunks.iter().map(|c| c.vec.as_ptr()))
            .collect();
        let mut spare = 0usize;
        let rows: Vec<(&ChunkHash, &Arc<[f32]>)> = st
            .cache
            .iter()
            .filter(|(_, v)| {
                if live.contains(&v.as_ptr()) {
                    return true;
                }
                spare += 1;
                spare <= MAX_SPARE_VECTORS
            })
            .collect();
        if let Err(e) = write_cache(&path, st.dim, &rows) {
            tracing::debug!("could not save embeddings to {}: {e}", path.display());
        }
    }
}

/// Run `f` to completion on a runtime of its own. `None` if none could be made.
fn block_on<F: std::future::Future>(f: F) -> Option<F::Output> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    Some(rt.block_on(f))
}

/// Put a file's chunks in the store if every chunk has a vector.
fn install(
    st: &mut Store,
    path: &Arc<Path>,
    stamp: Stamp,
    hashed: &[(u32, u32, ChunkHash)],
) -> bool {
    let chunks: Option<Vec<ChunkVec>> = hashed
        .iter()
        .map(|(s, e, h)| {
            st.cache.get(h).map(|v| ChunkVec {
                start: *s,
                end: *e,
                vec: Arc::clone(v),
            })
        })
        .collect();
    match chunks {
        Some(chunks) => {
            st.files
                .insert(Arc::clone(path), FileVecs { stamp, chunks });
            true
        }
        None => false,
    }
}

fn write_cache(path: &Path, dim: usize, rows: &[(&ChunkHash, &Arc<[f32]>)]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        out.write_all(CACHE_MAGIC)?;
        out.write_all(&(dim as u32).to_le_bytes())?;
        out.write_all(&(rows.len() as u32).to_le_bytes())?;
        for (h, v) in rows {
            out.write_all(&h[..])?;
            for x in v.iter() {
                out.write_all(&x.to_le_bytes())?;
            }
        }
        out.flush()?;
    }
    std::fs::rename(&tmp, path)
}

/// Read a cache file; `None` when it is missing, damaged or of another format.
pub(crate) fn load_cache(path: &Path) -> Option<(usize, VectorCache)> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() < 16 || &bytes[..8] != CACHE_MAGIC {
        return None;
    }
    let dim = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let count = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    let row = 16 + dim * 4;
    if dim == 0 || bytes.len() != 16 + count.checked_mul(row)? {
        return None;
    }
    let mut out = HashMap::with_capacity(count);
    for r in bytes[16..].chunks_exact(row) {
        let mut h = [0u8; 16];
        h.copy_from_slice(&r[..16]);
        let v: Vec<f32> = r[16..]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        if v.iter().all(|x| x.is_finite()) {
            out.insert(h, v.into());
        }
    }
    Some((dim, out))
}

/// Reciprocal rank fusion constant: the usual 60.
pub(crate) const RRF_K: f64 = 60.0;

/// Fused score of a file at 0-based `rank` in one list.
pub(crate) fn rrf(rank: usize) -> f64 {
    1.0 / (RRF_K + rank as f64 + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_families_get_their_prefixes() {
        assert_eq!(prefixes("nomic-embed-text").0, "search_query: ");
        assert_eq!(prefixes("nomic-embed-text:v1.5").1, "search_document: ");
        assert_eq!(prefixes("intfloat/e5-small-v2").1, "passage: ");
        assert!(prefixes("mxbai-embed-large").0.starts_with("Represent"));
        assert_eq!(prefixes("all-minilm"), ("", ""));
    }

    #[test]
    fn normalize_makes_unit_vectors_and_refuses_zero() {
        let v = normalize(vec![3.0, 4.0]).unwrap();
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        assert!(normalize(vec![0.0, 0.0]).is_none());
        assert!(normalize(vec![f32::NAN, 1.0]).is_none());
    }

    #[test]
    fn the_hash_covers_path_and_text() {
        let a = hash_text(&chunk_text("", "src/a.rs", &["fn x() {}"]));
        let b = hash_text(&chunk_text("", "src/b.rs", &["fn x() {}"]));
        let c = hash_text(&chunk_text("", "src/a.rs", &["fn y() {}"]));
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, hash_text(&chunk_text("", "src/a.rs", &["fn x() {}"])));
    }

    #[test]
    fn cache_round_trips_and_refuses_damage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/v.bin");
        let h1 = [1u8; 16];
        let h2 = [2u8; 16];
        let v1: Arc<[f32]> = vec![0.6, 0.8].into();
        let v2: Arc<[f32]> = vec![1.0, 0.0].into();
        write_cache(&path, 2, &[(&h1, &v1), (&h2, &v2)]).unwrap();
        let (dim, back) = load_cache(&path).unwrap();
        assert_eq!(dim, 2);
        assert_eq!(&back[&h1][..], &[0.6, 0.8]);
        assert_eq!(&back[&h2][..], &[1.0, 0.0]);
        // Truncated file: refused, not half-read.
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();
        assert!(load_cache(&path).is_none());
        std::fs::write(&path, b"not a cache").unwrap();
        assert!(load_cache(&path).is_none());
    }

    #[test]
    fn rrf_prefers_higher_ranks() {
        assert!(rrf(0) > rrf(1));
        assert!((rrf(0) - 1.0 / 61.0).abs() < 1e-12);
    }
}
