//! The inverted index: BM25 over chunks with exact incremental updates.
//!
//! Only chunk *metadata* lives here (file, line range, token count) plus the
//! postings `term -> [(chunk, tf)]`; chunk text is read back from disk for the
//! few hits that are returned.
//!
//! Chunk ids only grow, so every posting list stays sorted by chunk and each
//! file owns one contiguous id range. Removing a file physically deletes its
//! postings (by binary-searching that range in the lists of the file's own
//! terms), which keeps document frequencies, the chunk count and the average
//! chunk length exact at all times; only the small chunk table keeps
//! tombstones, and [`Index::compact`] renumbers it once enough have piled up.
//! Scores depend only on the live chunks, so an index that went through any
//! sequence of updates ranks exactly like one built from scratch.

use super::paths::path_key;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const K1: f64 = 1.2;
const B: f64 = 0.75;
const NONE: u32 = u32::MAX;
/// Up to this many files are removed by draining each posting list range by
/// range; more use a single retain pass over the affected lists.
const DRAIN_MAX_FILES: usize = 8;
/// Dead chunk-table slots tolerated before compaction (and at least as many as live chunks / 2).
#[cfg(not(test))]
const COMPACT_MIN_DEAD: usize = 4096;
#[cfg(test)]
const COMPACT_MIN_DEAD: usize = 16;
/// Chunk ids are `u32`; stay far below the limit.
const MAX_CHUNKS: usize = (u32::MAX / 2) as usize;

/// What identifies a file's content cheaply: modification time and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub mtime: SystemTime,
    pub size: u64,
}

impl Stamp {
    pub(crate) fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            mtime: meta.modified().unwrap_or(UNIX_EPOCH),
            size: meta.len(),
        }
    }
}

/// One chunk ready to be indexed.
pub(crate) struct ParsedChunk {
    pub start: u32,
    pub end: u32,
    /// Token count, the BM25 chunk length.
    pub len: u32,
    pub terms: Vec<(String, u32)>,
}

pub(crate) struct ParsedFile {
    pub path: PathBuf,
    pub stamp: Stamp,
    pub chunks: Vec<ParsedChunk>,
}

/// The outcome of reading one file.
pub(crate) enum Parsed {
    File(ParsedFile),
    /// Too large, binary or not UTF-8.
    Skipped(PathBuf),
    /// Could not be read.
    Failed(PathBuf),
    /// Deleted before it could be read.
    Gone(PathBuf),
}

impl Parsed {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Parsed::File(f) => &f.path,
            Parsed::Skipped(p) | Parsed::Failed(p) | Parsed::Gone(p) => p,
        }
    }
}

/// What one [`Index::apply`] changed.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Applied {
    pub indexed: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub errors: usize,
}

impl Applied {
    pub(crate) fn merge(&mut self, o: Applied) {
        self.indexed += o.indexed;
        self.updated += o.updated;
        self.removed += o.removed;
        self.skipped += o.skipped;
        self.errors += o.errors;
    }
}

#[derive(Clone, Copy)]
struct Posting {
    chunk: u32,
    tf: u32,
}

#[derive(Clone, Copy)]
struct ChunkMeta {
    /// Owning file slot, [`NONE`] for a tombstone.
    file: u32,
    start: u32,
    end: u32,
    len: u32,
}

pub(crate) struct FileMeta {
    pub path: Arc<Path>,
    pub stamp: Stamp,
    /// Commit counter value when this entry was written.
    pub seq: u64,
    chunk_first: u32,
    chunk_count: u32,
    /// Every term occurring in the file's chunks, once each.
    terms: Box<[u32]>,
}

/// A ranked file: its best chunk.
pub(crate) struct Candidate {
    pub path: Arc<Path>,
    /// The file's stamp when it was indexed.
    pub stamp: Stamp,
    pub start: u32,
    pub end: u32,
    pub score: f64,
}

pub(crate) struct Ranked {
    /// The best `limit` matching files, best first.
    pub candidates: Vec<Candidate>,
    /// How many files matched at all.
    pub matched: usize,
}

