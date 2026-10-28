//! Public value types of the search engine.

use serde::Serialize;

/// Hard caps on what one [`super::CodeSearch`] indexes. Hitting a cap is never
/// silent: the build reports `truncated` and the search tools say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLimits {
    /// Most files in the index at once.
    pub max_files: usize,
    /// Larger files are skipped.
    pub max_file_bytes: u64,
    /// Most bytes of file content in the index at once.
    pub max_total_bytes: u64,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_files: 50_000,
            max_file_bytes: 1 << 20,
            max_total_bytes: 256 << 20,
        }
    }
}

/// What one [`super::CodeSearch::build_blocking`] call did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BuildStats {
    /// Files added to the index.
    pub indexed: usize,
    /// Files re-indexed because they changed on disk.
    pub updated: usize,
    /// Files whose (mtime, size) matched the index; not read again.
    pub unchanged: usize,
    /// Files dropped from the index: deleted, newly ignored or denied, no
    /// longer indexable (too large, binary, not UTF-8) or over a limit. Always
    /// `total_files` after = before + `indexed` - `removed`.
    pub removed: usize,
    /// Candidate files that could not be indexed: too large, binary or not UTF-8.
    pub skipped: usize,
    /// Unreadable files and directories, and paths that do not exist.
    pub errors: usize,
    /// Files (or a denied directory, once) kept out by the privacy policy.
    pub privacy_skipped: usize,
    /// Files in the index when the call finished.
    pub total_files: usize,
    /// Chunks in the index when the call finished.
    pub chunks: usize,
    /// A [`SearchLimits`] cap stopped the build; some files are not indexed.
    pub truncated: bool,
    pub elapsed_ms: u64,
    /// Human-readable problems with the requested paths.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// A snapshot of the index; reading it never waits for a build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct IndexStatus {
    /// At least one build has completed for the current root.
    pub built: bool,
    /// A build is running right now.
    pub building: bool,
    pub files: usize,
    pub chunks: usize,
    /// The most recent build stopped at a [`SearchLimits`] cap.
    pub truncated: bool,
}

/// One ranked chunk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHit {
    /// Absolute path.
    pub file: String,
    /// First line of `snippet`, 1-based.
    pub start_line: usize,
    /// Last line of `snippet`, 1-based and inclusive.
    pub end_line: usize,
    /// BM25 score of the best chunk in the file, rounded to 4 decimals.
    #[serde(serialize_with = "serialize_score")]
    pub score: f32,
    pub snippet: String,
}

/// The answer to one query, best match first, at most one hit per file.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SearchResults {
    pub matches: Vec<SearchHit>,
    /// Higher-ranked files left out because the privacy policy denies them.
    #[serde(skip_serializing_if = "is_zero")]
    pub privacy_skipped: usize,
    /// The index hit a [`SearchLimits`] cap, so matches may be missing.
    #[serde(skip_serializing_if = "is_false")]
    pub index_truncated: bool,
}

/// JSON numbers are f64: widening an f32 like 1.2346 would print as
/// 1.2345999479293823, so scores are written as the 4-decimal f64 they stand for.
fn serialize_score<S: serde::Serializer>(score: &f32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64((f64::from(*score) * 1e4).round() / 1e4)
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("search index I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid search query: {0}")]
    InvalidQuery(String),
    #[error("the search root changed while the operation was running")]
    RootChanged,
    #[error("search worker failed: {0}")]
    Worker(String),
}
