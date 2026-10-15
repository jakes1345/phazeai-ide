//! In-memory log of what was sent to AI providers (and what was blocked).
//!
//! A bounded ring of the last [`CAPACITY`] entries, process-wide, not
//! persisted. Every request made through a client built by
//! [`crate::llm::ProviderRegistry`] is recorded by [`super::GuardedClient`]:
//! time, provider, model, approximate characters sent, and the project files
//! whose contents were attached since the previous request.
//!
//! "Files attached" is tracked per logical request with a task-local scope:
//! code that attaches file contents calls [`note_file`] (tools such as
//! `read_file` / `grep` do this while the agent runs) or wraps the request in
//! [`scope`] with the files it put in the prompt (@mentions, inline
//! completion, design). Files noted outside any scope are not attributed.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Maximum number of entries kept.
pub const CAPACITY: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditKind {
    /// A request that was sent.
    Request {
        provider: String,
        model: String,
        /// Characters of message text + tool-call arguments + tool schemas.
        chars: usize,
        /// Project files whose contents were attached since the previous request.
        files: Vec<String>,
    },
    /// A file that was kept out of an AI request.
    BlockedFile {
        path: String,
        reason: String,
        /// Which feature tried (e.g. "read_file", "@mention", "inline completion").
        via: String,
    },
    /// A request refused because the project is local-only.
    BlockedProvider {
        provider: String,
        model: String,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Monotonic id (unique within the process).
    pub id: u64,
    pub time: chrono::DateTime<chrono::Local>,
    pub kind: AuditKind,
}

impl AuditEntry {
    pub fn is_blocked(&self) -> bool {
        !matches!(self.kind, AuditKind::Request { .. })
    }

    /// One-line summary for logs and the CLI.
    pub fn summary(&self) -> String {
        let t = self.time.format("%H:%M:%S");
        match &self.kind {
            AuditKind::Request {
                provider,
                model,
                chars,
                files,
            } => {
                let files = if files.is_empty() {
                    "no project files".to_string()
                } else {
                    files.join(", ")
                };
                format!("{t}  request  {provider}/{model}  ~{chars} chars  [{files}]")
            }
            AuditKind::BlockedFile { path, reason, via } => {
                format!("{t}  BLOCKED  {path} ({via}): {reason}")
            }
            AuditKind::BlockedProvider {
                provider, model, ..
            } => format!("{t}  BLOCKED  request to {provider}/{model}: project is local-only"),
        }
    }
}

static LOG: Mutex<VecDeque<AuditEntry>> = Mutex::new(VecDeque::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn push(kind: AuditKind) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let entry = AuditEntry {
        id,
        time: chrono::Local::now(),
        kind,
    };
    let mut log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if log.len() >= CAPACITY {
        log.pop_front();
    }
    log.push_back(entry);
    id
}

/// Record a request that is about to be sent. Returns the entry id.
pub fn record_request(provider: &str, model: &str, chars: usize, files: Vec<String>) -> u64 {
    push(AuditKind::Request {
        provider: provider.to_string(),
        model: model.to_string(),
        chars,
        files,
    })
}

/// Record a file that was kept out of a request.
pub fn record_blocked_file(path: &str, reason: &str, via: &str) -> u64 {
    push(AuditKind::BlockedFile {
        path: path.to_string(),
        reason: reason.to_string(),
        via: via.to_string(),
    })
}

/// Record a request refused because the project is local-only.
pub fn record_blocked_provider(provider: &str, model: &str, reason: &str) -> u64 {
    push(AuditKind::BlockedProvider {
        provider: provider.to_string(),
        model: model.to_string(),
        reason: reason.to_string(),
    })
}

/// Up to `limit` entries, newest first.
pub fn recent(limit: usize) -> Vec<AuditEntry> {
    let log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    log.iter().rev().take(limit).cloned().collect()
}

/// Id of the newest entry (0 when empty) — cheap change detection for UIs.
pub fn latest_id() -> u64 {
    let log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    log.back().map(|e| e.id).unwrap_or(0)
}

tokio::task_local! {
    static SHARED: RefCell<Vec<String>>;
}

fn display(path: &Path) -> String {
    super::active().display_path(path)
}

fn dedup_into(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

static NEXT_SCOPE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Attribute `path` to the next [`scope`] that opens (any caller). For hosts
/// that put file contents into a prompt and hand it to an agent run later,
/// such as the CLI's `/add`.
pub fn attach_to_next_scope(path: &Path) {
    let shown = display(path);
    let mut next = NEXT_SCOPE.lock().unwrap_or_else(|e| e.into_inner());
    dedup_into(&mut next, shown);
}

/// Run `fut` with a fresh "files attached" scope, pre-seeded with `files`
/// (files whose contents the caller put into the prompt) and anything queued
/// with [`attach_to_next_scope`].
pub async fn scope<F: Future>(files: Vec<PathBuf>, fut: F) -> F::Output {
    let mut seeded = std::mem::take(&mut *NEXT_SCOPE.lock().unwrap_or_else(|e| e.into_inner()));
    for f in &files {
        dedup_into(&mut seeded, display(f));
    }
    SHARED.scope(RefCell::new(seeded), fut).await
}

/// Whether the caller is inside a [`scope`].
pub fn in_scope() -> bool {
    SHARED.try_with(|_| ()).is_ok()
}

/// Note that `path`'s contents are being attached to the next request.
/// Ignored outside a [`scope`].
pub fn note_file(path: &Path) {
    let shown = display(path);
    let _ = SHARED.try_with(|c| dedup_into(&mut c.borrow_mut(), shown));
}

/// Take the files noted since the last request (empty outside a scope).
pub(crate) fn take_files() -> Vec<String> {
    SHARED
        .try_with(|c| std::mem::take(&mut *c.borrow_mut()))
        .unwrap_or_default()
}