pub(crate) struct Index {
    terms: HashMap<Box<str>, u32>,
    postings: Vec<Vec<Posting>>,
    chunks: Vec<ChunkMeta>,
    files: Vec<Option<FileMeta>>,
    free_files: Vec<u32>,
    by_key: HashMap<PathBuf, u32>,
    live_chunks: usize,
    dead_chunks: usize,
    total_len: u64,
    total_bytes: u64,
    /// Bumped by every change to a file; see [`Index::apply`].
    seq: u64,
}

fn round4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

impl Index {
    pub(crate) fn new() -> Self {
        Self {
            terms: HashMap::new(),
            postings: Vec::new(),
            chunks: Vec::new(),
            files: Vec::new(),
            free_files: Vec::new(),
            by_key: HashMap::new(),
            live_chunks: 0,
            dead_chunks: 0,
            total_len: 0,
            total_bytes: 0,
            seq: 0,
        }
    }

    pub(crate) fn file_count(&self) -> usize {
        self.by_key.len()
    }

    pub(crate) fn chunk_count(&self) -> usize {
        self.live_chunks
    }

    /// Bytes of file content currently indexed.
    pub(crate) fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    pub(crate) fn get(&self, key: &Path) -> Option<&FileMeta> {
        let id = *self.by_key.get(key)?;
        self.files[id as usize].as_ref()
    }

