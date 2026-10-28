//! The IDE side of in-process code search (`phazeai_core::CodeSearch`).
//!
//! The engine indexes and ranks; this module connects it to the window. Builds
//! and searches run on worker threads and report back through
//! `create_ext_action`, so the Floem UI thread never waits on them. While a
//! worker runs, the engine's own `status()` (atomics only) is mirrored into the
//! signals the Search panel reads, so "Indexing… 1,204 files" is the engine's
//! real progress, not a guess.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use floem::{
    action::exec_after,
    ext_event::create_ext_action,
    reactive::{create_effect, Scope, SignalGet, SignalUpdate},
};
use phazeai_core::privacy;
use phazeai_core::search::{
    looks_like_project, BuildStats, IndexStatus, SearchError, SearchLimits, SearchResults,
};
use phazeai_core::CodeSearch;

use crate::domain_state::ProjectState;

/// Hits shown per search.
const TOP_K: usize = 8;
/// How often the status line follows the engine while a worker is running.
const STATUS_POLL: Duration = Duration::from_millis(250);
/// Lines of a hit's snippet shown in the panel, and the longest line kept.
const PREVIEW_LINES: usize = 3;
const PREVIEW_COLS: usize = 140;

/// A line telling the user how the last search or build went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub is_error: bool,
}

