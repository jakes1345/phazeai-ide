//! [`CodeSearch`]: builds, incremental updates and queries.
//!
//! Concurrency model
//! -----------------
//! * Files are found, read, tokenised and counted on worker threads with **no
//!   lock held**. The index's write lock is taken only to apply a batch of
//!   already-parsed files (milliseconds), so a search that arrives mid-build
//!   ranks against the last committed batch and is never held up for the
//!   length of the build.
//! * Builds are serialised by a gate. A second caller waits for the running
//!   build, then does its own sync, which is cheap because everything is
//!   unchanged by then; its stats describe the state when *it* finished.
//! * Single-file updates ([`CodeSearch::refresh_file`], [`CodeSearch::remove_file`])
//!   are serialised by a second gate, from reading the file to committing it:
//!   two of them racing (an editor save and a search repairing a stale hit)
//!   cannot leave the older read in the index. They do not wait for builds,
//!   and a build never commits over a file a single-file update wrote after
//!   the build looked at the index.
//! * [`CodeSearch::status`] reads atomics only.
//! * Locks are never nested: a gate is taken before the state lock, never
//!   the other way round, the two gates are never held together, and no
//!   thread holds the state lock while waiting for it again.

use super::index::{Applied, Candidate, Index, Parsed};
use super::paths::{absolutize, path_key, path_starts_with};
use super::scan::{self, Planned, COMMIT_GROUP};
use super::snippet;
use super::tokenize;
use super::types::{BuildStats, IndexStatus, SearchError, SearchHit, SearchLimits, SearchResults};
use crate::privacy;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

/// Hardest cap on `top_k`, whatever the caller asks for.
const MAX_TOP_K: usize = 50;
/// Longer queries are refused rather than silently truncated.
const MAX_QUERY_BYTES: usize = 16 * 1024;
/// How many times one search re-reads files it found out of date and re-ranks.
const MAX_FIXUP_ROUNDS: usize = 2;

/// An in-process keyword search index for one workspace.
///
/// Cheap to clone (all clones share one index). See the [module docs](super).
#[derive(Clone)]
pub struct CodeSearch {
    inner: Arc<Inner>,
}

struct Inner {
    limits: SearchLimits,
    state: RwLock<State>,
    /// Held for the whole of a build.
    gate: Mutex<()>,
    /// Held by `refresh_file` and `remove_file` from reading a file to
    /// committing the result, so a slow refresh (a search fixing a stale hit)
    /// can never land after, and so overwrite, a newer one (an editor save).
    edits: Mutex<()>,
    built: AtomicBool,
    building: AtomicBool,
    truncated: AtomicBool,
    files: AtomicUsize,
    chunks: AtomicUsize,
    /// Test seam: runs after each committed batch with no lock held.
    #[cfg(test)]
    after_commit: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Test seam: runs after `refresh_file` read a file, before it commits.
    #[cfg(test)]
    after_parse: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

struct State {
    root: PathBuf,
    /// Bumped by `reset_root`; work started for an older root is abandoned.
    generation: u64,
    index: Index,
}

/// Clears `building` however a build ends.
struct BuildingFlag<'a>(&'a AtomicBool);

impl Drop for BuildingFlag<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Whether one more file of `size` bytes fits beside the files already kept.
fn fits(limits: &SearchLimits, kept_files: usize, kept_bytes: u64, size: u64) -> bool {
    kept_files < limits.max_files && kept_bytes.saturating_add(size) <= limits.max_total_bytes
}

impl std::fmt::Debug for CodeSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeSearch")
            .field("root", &self.root())
            .field("status", &self.status())
            .finish()
    }
}