    /// Every indexed file with its map key.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&Path, &FileMeta)> {
        self.by_key
            .iter()
            .filter_map(|(k, &id)| self.files[id as usize].as_ref().map(|f| (k.as_path(), f)))
    }

    /// Apply one batch atomically (the caller holds the write lock).
    ///
    /// * `removals` are map keys to drop.
    /// * `items` are freshly read files; each replaces the entry for its path.
    ///
    /// An entry written after `plan_seq` (by a concurrent `refresh_file`, say)
    /// is newer than anything the batch's planner saw, so the batch leaves it
    /// alone. Pass `u64::MAX` for changes that must win.
    pub(crate) fn apply(
        &mut self,
        plan_seq: u64,
        removals: &[PathBuf],
        items: Vec<Parsed>,
    ) -> Applied {
        let mut applied = Applied::default();
        let mut dead: Vec<u32> = Vec::new();
        for key in removals {
            if let Some(&id) = self.by_key.get(key) {
                if self.slot(id).seq <= plan_seq {
                    dead.push(id);
                    applied.removed += 1;
                }
            }
        }
        let mut adds: Vec<(ParsedFile, bool)> = Vec::new();
        for item in items {
            let existing = self.by_key.get(&path_key(item.path())).copied();
            if existing.is_some_and(|id| self.slot(id).seq > plan_seq) {
                continue;
            }
            // Whatever was read, the old version of the file is out of date.
            if let Some(id) = existing {
                dead.push(id);
            }
            // `removed` counts every file that leaves the index, so
            // `indexed - removed` is always the change in file count.
            let left = usize::from(existing.is_some());
            match item {
                Parsed::File(pf) => adds.push((pf, existing.is_some())),
                Parsed::Skipped(_) => {
                    applied.skipped += 1;
                    applied.removed += left;
                }
                Parsed::Failed(_) => {
                    applied.errors += 1;
                    applied.removed += left;
                }
                Parsed::Gone(_) => applied.removed += left,
            }
        }
        dead.sort_unstable();
        dead.dedup();
        self.remove_ids(&dead);
        for (pf, was_update) in adds {
            if !self.insert(pf) {
                applied.skipped += 1;
                applied.removed += usize::from(was_update);
            } else if was_update {
                applied.updated += 1;
            } else {
                applied.indexed += 1;
            }
        }
        if self.dead_chunks > COMPACT_MIN_DEAD && self.dead_chunks > self.live_chunks / 2 {
            self.compact();
        }
        applied
    }

    /// Remove the files at or below `base_key`; returns how many.
    pub(crate) fn remove_under(&mut self, base_key: &Path) -> usize {
        let mut ids: Vec<u32> = self
            .by_key
            .iter()
            .filter(|(k, _)| k.starts_with(base_key))
            .map(|(_, &id)| id)
            .collect();
        ids.sort_unstable();
        self.remove_ids(&ids);
        if self.dead_chunks > COMPACT_MIN_DEAD && self.dead_chunks > self.live_chunks / 2 {
            self.compact();
        }
        ids.len()
    }

    fn slot(&self, id: u32) -> &FileMeta {
        self.files[id as usize]
            .as_ref()
            .expect("by_key only holds live file slots")
    }

    fn intern(&mut self, term: &str) -> u32 {
        if let Some(&id) = self.terms.get(term) {
            return id;
        }
        let id = self.postings.len() as u32;
        self.terms.insert(term.into(), id);
        self.postings.push(Vec::new());
        id
    }

    /// Add a file. `false` when the chunk id space is exhausted.
    fn insert(&mut self, pf: ParsedFile) -> bool {
        let ParsedFile {
            path,
            stamp,
            chunks,
        } = pf;
        if self.chunks.len() + chunks.len() > MAX_CHUNKS {
            return false;
        }
        let id = self.free_files.pop().unwrap_or_else(|| {
            self.files.push(None);
            (self.files.len() - 1) as u32
        });
        let chunk_first = self.chunks.len() as u32;
        let chunk_count = chunks.len() as u32;
        let mut file_terms: Vec<u32> = Vec::new();
        for pc in chunks {
            let cid = self.chunks.len() as u32;
            self.chunks.push(ChunkMeta {
                file: id,
                start: pc.start,
                end: pc.end,
                len: pc.len,
            });
            self.total_len += u64::from(pc.len);
            for (term, tf) in pc.terms {
                let t = self.intern(&term);
                self.postings[t as usize].push(Posting { chunk: cid, tf });
                file_terms.push(t);
            }
        }
        file_terms.sort_unstable();
        file_terms.dedup();
        self.live_chunks += chunk_count as usize;
        self.total_bytes += stamp.size;
        self.seq += 1;
        self.by_key.insert(path_key(&path), id);
        self.files[id as usize] = Some(FileMeta {
            path: Arc::from(path),
            stamp,
            seq: self.seq,
            chunk_first,
            chunk_count,
            terms: file_terms.into_boxed_slice(),
        });
        true
    }

    /// Remove files by slot; `ids` is sorted and free of duplicates.
    fn remove_ids(&mut self, ids: &[u32]) {
        if ids.is_empty() {
            return;
        }
        let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(ids.len());
        let mut terms: Vec<u32> = Vec::new();
        for &id in ids {
            let f = self.slot(id);
            if f.chunk_count > 0 {
                ranges.push((f.chunk_first, f.chunk_first + f.chunk_count));
                terms.extend_from_slice(&f.terms);
            }
        }
        ranges.sort_unstable();
        terms.sort_unstable();
        terms.dedup();
        if ranges.len() <= DRAIN_MAX_FILES {
            for &t in &terms {
                let list = &mut self.postings[t as usize];
                // Back to front so earlier ranges keep their positions.
                for &(a, b) in ranges.iter().rev() {
                    let lo = list.partition_point(|p| p.chunk < a);
                    let hi = lo + list[lo..].partition_point(|p| p.chunk < b);
                    list.drain(lo..hi);
                }
                if list.is_empty() {
                    *list = Vec::new();
                }
            }
        } else {
            let mut dead = vec![false; self.chunks.len()];
            for &(a, b) in &ranges {
                dead[a as usize..b as usize].fill(true);
            }
            for &t in &terms {
                let list = &mut self.postings[t as usize];
                list.retain(|p| !dead[p.chunk as usize]);
                if list.is_empty() {
                    *list = Vec::new();
                }
            }
        }
        for &id in ids {
            let Some(f) = self.files[id as usize].take() else {
                continue;
            };
            for c in f.chunk_first..f.chunk_first + f.chunk_count {
                let meta = &mut self.chunks[c as usize];
                self.total_len -= u64::from(meta.len);
                meta.file = NONE;
            }
            self.live_chunks -= f.chunk_count as usize;
            self.dead_chunks += f.chunk_count as usize;
            self.total_bytes -= f.stamp.size;
            self.by_key.remove(&path_key(&f.path));
            self.free_files.push(id);
        }
    }

    /// Renumber the live chunks densely and forget terms nobody uses. Order is
    /// preserved, so posting lists stay sorted and file ranges contiguous.
    fn compact(&mut self) {
        let mut remap = vec![NONE; self.chunks.len()];
        let mut chunks = Vec::with_capacity(self.live_chunks);
        for (old, c) in self.chunks.iter().enumerate() {
            if c.file != NONE {
                remap[old] = chunks.len() as u32;
                chunks.push(*c);
            }
        }
        let old_terms = std::mem::take(&mut self.terms);
        let mut term_remap = vec![NONE; self.postings.len()];
        let mut terms = HashMap::with_capacity(old_terms.len());
        let mut postings: Vec<Vec<Posting>> = Vec::with_capacity(old_terms.len());
        for (text, old) in old_terms {
            let mut list = std::mem::take(&mut self.postings[old as usize]);
            if list.is_empty() {
                continue;
            }
            for p in &mut list {
                p.chunk = remap[p.chunk as usize];
            }
            list.shrink_to_fit();
            term_remap[old as usize] = postings.len() as u32;
            terms.insert(text, postings.len() as u32);
            postings.push(list);
        }
        for f in self.files.iter_mut().flatten() {
            if f.chunk_count > 0 {
                f.chunk_first = remap[f.chunk_first as usize];
            }
            for t in f.terms.iter_mut() {
                *t = term_remap[*t as usize];
            }
        }
        self.chunks = chunks;
        self.terms = terms;
        self.postings = postings;
        self.dead_chunks = 0;
    }

    /// Rank the files for `terms` (distinct, lowercased): BM25 over chunks,
    /// scaled by the squared share of the query's terms the chunk contains,
    /// best chunk per file, ties broken by path.
    pub(crate) fn rank(&self, terms: &[String], limit: usize) -> Ranked {
        let none = Ranked {
            candidates: Vec::new(),
            matched: 0,
        };
        if self.live_chunks == 0 || terms.is_empty() {
            return none;
        }
        let n = self.live_chunks as f64;
        let avgdl = (self.total_len as f64 / n).max(1.0);
        let present: Vec<(usize, f64)> = terms
            .iter()
            .filter_map(|t| {
                let id = *self.terms.get(t.as_str())? as usize;
                let df = self.postings[id].len();
                (df > 0).then(|| (id, (1.0 + (n - df as f64 + 0.5) / (df as f64 + 0.5)).ln()))
            })
            .collect();
        if present.is_empty() {
            return none;
        }
        let q = present.len() as f64;
        let mut score = vec![0f64; self.chunks.len()];
        let mut hits = vec![0u8; self.chunks.len()];
        let mut touched: Vec<u32> = Vec::new();
        // Every term adds to a chunk exactly once and in query order, so a
        // chunk's score never depends on posting order or chunk numbering.
        for &(id, idf) in &present {
            for p in &self.postings[id] {
                let c = p.chunk as usize;
                let dl = f64::from(self.chunks[c].len);
                let tf = f64::from(p.tf);
                score[c] += idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * dl / avgdl));
                if hits[c] == 0 {
                    touched.push(p.chunk);
                }
                hits[c] = hits[c].saturating_add(1);
            }
        }
        // Best chunk per file.
        let mut best: Vec<(f64, u32)> = vec![(0.0, NONE); self.files.len()];
        for &c in &touched {
            let meta = &self.chunks[c as usize];
            let coverage = f64::from(hits[c as usize]) / q;
            let s = round4(score[c as usize] * coverage * coverage);
            let slot = &mut best[meta.file as usize];
            if slot.1 == NONE
                || s > slot.0
                || (s == slot.0 && meta.start < self.chunks[slot.1 as usize].start)
            {
                *slot = (s, c);
            }
        }
        let mut ranked: Vec<(f64, u32, u32)> = best
            .iter()
            .enumerate()
            .filter(|(_, (_, c))| *c != NONE)
            .map(|(f, &(s, c))| (s, f as u32, c))
            .collect();
        let matched = ranked.len();
        let order = |a: &(f64, u32, u32), b: &(f64, u32, u32)| {
            b.0.total_cmp(&a.0)
                .then_with(|| self.slot(a.1).path.cmp(&self.slot(b.1).path))
        };
        if limit < ranked.len() {
            ranked.select_nth_unstable_by(limit, order);
            ranked.truncate(limit);
        }
        ranked.sort_unstable_by(order);
        let candidates = ranked
            .into_iter()
            .map(|(s, f, c)| {
                let file = self.slot(f);
                let chunk = &self.chunks[c as usize];
                Candidate {
                    path: Arc::clone(&file.path),
                    stamp: file.stamp,
                    start: chunk.start,
                    end: chunk.end,
                    score: s,
                }
            })
            .collect();
        Ranked {
            candidates,
            matched,
        }
    }

    #[cfg(test)]
    pub(crate) fn table_len(&self) -> usize {
        self.chunks.len()
    }

    #[cfg(test)]
    pub(crate) fn dead_chunks(&self) -> usize {
        self.dead_chunks
    }

    /// Check every structural invariant; panics with a description when one is broken.
    #[cfg(test)]
    pub(crate) fn assert_consistent(&self) {
        let live: usize = self.chunks.iter().filter(|c| c.file != NONE).count();
        assert_eq!(live, self.live_chunks, "live chunk count");
        assert_eq!(
            self.chunks.len() - live,
            self.dead_chunks,
            "dead chunk count"
        );
        let len_sum: u64 = self
            .chunks
            .iter()
            .filter(|c| c.file != NONE)
            .map(|c| u64::from(c.len))
            .sum();
        assert_eq!(len_sum, self.total_len, "total chunk length");
        let mut bytes = 0;
        let mut counted = 0;
        for (id, f) in self.files.iter().enumerate() {
            let Some(f) = f else { continue };
            counted += 1;
            bytes += f.stamp.size;
            assert_eq!(self.by_key.get(&path_key(&f.path)), Some(&(id as u32)));
            for c in f.chunk_first..f.chunk_first + f.chunk_count {
                assert_eq!(self.chunks[c as usize].file, id as u32, "chunk owner");
            }
            let mut distinct = f.terms.to_vec();
            distinct.sort_unstable();
            distinct.dedup();
            assert_eq!(distinct.len(), f.terms.len(), "file terms are distinct");
        }
        assert_eq!(counted, self.by_key.len(), "file count");
        assert_eq!(bytes, self.total_bytes, "total bytes");
        let mut posting_total = 0;
        for (t, list) in self.postings.iter().enumerate() {
            assert!(
                list.windows(2).all(|w| w[0].chunk < w[1].chunk),
                "postings of term {t} are not strictly sorted"
            );
            for p in list {
                assert_ne!(
                    self.chunks[p.chunk as usize].file, NONE,
                    "posting of a dead chunk"
                );
                assert!(p.tf > 0);
            }
            posting_total += list.len();
        }
        let from_files: usize = self
            .files
            .iter()
            .flatten()
            .map(|f| {
                f.terms
                    .iter()
                    .map(|&t| {
                        self.postings[t as usize]
                            .iter()
                            .filter(|p| {
                                p.chunk >= f.chunk_first && p.chunk < f.chunk_first + f.chunk_count
                            })
                            .count()
                    })
                    .sum::<usize>()
            })
            .sum();
        assert_eq!(
            posting_total, from_files,
            "every posting belongs to a listed term of its file"
        );
    }
}