impl Notice {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Status line for a window whose settings turn code search off.
pub const TURNED_OFF: &str =
    "Code search is turned off in settings (enabled = false under [search] in config.toml).";

/// Status line before anything has been indexed.
pub fn initial_status(enabled: bool) -> String {
    if enabled {
        index_status_text(&IndexStatus::default(), &SearchLimits::default())
    } else {
        TURNED_OFF.to_string()
    }
}

/// Start code search for a freshly created window: index the workspace in the
/// background when `auto_index` is on and the workspace looks like a project,
/// and follow the workspace when the user opens another folder.
///
/// The workspace is the git root of the working directory, or the working
/// directory itself, and a launcher starts the IDE in `/` or in the home
/// directory. Such a folder is left for the first search to index, if the user
/// ever asks, rather than walked on every launch.
pub fn init(p: &ProjectState) {
    if !p.code_search_enabled {
        return;
    }
    if auto_index_wanted(p.code_search_auto_index, &p.workspace_root.get_untracked()) {
        start_build(p);
    }
    let p = p.clone();
    create_effect(move |previous: Option<PathBuf>| {
        let root = p.workspace_root.get();
        if previous.as_ref().is_some_and(|old| *old != root) {
            switch_root(&p, root.clone());
        }
        root
    });
}

/// Whether the index of `root` should be built without a search asking for it:
/// `[search] auto_index` is on and `root` looks like a project.
fn auto_index_wanted(setting: bool, root: &Path) -> bool {
    setting && looks_like_project(root)
}

/// Point the engine at `root`, with the privacy policy of that project.
///
/// The policy follows the workspace in an effect of its own
/// (`panels::privacy::install`), and Floem runs the subscribers of a signal in
/// no particular order, so the build that starts next could otherwise read the
/// old project's policy and index files the new one denies. Setting the project
/// here, first, is idempotent: the other effect then does it again.
fn follow_workspace(search: &CodeSearch, root: &Path) {
    privacy::set_active_project(root);
    search.reset_root(root);
}

/// The workspace changed: drop the old index and its results, then index the
/// new folder if `auto_index` is on.
///
/// `reset_root` runs here on the UI thread. It waits at most for the commit in
/// flight and hands the old index to a thread to be freed, so it does not
/// stall the window however big the old project was.
fn switch_root(p: &ProjectState, root: PathBuf) {
    follow_workspace(&p.code_search, &root);
    p.code_search_seq.update(|n| *n += 1);
    p.code_search_results.set(Vec::new());
    p.code_search_notice.set(None);
    // A build for the old root may still be winding down; it must not stop the
    // new root from being indexed.
    p.code_search_building.set(false);
    if auto_index_wanted(p.code_search_auto_index, &root) {
        start_build(p);
    } else {
        sync_status(p);
    }
}

/// Bring the index in line with the disk (the Reindex button, auto-index).
/// Cheap when little changed: unchanged files are not read again.
pub fn start_build(p: &ProjectState) {
    if !p.code_search_enabled || p.code_search_building.get_untracked() {
        return;
    }
    p.code_search_notice.set(None);
    show_indexing(p);

    let running = Arc::new(AtomicBool::new(true));
    let scope = Scope::new();
    let done = {
        let p = p.clone();
        let running = running.clone();
        create_ext_action(scope, move |result: Result<BuildStats, SearchError>| {
            scope.dispose();
            running.store(false, Ordering::Release);
            match result {
                Ok(stats) => p.code_search_notice.set(build_notice(&stats)),
                // The folder changed under the build; the new root's own build
                // reports for itself.
                Err(SearchError::RootChanged) => {}
                Err(e) => p
                    .code_search_notice
                    .set(Some(Notice::error(format!("Indexing failed: {e}")))),
            }
            sync_status(&p);
        })
    };
    let engine = p.code_search.clone();
    std::thread::spawn(move || done(guarded(|| engine.build_blocking(&[]))));
    watch_status(p.clone(), running);
}

/// Search the workspace for `query` and publish the hits.
///
/// The first search on a window whose index is not built yet builds it first,
/// so it can take a while on a big tree; the status line says "Indexing…".
pub fn run_search(p: &ProjectState, query: &str) {
    let query = query.trim().to_string();
    if !p.code_search_enabled || query.is_empty() {
        return;
    }
    let seq = p.code_search_seq.get_untracked() + 1;
    p.code_search_seq.set(seq);

    let running = Arc::new(AtomicBool::new(true));
    let first = !p.code_search.status().built;
    if first {
        show_indexing(p);
    }

    let scope = Scope::new();
    let done = {
        let p = p.clone();
        let running = running.clone();
        let query = query.clone();
        create_ext_action(scope, move |result: Result<SearchResults, SearchError>| {
            scope.dispose();
            running.store(false, Ordering::Release);
            sync_status(&p);
            // A newer search, or another workspace, owns the results now.
            if p.code_search_seq.get_untracked() != seq {
                return;
            }
            match result {
                Ok(found) => {
                    p.code_search_notice.set(search_notice(&query, &found));
                    p.code_search_results.set(found.matches);
                }
                Err(e) => {
                    p.code_search_results.set(Vec::new());
                    p.code_search_notice
                        .set(Some(Notice::error(format!("Code search failed: {e}"))));
                }
            }
        })
    };
    let engine = p.code_search.clone();
    std::thread::spawn(move || done(guarded(|| engine.search_blocking(&query, TOP_K))));
    if first {
        watch_status(p.clone(), running);
    }
}

/// Run `work`, turning a panic into an error. The UI waits on the answer to
/// stop its progress timer and re-enable the buttons, so it must always come.
fn guarded<T>(work: impl FnOnce() -> Result<T, SearchError>) -> Result<T, SearchError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|_| {
        Err(SearchError::Worker(
            "the worker thread panicked".to_string(),
        ))
    })
}

/// Re-index a file that was just written. Call from a worker thread: the
/// refresh reads the file and walks its parent directories for ignore rules.
///
/// An index that does not exist yet needs no help; the build that creates it
/// reads the file as it is then.
pub fn refresh_saved_file(search: &CodeSearch, path: &Path) {
    let status = search.status();
    if status.built || status.building {
        search.refresh_file(path);
    }
}

/// Show "Indexing…" straight away. The worker thread has not necessarily set
/// the engine's own flag yet, and the next poll would otherwise flicker it back.
fn show_indexing(p: &ProjectState) {
    p.code_search_building.set(true);
    p.code_search_status.set(index_status_text(
        &IndexStatus {
            building: true,
            ..p.code_search.status()
        },
        &p.code_search.limits(),
    ));
}

/// Copy the engine's status into the panel signals. Only writes what changed,
/// so an idle poll costs the UI nothing.
fn sync_status(p: &ProjectState) {
    let status = p.code_search.status();
    let text = index_status_text(&status, &p.code_search.limits());
    if p.code_search_building.get_untracked() != status.building {
        p.code_search_building.set(status.building);
    }
    if p.code_search_status.get_untracked() != text {
        p.code_search_status.set(text);
    }
}