impl CodeSearch {
    /// A search over `root` with the default [`SearchLimits`]. Nothing is read
    /// until the first build or search.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_limits(root, SearchLimits::default())
    }

    pub fn with_limits(root: impl Into<PathBuf>, limits: SearchLimits) -> Self {
        Self {
            inner: Arc::new(Inner {
                limits,
                state: RwLock::new(State {
                    root: absolutize(&root.into()),
                    generation: 0,
                    index: Index::new(),
                }),
                gate: Mutex::new(()),
                edits: Mutex::new(()),
                built: AtomicBool::new(false),
                building: AtomicBool::new(false),
                truncated: AtomicBool::new(false),
                files: AtomicUsize::new(0),
                chunks: AtomicUsize::new(0),
                #[cfg(test)]
                after_commit: Mutex::new(None),
                #[cfg(test)]
                after_parse: Mutex::new(None),
            }),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.read().root.clone()
    }

    pub fn limits(&self) -> SearchLimits {
        self.inner.limits
    }

    /// The workspace changed: forget the index and the "built" mark. A build
    /// still running for the old root stops with [`SearchError::RootChanged`].
    pub fn reset_root(&self, root: impl Into<PathBuf>) {
        let root = absolutize(&root.into());
        let old = {
            let mut st = self.write();
            st.root = root;
            st.generation += 1;
            self.inner.built.store(false, Ordering::Release);
            self.inner.truncated.store(false, Ordering::Release);
            self.inner.files.store(0, Ordering::Release);
            self.inner.chunks.store(0, Ordering::Release);
            std::mem::replace(&mut st.index, Index::new())
        };
        // Freeing a big index takes a while; do it outside the lock.
        drop(old);
    }

    /// Bring the index in line with the disk for `paths` (files or directories,
    /// relative paths are relative to the root; empty means the whole root):
    /// new files are added, files whose (mtime, size) changed are re-read, files
    /// that vanished or are no longer eligible are dropped, everything else is
    /// left alone. Callable from any non-async thread.
    pub fn build_blocking(&self, paths: &[PathBuf]) -> Result<BuildStats, SearchError> {
        let _gate = self.gate();
        self.sync_locked(paths)
    }

    /// [`build_blocking`](Self::build_blocking) on the blocking thread pool.
    pub async fn build(&self, paths: &[PathBuf]) -> Result<BuildStats, SearchError> {
        let this = self.clone();
        let paths = paths.to_vec();
        tokio::task::spawn_blocking(move || this.build_blocking(&paths))
            .await
            .map_err(|e| SearchError::Worker(e.to_string()))?
    }

    /// Rank the indexed files for `query` and return the best `top_k` (at most
    /// 50), one hit per file. The first search builds the whole root (or waits
    /// for the build that is already doing so: results from half an index
    /// would silently miss files). Once an index has been built, searches never
    /// wait for later builds; they rank against what has been committed.
    ///
    /// Files the privacy policy denies *now* are dropped even if they were
    /// indexed earlier, files that changed on disk since indexing are re-read
    /// first, and surviving files are noted in the privacy audit scope of the
    /// calling task (see `privacy::audit::note_file`).
    pub fn search_blocking(&self, query: &str, top_k: usize) -> Result<SearchResults, SearchError> {
        let results = self.search_inner(query, top_k)?;
        note_files(&results);
        Ok(results)
    }

    /// [`search_blocking`](Self::search_blocking) for async callers; the audit
    /// notes are recorded on the awaiting task.
    pub async fn search(&self, query: &str, top_k: usize) -> Result<SearchResults, SearchError> {
        let this = self.clone();
        let query = query.to_owned();
        let results = tokio::task::spawn_blocking(move || this.search_inner(&query, top_k))
            .await
            .map_err(|e| SearchError::Worker(e.to_string()))??;
        note_files(&results);
        Ok(results)
    }

    /// Re-read one file after it was saved. Does nothing for a file that is
    /// outside the root or not indexable; a file that became ineligible, was
    /// deleted or replaced by something else leaves the index.
    pub fn refresh_file(&self, path: &Path) {
        let _edit = self.edits();
        if let Err(e) = self.refresh_inner(path) {
            tracing::debug!("code search refresh of {} failed: {e}", path.display());
        }
    }

    /// Drop a file (or, for a directory, every file below it) from the index.
    pub fn remove_file(&self, path: &Path) {
        let key = path_key(&absolutize(path));
        let _edit = self.edits();
        let mut st = self.write();
        if st.index.remove_under(&key) > 0 {
            self.publish(&st.index);
        }
    }

    /// Never waits for a build.
    pub fn status(&self) -> IndexStatus {
        IndexStatus {
            built: self.inner.built.load(Ordering::Acquire),
            building: self.inner.building.load(Ordering::Acquire),
            files: self.inner.files.load(Ordering::Acquire),
            chunks: self.inner.chunks.load(Ordering::Acquire),
            truncated: self.inner.truncated.load(Ordering::Acquire),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, State> {
        self.inner.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, State> {
        self.inner.state.write().unwrap_or_else(|e| e.into_inner())
    }

    fn gate(&self) -> MutexGuard<'_, ()> {
        self.inner.gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn edits(&self) -> MutexGuard<'_, ()> {
        self.inner.edits.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn publish(&self, index: &Index) {
        self.inner
            .files
            .store(index.file_count(), Ordering::Release);
        self.inner
            .chunks
            .store(index.chunk_count(), Ordering::Release);
    }

    #[cfg(test)]
    fn after_commit_hook(&self) {
        let hook = self
            .inner
            .after_commit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(not(test))]
    fn after_commit_hook(&self) {}

    #[cfg(test)]
    fn after_parse_hook(&self) {
        let hook = self
            .inner
            .after_parse
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(not(test))]
    fn after_parse_hook(&self) {}

    /// Apply one batch under the write lock.
    fn commit(
        &self,
        generation: u64,
        plan_seq: u64,
        removals: &[PathBuf],
        items: Vec<Parsed>,
    ) -> Result<Applied, SearchError> {
        let mut st = self.write();
        if st.generation != generation {
            return Err(SearchError::RootChanged);
        }
        let applied = st.index.apply(plan_seq, removals, items);
        self.publish(&st.index);
        Ok(applied)
    }

    /// One sync pass; the caller holds the gate.
    fn sync_locked(&self, paths: &[PathBuf]) -> Result<BuildStats, SearchError> {
        let inner = &*self.inner;
        inner.building.store(true, Ordering::Release);
        let _building = BuildingFlag(&inner.building);
        let started = Instant::now();
        let (root, generation) = {
            let st = self.read();
            (st.root.clone(), st.generation)
        };
        std::fs::metadata(&root)?;
        let policy = privacy::active();
        let mut stats = BuildStats::default();

        // Which parts of the tree this call is responsible for.
        let mut scopes: Vec<PathBuf> = Vec::new();
        if paths.is_empty() {
            scopes.push(root.clone());
        }
        for p in paths {
            let abs = absolutize(&root.join(p));
            if path_starts_with(&abs, &root) {
                scopes.push(abs);
            } else {
                stats.errors += 1;
                stats.warnings.push(format!(
                    "{} is outside the workspace {}",
                    p.display(),
                    root.display()
                ));
            }
        }
        scopes.sort_by_cached_key(|s| path_key(s));
        let mut kept: Vec<PathBuf> = Vec::new();
        for s in scopes {
            if !kept.iter().any(|k| path_starts_with(&s, k)) {
                kept.push(s);
            }
        }
        let missing: Vec<PathBuf> = kept
            .iter()
            .filter(|s| std::fs::symlink_metadata(s).is_err())
            .cloned()
            .collect();

        // Everything that is eligible right now.
        let mut found: Vec<PathBuf> = Vec::new();
        for scope in &kept {
            let d = scan::discover(&root, scope, &policy);
            stats.privacy_skipped += d.denied_dirs;
            stats.errors += d.errors;
            found.extend(d.files);
        }
        found.sort();
        found.dedup();
        let plan = scan::plan(&found, &policy, &inner.limits);
        stats.privacy_skipped += plan.privacy_skipped;
        stats.skipped += plan.skipped;
        stats.errors += plan.errors;

        // Compare with the index.
        let planned_keys: HashSet<PathBuf> = plan.files.iter().map(|p| path_key(&p.path)).collect();
        let scope_keys: Vec<PathBuf> = kept.iter().map(|s| path_key(s)).collect();
        let (todo, removals, plan_seq) = {
            let st = self.read();
            if st.generation != generation {
                return Err(SearchError::RootChanged);
            }
            let index = &st.index;
            let mut kept_files = index.file_count();
            let mut kept_bytes = index.total_bytes();
            let mut removals: Vec<PathBuf> = Vec::new();
            for (key, meta) in index.entries() {
                if scope_keys.iter().any(|s| key.starts_with(s)) && !planned_keys.contains(key) {
                    kept_files -= 1;
                    kept_bytes -= meta.stamp.size;
                    removals.push(key.to_path_buf());
                }
            }
            let mut changed: Vec<(Planned, Option<PathBuf>)> = Vec::new();
            for p in plan.files {
                let key = path_key(&p.path);
                match index.get(&key) {
                    Some(meta) if meta.stamp == p.stamp => stats.unchanged += 1,
                    Some(meta) => {
                        kept_files -= 1;
                        kept_bytes -= meta.stamp.size;
                        changed.push((p, Some(key)));
                    }
                    None => changed.push((p, None)),
                }
            }
            // Past a limit, files are admitted in path order and the rest are
            // reported, not silently dropped. A changed file that no longer
            // fits must leave the index: its old content is stale.
            changed.sort_by(|a, b| a.0.path.cmp(&b.0.path));
            let mut todo: Vec<Planned> = Vec::new();
            for (p, existing) in changed {
                if fits(&inner.limits, kept_files, kept_bytes, p.stamp.size) {
                    kept_files += 1;
                    kept_bytes += p.stamp.size;
                    todo.push(p);
                } else {
                    stats.truncated = true;
                    removals.extend(existing);
                }
            }
            (todo, removals, index.seq())
        };
        for m in &missing {
            let key = path_key(m);
            if !removals.iter().any(|k| k.starts_with(&key)) {
                stats.errors += 1;
                stats
                    .warnings
                    .push(format!("path does not exist: {}", m.display()));
            }
        }

        let mut applied = self.commit(generation, plan_seq, &removals, Vec::new())?;
        self.read_and_commit(todo, &root, generation, plan_seq, &mut applied)?;

        stats.indexed = applied.indexed;
        stats.updated = applied.updated;
        stats.removed = applied.removed;
        stats.skipped += applied.skipped;
        stats.errors += applied.errors;
        {
            let st = self.read();
            if st.generation != generation {
                return Err(SearchError::RootChanged);
            }
            stats.total_files = st.index.file_count();
            stats.chunks = st.index.chunk_count();
            inner.built.store(true, Ordering::Release);
            // A partial sync cannot prove that an earlier truncation is gone.
            if paths.is_empty() || stats.truncated {
                inner.truncated.store(stats.truncated, Ordering::Release);
            }
        }
        stats.elapsed_ms = started.elapsed().as_millis() as u64;
        let changed = stats.indexed + stats.updated + stats.removed;
        if changed > 0 || stats.truncated {
            tracing::info!(
                indexed = stats.indexed,
                updated = stats.updated,
                unchanged = stats.unchanged,
                removed = stats.removed,
                skipped = stats.skipped,
                errors = stats.errors,
                privacy_skipped = stats.privacy_skipped,
                files = stats.total_files,
                chunks = stats.chunks,
                truncated = stats.truncated,
                elapsed_ms = stats.elapsed_ms,
                "code search index synced"
            );
        } else {
            tracing::debug!(
                files = stats.total_files,
                elapsed_ms = stats.elapsed_ms,
                "code search index already up to date"
            );
        }
        Ok(stats)
    }

    /// Read `todo` on worker threads and commit the results in batches while
    /// the workers carry on with the next files.
    fn read_and_commit(
        &self,
        todo: Vec<Planned>,
        root: &Path,
        generation: u64,
        plan_seq: u64,
        applied: &mut Applied,
    ) -> Result<(), SearchError> {
        if todo.is_empty() {
            return Ok(());
        }
        let workers = scan::worker_count().min(todo.len());
        let limits = self.inner.limits;
        let next = AtomicUsize::new(0);
        let cancel = AtomicBool::new(false);
        let mut outcome = Ok(());
        std::thread::scope(|s| {
            let (tx, rx) = sync_channel::<Parsed>(workers * 4);
            for _ in 0..workers {
                let tx = tx.clone();
                let (todo, next, cancel) = (&todo, &next, &cancel);
                s.spawn(move || {
                    while !cancel.load(Ordering::Relaxed) {
                        let Some(item) = todo.get(next.fetch_add(1, Ordering::Relaxed)) else {
                            break;
                        };
                        if tx
                            .send(scan::parse_file(&item.path, root, &limits))
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }
            drop(tx);
            let mut group: Vec<Parsed> = Vec::with_capacity(COMMIT_GROUP);
            while let Ok(first) = rx.recv() {
                group.push(first);
                while group.len() < COMMIT_GROUP {
                    match rx.try_recv() {
                        Ok(more) => group.push(more),
                        Err(_) => break,
                    }
                }
                match self.commit(generation, plan_seq, &[], std::mem::take(&mut group)) {
                    Ok(a) => applied.merge(a),
                    Err(e) => {
                        cancel.store(true, Ordering::Relaxed);
                        outcome = Err(e);
                        break;
                    }
                }
                self.after_commit_hook();
            }
            // `rx` drops here, which releases workers blocked in `send`.
        });
        outcome
    }

    /// Build the root if no build has completed yet.
    fn ensure_built(&self) -> Result<(), SearchError> {
        for _ in 0..3 {
            if self.inner.built.load(Ordering::Acquire) {
                return Ok(());
            }
            let _gate = self.gate();
            // Another thread may have finished the build while we waited.
            if self.inner.built.load(Ordering::Acquire) {
                return Ok(());
            }
            match self.sync_locked(&[]) {
                Ok(_) => return Ok(()),
                Err(SearchError::RootChanged) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SearchError::RootChanged)
    }

    fn refresh_inner(&self, path: &Path) -> Result<(), SearchError> {
        let abs = absolutize(path);
        let (root, generation) = {
            let st = self.read();
            (st.root.clone(), st.generation)
        };
        if abs == root || !path_starts_with(&abs, &root) {
            return Ok(());
        }
        let key = path_key(&abs);
        let policy = privacy::active();
        let file = if policy.is_denied(&abs) {
            None
        } else {
            scan::discover(&root, &abs, &policy)
                .files
                .into_iter()
                .find(|f| path_key(f) == key)
        };
        let Some(file) = file else {
            self.commit(generation, u64::MAX, &[key], Vec::new())?;
            return Ok(());
        };
        let size = std::fs::metadata(&file).map_or(0, |m| m.len());
        if size <= self.inner.limits.max_file_bytes {
            let room = {
                let st = self.read();
                let old = st.index.get(&key).map(|m| m.stamp.size);
                let kept_files = st.index.file_count() - usize::from(old.is_some());
                let kept_bytes = st.index.total_bytes() - old.unwrap_or(0);
                fits(&self.inner.limits, kept_files, kept_bytes, size)
            };
            if !room {
                self.inner.truncated.store(true, Ordering::Release);
                self.commit(generation, u64::MAX, &[key], Vec::new())?;
                return Ok(());
            }
        }
        let parsed = scan::parse_file(&file, &root, &self.inner.limits);
        self.after_parse_hook();
        self.commit(generation, u64::MAX, &[], vec![parsed])?;
        Ok(())
    }

    fn search_inner(&self, query: &str, top_k: usize) -> Result<SearchResults, SearchError> {
        if query.len() > MAX_QUERY_BYTES {
            return Err(SearchError::InvalidQuery(format!(
                "the query is {} bytes long; the limit is {MAX_QUERY_BYTES}",
                query.len()
            )));
        }
        let terms = tokenize::query_terms(query);
        let k = top_k.min(MAX_TOP_K);
        if terms.is_empty() || k == 0 {
            return Ok(SearchResults::default());
        }
        self.ensure_built()?;
        let policy = privacy::active();
        let lookup: HashMap<&str, usize> = terms
            .iter()
            .enumerate()
            .map(|(i, t)| (t.as_str(), i))
            .collect();
        // Ask for more candidates than needed: some may be denied or stale.
        let mut want = k * 3 + 8;
        let mut fixups = 0;
        loop {
            let ranked = self.read().index.rank(&terms, want);
            let mut hits: Vec<SearchHit> = Vec::new();
            let mut denied = 0;
            let mut stale: Vec<Arc<Path>> = Vec::new();
            for cand in &ranked.candidates {
                if hits.len() == k {
                    break;
                }
                // The policy may have changed since the file was indexed.
                if policy.is_denied(&cand.path) {
                    denied += 1;
                    continue;
                }
                match self.load_hit(cand, &lookup) {
                    Some(hit) => hits.push(hit),
                    None => stale.push(Arc::clone(&cand.path)),
                }
            }
            if !stale.is_empty() && fixups < MAX_FIXUP_ROUNDS {
                fixups += 1;
                for p in &stale {
                    self.refresh_file(p);
                }
                continue;
            }
            if hits.len() < k && ranked.matched > ranked.candidates.len() {
                want = want.saturating_mul(4);
                continue;
            }
            return Ok(SearchResults {
                matches: hits,
                privacy_skipped: denied,
                index_truncated: self.inner.truncated.load(Ordering::Acquire),
            });
        }
    }

    /// Turn a candidate into a hit by reading its lines from disk. `None` when
    /// the file no longer matches what was indexed (changed, vanished, ...).
    fn load_hit(&self, cand: &Candidate, lookup: &HashMap<&str, usize>) -> Option<SearchHit> {
        let text = scan::read_checked(&cand.path, cand.stamp, self.inner.limits.max_file_bytes)?;
        let lines: Vec<&str> = text.lines().collect();
        let (start, end) = (cand.start as usize, cand.end as usize);
        if start == 0 || start > end || end > lines.len() {
            return None;
        }
        let snip = snippet::build(&lines[start - 1..end], start, lookup);
        Some(SearchHit {
            file: cand.path.to_string_lossy().into_owned(),
            start_line: snip.start,
            end_line: snip.end,
            score: cand.score as f32,
            snippet: snip.text,
        })
    }

    #[cfg(test)]
    pub(super) fn set_after_commit(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self
            .inner
            .after_commit
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(hook));
    }

    #[cfg(test)]
    pub(super) fn set_after_parse(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self
            .inner
            .after_parse
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(hook));
    }

    #[cfg(test)]
    pub(super) fn with_index<R>(&self, f: impl FnOnce(&Index) -> R) -> R {
        f(&self.read().index)
    }

    #[cfg(test)]
    pub(super) fn with_index_mut<R>(&self, f: impl FnOnce(&mut Index) -> R) -> R {
        let mut st = self.write();
        let out = f(&mut st.index);
        self.publish(&st.index);
        out
    }

    #[cfg(test)]
    pub(super) fn stamp_of(&self, path: &Path) -> Option<super::index::Stamp> {
        self.read()
            .index
            .get(&path_key(&absolutize(path)))
            .map(|m| m.stamp)
    }
}

/// Record the files whose text is about to go into a model request.
fn note_files(results: &SearchResults) {
    for hit in &results.matches {
        privacy::audit::note_file(Path::new(&hit.file));
    }
}
