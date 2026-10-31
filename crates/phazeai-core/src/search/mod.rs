//! In-process code search: BM25 keyword search over line-based chunks.
//!
//! This replaces the old Python sidecar. There is no embedding model and no
//! semantic matching: the index is an inverted index over *tokens*, and the
//! ranking is plain BM25 plus two code-specific touches.
//!
//! * **Identifiers are split.** `parseJsonConfig`, `parse_json_config` and
//!   `ParseJSONConfig` are indexed as themselves *and* as `parse` / `json` /
//!   `config`, so the query `json config` finds all three.
//! * **Locations count.** The words of a file's path (directories and file
//!   stem) are added to every chunk of the file, so `lsp manager` finds
//!   `lsp/manager.rs`.
//!
//! A file is cut into chunks of about 40 lines ([`chunk`]); a hit is the best
//! chunk of a file and carries its exact line range plus a snippet of at most
//! ten lines. Chunk text is not kept in memory: the index stores token
//! statistics, and the few returned files are read again, which also proves
//! they still match what was indexed.
//!
//! What gets indexed is decided by one walker ([`scan::discover`]): it honours
//! `.gitignore` / `.ignore`, never follows symlinks, skips build and vendor
//! directories, hidden files and anything but a list of source extensions,
//! and refuses files that are too large, binary or not UTF-8. The privacy
//! policy (`crate::privacy`) is consulted before a file is read and again for
//! every hit at query time, so a policy change takes effect immediately.
//!
//! The index follows the disk. The first search builds the root; later
//! searches walk it again before ranking (at most about once a second, files
//! are only stat-ed), so a file created, rewritten or deleted since the last
//! look is found by the next search, and every returned hit is read again,
//! which repairs a file that changed after that walk. Building or searching a
//! few paths by hand ([`CodeSearch::build_blocking`] with `paths`) does not
//! count as the root being built.
//!
//! Known limit: change detection compares modification time and size, so an
//! external same-size edit within the file system's timestamp granularity
//! goes unnoticed until the next change. Saves from the editor call
//! [`CodeSearch::refresh_file`], which always re-reads.

mod chunk;
mod engine;
mod index;
mod paths;
mod project;
mod scan;
mod snippet;
mod tokenize;
mod tool;
mod types;

#[cfg(test)]
mod tests;

pub use engine::CodeSearch;
pub use project::looks_like_project;
pub use tool::{BuildIndexTool, CodeSearchTool};
pub use types::{BuildStats, IndexStatus, SearchError, SearchHit, SearchLimits, SearchResults};