/// Follow the engine's progress until `running` clears. Timers fire on the UI
/// thread, so the signal writes need no hand-off.
fn watch_status(p: ProjectState, running: Arc<AtomicBool>) {
    exec_after(STATUS_POLL, move |_| {
        if !running.load(Ordering::Acquire) {
            return;
        }
        sync_status(&p);
        watch_status(p, running);
    });
}

/// "1,204" for `1204`.
fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "1 file", "2 files", "1,204 files".
fn count(n: usize, noun: &str) -> String {
    format!(
        "{} {noun}{}",
        group_digits(n),
        if n == 1 { "" } else { "s" }
    )
}

/// What the index looks like right now, in one line.
pub fn index_status_text(status: &IndexStatus, limits: &SearchLimits) -> String {
    if status.building {
        return if status.files == 0 {
            "Indexing…".to_string()
        } else {
            format!("Indexing… {}", count(status.files, "file"))
        };
    }
    if !status.built {
        return "Index not built".to_string();
    }
    if status.truncated {
        let cap = if status.files >= limits.max_files {
            format!("{} files", group_digits(limits.max_files))
        } else {
            format!("{} MiB", limits.max_total_bytes.div_ceil(1 << 20))
        };
        return format!(
            "Index truncated at {cap} · {}",
            count(status.chunks, "chunk")
        );
    }
    format!(
        "Indexed {} · {}",
        count(status.files, "file"),
        count(status.chunks, "chunk")
    )
}

/// What a finished build says beyond the status line, if anything: files the
/// index could not hold because they were unreadable or kept out by policy.
fn build_notice(stats: &BuildStats) -> Option<Notice> {
    let mut parts = Vec::new();
    if stats.errors > 0 {
        parts.push(format!("{} unreadable", group_digits(stats.errors)));
    }
    if stats.privacy_skipped > 0 {
        parts.push(format!(
            "{} excluded by the privacy policy",
            group_digits(stats.privacy_skipped)
        ));
    }
    (!parts.is_empty()).then(|| Notice::info(format!("Not indexed: {}", parts.join(" · "))))
}

/// What a finished search says beyond the hits themselves.
fn search_notice(query: &str, results: &SearchResults) -> Option<Notice> {
    let mut parts = Vec::new();
    if results.matches.is_empty() {
        let shown: String = query.chars().take(60).collect();
        let ellipsis = if query.chars().count() > 60 {
            "…"
        } else {
            ""
        };
        parts.push(format!("No code matches for “{shown}{ellipsis}”"));
    }
    if results.privacy_skipped > 0 {
        parts.push(format!(
            "{} hidden by the privacy policy",
            count(results.privacy_skipped, "file")
        ));
    }
    if results.index_truncated {
        parts.push("the index is truncated, so some files are not searched".to_string());
    }
    (!parts.is_empty()).then(|| Notice::info(parts.join(" · ")))
}

/// `file` relative to the workspace root, or as given when it is outside it.
pub fn display_path(root: &Path, file: &str) -> String {
    Path::new(file)
        .strip_prefix(root)
        .map(|rel| rel.display().to_string())
        .unwrap_or_else(|_| file.to_string())
}

/// The first lines of a snippet for the results list: leading blank lines
/// dropped, tabs widened, the common indentation removed, long lines cut.
pub fn snippet_preview(snippet: &str) -> Vec<String> {
    let lines: Vec<String> = snippet
        .lines()
        .map(|l| l.replace('\t', "    ").trim_end().to_string())
        .skip_while(|l| l.is_empty())
        .take(PREVIEW_LINES)
        .collect();
    let indent = lines
        .iter()
        .filter(|l| !l.is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            let body = l.get(indent..).unwrap_or("");
            if body.chars().count() > PREVIEW_COLS {
                let cut: String = body.chars().take(PREVIEW_COLS - 1).collect();
                format!("{cut}…")
            } else {
                body.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use phazeai_core::search::SearchHit;

    fn status(built: bool, building: bool, files: usize, chunks: usize) -> IndexStatus {
        IndexStatus {
            built,
            building,
            files,
            chunks,
            truncated: false,
        }
    }

    fn hit(file: &str) -> SearchHit {
        SearchHit {
            file: file.to_string(),
            start_line: 1,
            end_line: 3,
            score: 1.0,
            snippet: "fn main() {}".to_string(),
        }
    }

    #[test]
    fn saved_files_join_an_index_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("saved.rs");
        std::fs::write(&file, "fn before_save() {}\n").unwrap();
        let engine = CodeSearch::new(dir.path());

        // No index yet: the build that makes one will read the file as it is.
        refresh_saved_file(&engine, &file);
        assert_eq!(engine.status().files, 0);

        engine.build_blocking(&[]).unwrap();
        std::fs::write(&file, "fn after_the_save_with_a_longer_name() {}\n").unwrap();
        refresh_saved_file(&engine, &file);
        let found = engine.search_blocking("after_the_save", 5).unwrap();
        assert_eq!(found.matches.len(), 1, "{found:?}");
        assert!(found.matches[0].file.ends_with("saved.rs"));
    }

    #[test]
    fn following_a_workspace_sets_its_privacy_policy_before_anything_is_indexed() {
        let old = tempfile::tempdir().unwrap();
        let new = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(new.path().join(".phazeai")).unwrap();
        std::fs::write(
            new.path().join(".phazeai/privacy.toml"),
            "deny = [\"finance/**\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(new.path().join("finance")).unwrap();
        std::fs::write(new.path().join("finance/ledger.md"), "ledgerword\n").unwrap();
        std::fs::write(new.path().join("main.rs"), "fn mainword() {}\n").unwrap();

        let engine = CodeSearch::new(old.path());
        privacy::set_active_project(old.path());
        follow_workspace(&engine, new.path());
        assert_eq!(engine.root(), new.path());
        // What the build that follows will read.
        assert!(privacy::active().is_denied(&new.path().join("finance/ledger.md")));
        let stats = engine.build_blocking(&[]).unwrap();
        assert_eq!(stats.total_files, 1, "{stats:?}");
        assert!(engine
            .search_blocking("ledgerword", 5)
            .unwrap()
            .matches
            .is_empty());
        privacy::clear_active_project();
    }

    #[test]
    fn only_a_folder_that_looks_like_a_project_is_indexed_unasked() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert!(auto_index_wanted(true, &repo));
        assert!(!auto_index_wanted(false, &repo), "the setting still rules");
        assert!(!auto_index_wanted(true, &plain));
        // What a desktop launcher hands the IDE as its working directory.
        let top = std::env::temp_dir()
            .ancestors()
            .last()
            .unwrap()
            .to_path_buf();
        assert!(!auto_index_wanted(true, &top));
    }

    #[test]
    fn a_panicking_worker_still_answers() {
        let err = guarded::<()>(|| panic!("boom")).unwrap_err();
        assert!(matches!(err, SearchError::Worker(_)), "{err}");
        assert_eq!(guarded(|| Ok(7)).unwrap(), 7);
    }

    #[test]
    fn digits_are_grouped_in_threes() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(1_204), "1,204");
        assert_eq!(group_digits(50_000), "50,000");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    #[test]
    fn counts_pluralise() {
        assert_eq!(count(0, "file"), "0 files");
        assert_eq!(count(1, "file"), "1 file");
        assert_eq!(count(2, "chunk"), "2 chunks");
    }

    #[test]
    fn status_line_follows_the_engine() {
        let limits = SearchLimits::default();
        assert_eq!(
            index_status_text(&status(false, false, 0, 0), &limits),
            "Index not built"
        );
        assert_eq!(
            index_status_text(&status(false, true, 0, 0), &limits),
            "Indexing…"
        );
        assert_eq!(
            index_status_text(&status(false, true, 1_204, 0), &limits),
            "Indexing… 1,204 files"
        );
        // A rebuild of an existing index is still "indexing".
        assert_eq!(
            index_status_text(&status(true, true, 1, 3), &limits),
            "Indexing… 1 file"
        );
        assert_eq!(
            index_status_text(&status(true, false, 1_204, 5_330), &limits),
            "Indexed 1,204 files · 5,330 chunks"
        );
        assert_eq!(
            index_status_text(&status(true, false, 1, 1), &limits),
            "Indexed 1 file · 1 chunk"
        );
    }

    #[test]
    fn a_truncated_index_names_the_cap_that_stopped_it() {
        let limits = SearchLimits::default();
        let by_files = IndexStatus {
            truncated: true,
            ..status(true, false, 50_000, 120_000)
        };
        assert_eq!(
            index_status_text(&by_files, &limits),
            "Index truncated at 50,000 files · 120,000 chunks"
        );
        let by_bytes = IndexStatus {
            truncated: true,
            ..status(true, false, 31_000, 90_000)
        };
        assert_eq!(
            index_status_text(&by_bytes, &limits),
            "Index truncated at 256 MiB · 90,000 chunks"
        );
    }

    #[test]
    fn disabled_windows_say_so() {
        assert_eq!(initial_status(false), TURNED_OFF);
        assert_eq!(initial_status(true), "Index not built");
        assert!(TURNED_OFF.contains("turned off"));
    }

    #[test]
    fn a_clean_build_has_nothing_to_add() {
        assert_eq!(build_notice(&BuildStats::default()), None);
        let skipped_only = BuildStats {
            skipped: 40,
            ..BuildStats::default()
        };
        assert_eq!(build_notice(&skipped_only), None);
    }

    #[test]
    fn a_build_reports_files_it_could_not_index() {
        let stats = BuildStats {
            errors: 2,
            privacy_skipped: 1_300,
            ..BuildStats::default()
        };
        let notice = build_notice(&stats).unwrap();
        assert!(!notice.is_error);
        assert_eq!(
            notice.text,
            "Not indexed: 2 unreadable · 1,300 excluded by the privacy policy"
        );
    }

    #[test]
    fn an_empty_result_names_the_query() {
        let none = SearchResults::default();
        assert_eq!(
            search_notice("parse config", &none).unwrap().text,
            "No code matches for “parse config”"
        );
        let long = "x".repeat(80);
        let text = search_notice(&long, &none).unwrap().text;
        assert!(text.contains(&format!("{}…”", "x".repeat(60))), "{text}");
    }

    #[test]
    fn a_search_reports_what_it_left_out() {
        let found = SearchResults {
            matches: vec![hit("/w/a.rs")],
            privacy_skipped: 1,
            index_truncated: true,
        };
        assert_eq!(
            search_notice("main", &found).unwrap().text,
            "1 file hidden by the privacy policy · the index is truncated, so some files are not searched"
        );
        let plain = SearchResults {
            matches: vec![hit("/w/a.rs")],
            ..SearchResults::default()
        };
        assert_eq!(search_notice("main", &plain), None);
    }

    #[test]
    fn paths_are_shown_relative_to_the_workspace() {
        let root = Path::new("/work/proj");
        assert_eq!(
            display_path(root, "/work/proj/src/lib.rs"),
            Path::new("src").join("lib.rs").display().to_string()
        );
        assert_eq!(display_path(root, "/elsewhere/x.rs"), "/elsewhere/x.rs");
    }

    #[test]
    fn previews_drop_blank_lead_in_and_common_indent() {
        let snippet =
            "\n\n        let a = 1;\n            let b = 2;\n        a + b\n        tail\n";
        assert_eq!(
            snippet_preview(snippet),
            vec!["let a = 1;", "    let b = 2;", "a + b"]
        );
    }

    #[test]
    fn previews_widen_tabs_and_cut_long_lines() {
        assert_eq!(snippet_preview("\tx"), vec!["x"]);
        assert_eq!(snippet_preview("\ta\n\t\tb"), vec!["a", "    b"]);
        let long = "y".repeat(500);
        let preview = snippet_preview(&long);
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].chars().count(), PREVIEW_COLS);
        assert!(preview[0].ends_with('…'));
    }

    #[test]
    fn previews_of_nothing_are_empty() {
        assert!(snippet_preview("").is_empty());
        assert!(snippet_preview("  \n\t\n").is_empty());
    }

    #[test]
    fn previews_keep_multibyte_text_intact() {
        assert_eq!(snippet_preview("  héllo wörld"), vec!["héllo wörld"]);
    }
}
