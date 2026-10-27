//! Behaviour tests for the whole engine, plus index-level checks.

use super::index::{Applied, Index, Parsed, ParsedFile, Stamp};
use super::*;
use crate::privacy;
use crate::tools::Tool;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

// ── helpers ──────────────────────────────────────────────────────────────────

fn proj() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    (dir, root)
}

fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, text).unwrap();
    p
}

/// Give `path` the modification time `secs` (distinct values make edits of the
/// same size detectable, whatever the file system's timestamp granularity).
fn touch(path: &Path, secs: u64) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000_000 + secs))
        .unwrap();
}

fn rel(root: &Path, file: &str) -> String {
    Path::new(file)
        .strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn found(res: &SearchResults, root: &Path) -> Vec<String> {
    res.matches.iter().map(|h| rel(root, &h.file)).collect()
}

/// Relative paths of everything in the index, sorted.
fn indexed(cs: &CodeSearch) -> Vec<String> {
    let root = cs.root();
    let mut v: Vec<String> = cs.with_index(|i| {
        i.entries()
            .map(|(k, _)| {
                k.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    });
    v.sort();
    v
}

fn search(cs: &CodeSearch, q: &str) -> SearchResults {
    cs.search_blocking(q, 50).unwrap()
}

fn build(cs: &CodeSearch) -> BuildStats {
    cs.build_blocking(&[]).unwrap()
}

/// Serialises tests that change the process-wide active privacy project.
struct Active {
    _guard: MutexGuard<'static, ()>,
}

fn activate(root: &Path) -> Active {
    let guard = crate::tools::sandbox::tests::SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    privacy::set_active_project(root);
    Active { _guard: guard }
}

impl Drop for Active {
    fn drop(&mut self) {
        privacy::clear_active_project();
    }
}

// ── ranking ──────────────────────────────────────────────────────────────────

const FILLERS: &[(&str, &str)] = &[
    (
        "src/render.rs",
        "fn render_window(frame: &mut Frame) {\n    frame.draw();\n}\n",
    ),
    (
        "src/input.rs",
        "fn handle_key(event: KeyEvent) {\n    // parse the key press\n}\n",
    ),
    (
        "src/net.rs",
        "fn config_path() -> PathBuf {\n    home().join(\"config\")\n}\n",
    ),
    (
        "src/io.rs",
        "fn read_all(path: &Path) -> String {\n    std::fs::read_to_string(path).unwrap()\n}\n",
    ),
    (
        "src/ui.rs",
        "struct Button { label: String }\nimpl Button { fn click(&self) {} }\n",
    ),
];

fn with_fillers(root: &Path) {
    for (p, t) in FILLERS {
        write(root, p, t);
    }
}

#[test]
fn identifier_words_find_camel_snake_and_pascal_case_definitions() {
    let (_d, root) = proj();
    with_fillers(&root);
    write(&root, "src/settings.rs", "pub fn parseJsonConfig(text: &str) -> Config {\n    serde_json::from_str(text).unwrap()\n}\n");
    write(
        &root,
        "tools/loader.py",
        "def parse_json_config(text):\n    return json.loads(text)\n",
    );
    write(
        &root,
        "pkg/loader.go",
        "func ParseJsonConfig(text string) Config {\n\treturn decode(text)\n}\n",
    );
    let cs = CodeSearch::new(&root);
    let r = search(&cs, "parse json config");
    let files = found(&r, &root);
    let top3: Vec<&str> = files.iter().take(3).map(String::as_str).collect();
    for f in ["src/settings.rs", "tools/loader.py", "pkg/loader.go"] {
        assert!(top3.contains(&f), "{f} not in top 3 of {files:?}");
    }
    // The same query phrased as the camelCase identifier finds it too; the Go
    // spelling lowercases to the very same identifier and ties it.
    let exact = found(&search(&cs, "parseJsonConfig"), &root);
    assert!(
        exact[..2].contains(&"src/settings.rs".to_string()),
        "{exact:?}"
    );
    assert!(
        exact[..2].contains(&"pkg/loader.go".to_string()),
        "{exact:?}"
    );
    // A bare 'json config' (no 'parse') still reaches the definitions.
    assert!(found(&search(&cs, "json config"), &root).contains(&"src/settings.rs".to_string()));
}

#[test]
fn defining_file_beats_files_that_only_mention_a_term() {
    let (_d, root) = proj();
    with_fillers(&root);
    write(
        &root,
        "src/settings.rs",
        "pub fn parseJsonConfig(text: &str) -> Config {\n    todo()\n}\n",
    );
    let cs = CodeSearch::new(&root);
    let r = search(&cs, "parse json config");
    assert_eq!(found(&r, &root)[0], "src/settings.rs");
    assert!(r.matches[0].score > r.matches[1].score);
}

#[test]
fn path_words_boost_and_alone_are_enough_to_match() {
    let (_d, root) = proj();
    with_fillers(&root);
    // Same body, different location.
    let body = "fn start() {\n    // lsp manager startup\n}\n";
    write(&root, "src/lsp/manager.rs", body);
    write(&root, "src/misc/util.rs", body);
    // Matches only through its path.
    write(&root, "crates/lsp/manager_pool.rs", "fn new() {}\n");
    let cs = CodeSearch::new(&root);
    let r = search(&cs, "lsp manager");
    let files = found(&r, &root);
    assert_eq!(files[0], "src/lsp/manager.rs");
    let pos = |f: &str| files.iter().position(|x| x == f).unwrap();
    assert!(pos("src/lsp/manager.rs") < pos("src/misc/util.rs"));
    assert!(files.contains(&"crates/lsp/manager_pool.rs".to_string()));
}

#[test]
fn covering_more_query_terms_beats_repeating_one() {
    let (_d, root) = proj();
    with_fillers(&root);
    let repeated = "alpha alpha alpha alpha\n".repeat(30);
    write(&root, "src/repeat.rs", &repeated);
    write(
        &root,
        "src/both.rs",
        "fn f() {\n    alpha();\n    beta();\n}\n",
    );
    let cs = CodeSearch::new(&root);
    let files = found(&search(&cs, "alpha beta"), &root);
    assert_eq!(files[0], "src/both.rs");
    assert!(files.contains(&"src/repeat.rs".to_string()));
}

#[test]
fn equal_scores_are_ordered_by_path_every_time() {
    let (_d, root) = proj();
    with_fillers(&root);
    // Directory names that are not tokens: the three files score identically.
    for d in ["c", "a", "b"] {
        write(&root, &format!("{d}/mod_file.rs"), "fn needle() {}\n");
    }
    let cs = CodeSearch::new(&root);
    let first = search(&cs, "needle");
    let scores: Vec<f32> = first.matches.iter().map(|h| h.score).collect();
    assert_eq!(scores.len(), 3);
    assert!(scores.windows(2).all(|w| w[0] == w[1]), "{scores:?}");
    assert_eq!(
        found(&first, &root),
        ["a/mod_file.rs", "b/mod_file.rs", "c/mod_file.rs"]
    );
    for _ in 0..5 {
        assert_eq!(search(&cs, "needle"), first);
    }
    assert_eq!(search(&CodeSearch::new(&root), "needle"), first);
}

#[test]
fn scores_are_rounded_to_four_decimals() {
    let (_d, root) = proj();
    with_fillers(&root);
    write(&root, "src/a.rs", "fn parse_config() {}\n");
    let cs = CodeSearch::new(&root);
    for h in search(&cs, "parse config").matches {
        let s = f64::from(h.score);
        assert!(((s * 1e4).round() / 1e4 - s).abs() < 1e-6, "{s}");
        assert!(h.score > 0.0);
    }
}

#[test]
fn one_hit_per_file_with_its_best_chunk() {
    let (_d, root) = proj();
    with_fillers(&root);
    let mut text = String::new();
    text.push_str("// weak: needle appears once here\n");
    for i in 0..120 {
        text.push_str(&format!("let filler{i} = {i};\n"));
    }
    text.push('\n');
    for _ in 0..6 {
        text.push_str("needle needle haystack\n");
    }
    write(&root, "src/long.rs", &text);
    let cs = CodeSearch::new(&root);
    let r = search(&cs, "needle");
    assert_eq!(r.matches.len(), 1);
    let h = &r.matches[0];
    assert!(h.start_line > 100, "{h:?}");
    assert!(h.snippet.contains("needle needle haystack"));
}

#[test]
fn line_numbers_describe_exactly_the_snippet() {
    let (_d, root) = proj();
    with_fillers(&root);
    let mut lines: Vec<String> = (1..=200)
        .map(|i| format!("    let value_{i} = compute({i});"))
        .collect();
    lines[122] = "    let unique = zebrafish_marker();".to_string();
    let path = write(&root, "src/big.rs", &(lines.join("\r\n") + "\r\n"));
    let cs = CodeSearch::new(&root);
    let r = search(&cs, "zebrafish marker");
    let h = &r.matches[0];
    assert_eq!(Path::new(&h.file), path);
    assert!(h.start_line <= 123 && 123 <= h.end_line, "{h:?}");
    assert!(h.end_line - h.start_line < 10);
    assert!(!h.snippet.contains('\r'));
    let expected = lines[h.start_line - 1..h.end_line]
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(h.snippet, expected);
    assert!(h.snippet.chars().count() <= 600);
}

#[test]
fn empty_and_stopword_queries_return_nothing_without_building() {
    let (_d, root) = proj();
    write(&root, "a.rs", "fn main() {}\n");
    let cs = CodeSearch::new(&root);
    for q in ["", "   ", "\n\t", "the of a", "!!! ???", "x"] {
        let r = cs.search_blocking(q, 5).unwrap();
        assert!(r.matches.is_empty(), "{q:?}");
        assert_eq!(r.privacy_skipped, 0);
    }
    assert!(
        !cs.status().built,
        "an empty query must not trigger a build"
    );
    assert!(cs.search_blocking("main", 0).unwrap().matches.is_empty());
    assert!(matches!(
        cs.search_blocking(&"word ".repeat(10_000), 5),
        Err(SearchError::InvalidQuery(_))
    ));
}

#[test]
fn top_k_is_capped_at_fifty() {
    let (_d, root) = proj();
    for i in 0..70 {
        write(&root, &format!("src/f{i}.rs"), "fn common_word() {}\n");
    }
    let cs = CodeSearch::new(&root);
    assert_eq!(
        cs.search_blocking("common word", 1000)
            .unwrap()
            .matches
            .len(),
        50
    );
    assert_eq!(
        cs.search_blocking("common word", 7).unwrap().matches.len(),
        7
    );
}

#[test]
fn first_search_builds_the_root() {
    let (_d, root) = proj();
    write(&root, "src/a.rs", "fn needle() {}\n");
    let cs = CodeSearch::new(&root);
    assert_eq!(cs.status(), IndexStatus::default());
    assert_eq!(found(&search(&cs, "needle"), &root), ["src/a.rs"]);
    let st = cs.status();
    assert!(st.built && !st.building);
    assert_eq!((st.files, st.chunks), (1, 1));
}

// ── building and incremental updates ─────────────────────────────────────────

#[test]
fn building_twice_never_duplicates_documents() {
    let (_d, root) = proj();
    with_fillers(&root);
    write(&root, "src/a.rs", "fn needle() {}\n");
    let cs = CodeSearch::new(&root);
    let first = build(&cs);
    assert_eq!(
        (first.indexed, first.unchanged, first.updated, first.removed),
        (6, 0, 0, 0)
    );
    let before = search(&cs, "needle");
    let second = build(&cs);
    assert_eq!(
        (
            second.indexed,
            second.unchanged,
            second.updated,
            second.removed
        ),
        (0, 6, 0, 0)
    );
    assert_eq!(second.total_files, 6);
    assert_eq!(second.chunks, first.chunks);
    assert_eq!(search(&cs, "needle"), before);
    cs.with_index(Index::assert_consistent);
    // Also when the same files are named explicitly, twice.
    let again = cs
        .build_blocking(&[
            root.join("src"),
            PathBuf::from("src/a.rs"),
            PathBuf::from("src"),
        ])
        .unwrap();
    assert_eq!((again.indexed, again.updated, again.unchanged), (0, 0, 6));
    assert_eq!(cs.status().files, 6);
}

#[test]
fn build_stats_track_changes() {
    let (_d, root) = proj();
    let a = write(&root, "a.rs", "fn one() {}\n");
    write(&root, "b.rs", "fn two() {}\n");
    write(&root, "c.rs", "fn three() {}\n");
    touch(&a, 1);
    let cs = CodeSearch::new(&root);
    assert_eq!(build(&cs).indexed, 3);
    fs::write(&a, "fn one_changed() {}\n").unwrap();
    touch(&a, 2);
    fs::remove_file(root.join("b.rs")).unwrap();
    write(&root, "d.rs", "fn four() {}\n");
    let st = build(&cs);
    assert_eq!(
        (st.indexed, st.updated, st.unchanged, st.removed),
        (1, 1, 1, 1)
    );
    assert_eq!(st.total_files, 3);
    assert_eq!(indexed(&cs), ["a.rs", "c.rs", "d.rs"]);
    assert_eq!(found(&search(&cs, "one changed"), &root), ["a.rs"]);
    assert!(search(&cs, "two").matches.is_empty());
    cs.with_index(Index::assert_consistent);
}

#[test]
fn same_size_edit_with_new_mtime_is_picked_up() {
    let (_d, root) = proj();
    let a = write(&root, "a.rs", "fn alpha() {}\n");
    touch(&a, 1);
    let cs = CodeSearch::new(&root);
    build(&cs);
    fs::write(&a, "fn omega() {}\n").unwrap();
    touch(&a, 2);
    assert_eq!(build(&cs).updated, 1);
    assert!(search(&cs, "alpha").matches.is_empty());
    assert_eq!(found(&search(&cs, "omega"), &root), ["a.rs"]);
}

#[test]
fn partial_builds_touch_only_their_paths() {
    let (_d, root) = proj();
    write(&root, "src/a.rs", "fn one() {}\n");
    write(&root, "src/deep/b.rs", "fn two() {}\n");
    write(&root, "docs/c.md", "three\n");
    let cs = CodeSearch::new(&root);
    let st = cs.build_blocking(&[PathBuf::from("src")]).unwrap();
    assert_eq!((st.indexed, st.total_files), (2, 2));
    assert_eq!(indexed(&cs), ["src/a.rs", "src/deep/b.rs"]);
    assert!(cs.status().built);
    // Searching an already built index does not build the rest.
    assert!(search(&cs, "three").matches.is_empty());
    // A file path works too, and removal is scoped to what was asked for.
    fs::remove_file(root.join("src/a.rs")).unwrap();
    fs::remove_file(root.join("docs/c.md")).unwrap();
    let st = cs.build_blocking(&[root.join("src/deep")]).unwrap();
    assert_eq!((st.removed, st.unchanged), (0, 1));
    let st = cs.build_blocking(&[root.join("src/a.rs")]).unwrap();
    assert_eq!(st.removed, 1);
    assert_eq!(st.errors, 0, "{:?}", st.warnings);
    assert_eq!(indexed(&cs), ["src/deep/b.rs"]);
    let st = build(&cs);
    assert_eq!(st.total_files, 1);
    assert_eq!(st.removed, 0);
}

#[test]
fn bad_paths_are_reported_not_fatal() {
    let (_d, root) = proj();
    write(&root, "a.rs", "fn one() {}\n");
    let cs = CodeSearch::new(&root);
    let outside = std::env::temp_dir().join("definitely-not-in-the-workspace");
    let st = cs
        .build_blocking(&[PathBuf::from("missing/dir"), outside, PathBuf::from("a.rs")])
        .unwrap();
    assert_eq!(st.errors, 2);
    assert_eq!(st.warnings.len(), 2, "{:?}", st.warnings);
    assert!(st.warnings.iter().any(|w| w.contains("missing")));
    assert!(st.warnings.iter().any(|w| w.contains("outside")));
    assert_eq!(st.indexed, 1);
}

#[test]
fn missing_root_is_an_error() {
    let (_d, root) = proj();
    let cs = CodeSearch::new(root.join("nope"));
    assert!(matches!(cs.build_blocking(&[]), Err(SearchError::Io(_))));
    assert!(cs.search_blocking("anything", 5).is_err());
    assert!(!cs.status().built);
}

#[test]
fn refresh_and_remove_file_follow_the_editor() {
    let (_d, root) = proj();
    with_fillers(&root);
    let cs = CodeSearch::new(&root);
    build(&cs);
    let n = cs.status().files;
    // A new file appears.
    let a = write(&root, "src/new.rs", "fn onlyone() {}\n");
    cs.refresh_file(&a);
    assert_eq!(cs.status().files, n + 1);
    assert_eq!(found(&search(&cs, "onlyone"), &root), ["src/new.rs"]);
    // It is saved again with different content, even with the same stamp.
    let before = cs.stamp_of(&a).unwrap();
    fs::write(&a, "fn onlytwo() {}\n").unwrap();
    touch(&a, 5);
    cs.refresh_file(&a);
    assert_ne!(cs.stamp_of(&a).unwrap(), before);
    assert!(search(&cs, "onlyone").matches.is_empty());
    assert_eq!(found(&search(&cs, "onlytwo"), &root), ["src/new.rs"]);
    // Relative spellings and ".." resolve to the same file.
    cs.refresh_file(&root.join("src").join("..").join("src").join("new.rs"));
    assert_eq!(cs.status().files, n + 1);
    // Deleted on disk: refresh drops it.
    fs::remove_file(&a).unwrap();
    cs.refresh_file(&a);
    assert_eq!(cs.status().files, n);
    // remove_file on something that is not indexed is a no-op.
    cs.remove_file(&a);
    cs.remove_file(&root.join("never/existed.rs"));
    assert_eq!(cs.status().files, n);
    // remove_file on a directory removes everything below it.
    cs.remove_file(&root.join("src"));
    assert_eq!(cs.status().files, 0);
    assert_eq!(cs.status().chunks, 0);
    cs.with_index(Index::assert_consistent);
}

#[test]
fn refresh_ignores_files_the_build_would_not_index() {
    let (_d, root) = proj();
    write(&root, ".gitignore", "secret_output.rs\ngenerated/\n");
    write(&root, "keep.rs", "fn keep() {}\n");
    let cs = CodeSearch::new(&root);
    build(&cs);
    let ignored = write(&root, "secret_output.rs", "fn ignored() {}\n");
    let in_ignored_dir = write(&root, "generated/x.rs", "fn generated() {}\n");
    let skipped_dir = write(&root, "node_modules/x.js", "function modules() {}\n");
    let hidden = write(&root, ".hidden.rs", "fn hidden() {}\n");
    let binary = write(&root, "image.png", "fn png() {}\n");
    let outside = tempfile::tempdir().unwrap();
    let elsewhere = write(outside.path(), "x.rs", "fn elsewhere() {}\n");
    for p in [
        &ignored,
        &in_ignored_dir,
        &skipped_dir,
        &hidden,
        &binary,
        &elsewhere,
    ] {
        cs.refresh_file(p);
    }
    cs.refresh_file(&root);
    assert_eq!(indexed(&cs), ["keep.rs"]);
    // A file that becomes ignored leaves the index when refreshed.
    write(
        &root,
        ".gitignore",
        "secret_output.rs\ngenerated/\nkeep.rs\n",
    );
    cs.refresh_file(&root.join("keep.rs"));
    assert!(indexed(&cs).is_empty());
}

#[test]
fn reset_root_drops_the_index() {
    let (_d1, one) = proj();
    let (_d2, two) = proj();
    write(&one, "a.rs", "fn alphamarker() {}\n");
    write(&two, "b.rs", "fn betamarker() {}\n");
    let cs = CodeSearch::new(&one);
    assert_eq!(found(&search(&cs, "alphamarker"), &one), ["a.rs"]);
    cs.reset_root(&two);
    assert_eq!(cs.root(), two);
    assert_eq!(cs.status(), IndexStatus::default());
    assert!(search(&cs, "alphamarker").matches.is_empty());
    assert_eq!(found(&search(&cs, "betamarker"), &two), ["b.rs"]);
    // Files of the old root are not ours any more.
    cs.refresh_file(&one.join("a.rs"));
    assert_eq!(indexed(&cs), ["b.rs"]);
}

#[test]
fn a_build_for_an_old_root_is_abandoned() {
    let (_d1, one) = proj();
    let (_d2, two) = proj();
    for i in 0..5 {
        write(&one, &format!("f{i}.rs"), "fn sigmaword() {}\n");
    }
    write(&two, "b.rs", "fn omegaword() {}\n");
    let cs = CodeSearch::new(&one);
    let other = cs.clone();
    let two2 = two.clone();
    cs.set_after_commit(move || other.reset_root(&two2));
    assert!(matches!(
        cs.build_blocking(&[]),
        Err(SearchError::RootChanged)
    ));
    assert!(!cs.status().built);
    assert!(!cs.status().building);
    cs.set_after_commit(|| {});
    assert_eq!(found(&search(&cs, "omegaword"), &two), ["b.rs"]);
    assert!(search(&cs, "sigmaword").matches.is_empty());
}

// ── what gets indexed ────────────────────────────────────────────────────────

#[test]
fn walker_rules() {
    let (_d, root) = proj();
    // Kept.
    for f in [
        "kept.rs",
        "Makefile",
        "Dockerfile",
        "src/deep/er/nest.ts",
        "real_dir/inner.rs",
        ".github/workflows/ci.yml",
        ".eslintrc.json",
        "CMakeLists.txt",
        "shaders/a.wgsl",
        "App.vue",
        "NOTES.MD",
    ] {
        write(&root, f, "content here\n");
    }
    // .gitignore (root and nested), .ignore, no .git directory needed.
    write(&root, ".gitignore", "ignored.rs\nlogs/\n*.tmp.rs\n");
    write(&root, "ignored.rs", "x1\n");
    write(&root, "logs/a.rs", "x2\n");
    write(&root, "x.tmp.rs", "x3\n");
    write(&root, "sub/.gitignore", "*.gen.rs\n");
    write(&root, "sub/a.gen.rs", "x4\n");
    write(&root, "sub/a.rs", "content here\n");
    write(&root, ".ignore", "from_dot_ignore.rs\n");
    write(&root, "from_dot_ignore.rs", "x5\n");
    // Skipped directories.
    for d in [
        ".git",
        "target",
        "node_modules",
        "dist",
        "build",
        ".next",
        ".venv",
        "venv",
        "vendor",
        "__pycache__",
        ".cache",
        ".gradle",
        ".idea",
        ".vscode",
        ".phazeai",
    ] {
        write(&root, &format!("{d}/inner/file.rs"), "x6\n");
        write(&root, &format!("pkg/{d}/file.rs"), "x7\n");
    }
    // Hidden files, wrong extensions.
    write(&root, ".hidden.rs", "x8\n");
    write(&root, "image.png", "x9\n");
    write(&root, "Cargo.lock", "x10\n");
    write(&root, "noext", "x11\n");
    // Binary, non-UTF-8, empty.
    fs::write(root.join("bin.rs"), b"fn main() {\0\0}\n").unwrap();
    fs::write(root.join("latin1.txt"), b"caf\xe9 \xff\xfe\n").unwrap();
    write(&root, "empty.rs", "");
    write(&root, "blank.rs", "\n\n   \n");
    // Symlinks are never followed.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("kept.rs"), root.join("link.rs")).unwrap();
        std::os::unix::fs::symlink(root.join("real_dir"), root.join("link_dir")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop_dir")).unwrap();
    }
    let cs = CodeSearch::new(&root);
    let st = build(&cs);
    let mut want = vec![
        ".eslintrc.json",
        ".github/workflows/ci.yml",
        "App.vue",
        "CMakeLists.txt",
        "Dockerfile",
        "Makefile",
        "NOTES.MD",
        "blank.rs",
        "empty.rs",
        "kept.rs",
        "real_dir/inner.rs",
        "shaders/a.wgsl",
        "src/deep/er/nest.ts",
        "sub/a.rs",
    ];
    want.sort_unstable();
    assert_eq!(indexed(&cs), want);
    assert_eq!(st.skipped, 2, "bin.rs and latin1.txt");
    assert_eq!(st.errors, 0);
    assert_eq!(st.privacy_skipped, 0);
    assert_eq!(st.total_files, want.len());
    // Empty and blank files are indexed but have no chunks.
    assert!(st.chunks >= want.len() - 2);
    cs.with_index(Index::assert_consistent);
    assert!(search(&cs, "x1 x2 x3 x4 x5 x6 x7 x8 x9").matches.is_empty());
    // Nothing changes on a second pass: binary files are skipped again, not errors.
    let again = build(&cs);
    assert_eq!(
        (again.indexed, again.updated, again.removed, again.skipped),
        (0, 0, 0, 2)
    );
}

#[test]
fn oversize_files_are_skipped_and_dropped_when_they_grow() {
    let (_d, root) = proj();
    let a = write(&root, "a.txt", "small\n");
    write(&root, "big.txt", &"line of text\n".repeat(200));
    let limits = SearchLimits {
        max_file_bytes: 1000,
        ..SearchLimits::default()
    };
    let cs = CodeSearch::with_limits(&root, limits);
    let st = build(&cs);
    assert_eq!((st.indexed, st.skipped), (1, 1));
    assert!(!st.truncated);
    // A file that outgrows the limit leaves the index.
    fs::write(&a, "grown\n".repeat(300)).unwrap();
    touch(&a, 3);
    let st = build(&cs);
    // big.txt is still too large; a.txt just became too large and left the index.
    assert_eq!((st.removed, st.skipped, st.total_files), (1, 2, 0));
    assert!(search(&cs, "small").matches.is_empty());
}

#[test]
fn max_files_truncates_in_path_order_and_says_so() {
    let (_d, root) = proj();
    for i in 0..6 {
        write(&root, &format!("src/f{i}.rs"), "fn needle() {}\n");
    }
    let limits = SearchLimits {
        max_files: 4,
        ..SearchLimits::default()
    };
    let cs = CodeSearch::with_limits(&root, limits);
    let st = build(&cs);
    assert!(st.truncated);
    assert_eq!((st.indexed, st.total_files), (4, 4));
    assert_eq!(
        indexed(&cs),
        ["src/f0.rs", "src/f1.rs", "src/f2.rs", "src/f3.rs"]
    );
    let st = cs.status();
    assert!(st.truncated);
    let r = search(&cs, "needle");
    assert!(r.index_truncated);
    assert_eq!(r.matches.len(), 4);
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["index_truncated"], true);
    // Making room lets the next build catch up.
    fs::remove_file(root.join("src/f0.rs")).unwrap();
    let st = build(&cs);
    assert_eq!((st.removed, st.indexed, st.total_files), (1, 1, 4));
    assert!(st.truncated, "f5 still does not fit");
    fs::remove_file(root.join("src/f1.rs")).unwrap();
    fs::remove_file(root.join("src/f2.rs")).unwrap();
    let st = build(&cs);
    assert!(!st.truncated);
    assert!(!cs.status().truncated);
    assert_eq!(st.total_files, 3);
}

#[test]
fn max_total_bytes_truncates_and_evicts_changed_files_that_no_longer_fit() {
    let (_d, root) = proj();
    let body = "fn needle() {} // padding padding padding\n";
    let a = write(&root, "a.rs", body);
    write(&root, "b.rs", body);
    write(&root, "c.rs", body);
    let limits = SearchLimits {
        max_total_bytes: (body.len() * 2 + 10) as u64,
        ..SearchLimits::default()
    };
    let cs = CodeSearch::with_limits(&root, limits);
    let st = build(&cs);
    assert!(st.truncated);
    assert_eq!(indexed(&cs), ["a.rs", "b.rs"]);
    // a.rs doubles in size: it no longer fits next to b.rs, so it must go
    // (its old text is stale) rather than stay behind.
    fs::write(&a, body.repeat(2)).unwrap();
    touch(&a, 9);
    let st = build(&cs);
    assert!(st.truncated);
    assert!(
        !indexed(&cs).contains(&"a.rs".to_string()),
        "{:?}",
        indexed(&cs)
    );
    assert_eq!(indexed(&cs), ["b.rs", "c.rs"]);
}

#[test]
fn refresh_file_respects_limits() {
    let (_d, root) = proj();
    write(&root, "a.rs", "fn one() {}\n");
    let limits = SearchLimits {
        max_files: 1,
        ..SearchLimits::default()
    };
    let cs = CodeSearch::with_limits(&root, limits);
    build(&cs);
    let b = write(&root, "b.rs", "fn two() {}\n");
    cs.refresh_file(&b);
    assert_eq!(indexed(&cs), ["a.rs"]);
    assert!(cs.status().truncated);
}

// ── privacy ──────────────────────────────────────────────────────────────────

#[test]
fn builtin_secrets_are_never_indexed_or_returned() {
    let (_d, root) = proj();
    write(&root, "src/main.rs", "fn main() { openaccess(); }\n");
    write(&root, "secrets.yaml", "token: hunterthreesecret\n");
    write(
        &root,
        "deploy/credentials.json",
        "{\"key\": \"hunterthreesecret\"}\n",
    );
    write(
        &root,
        "src/.env.local.rs",
        "not hidden-allowed anyway hunterthreesecret\n",
    );
    let cs = CodeSearch::new(&root);
    let st = build(&cs);
    assert_eq!(indexed(&cs), ["src/main.rs"]);
    assert_eq!(st.privacy_skipped, 2);
    let r = search(&cs, "hunterthreesecret");
    assert!(r.matches.is_empty());
    assert_eq!(
        r.privacy_skipped, 0,
        "never indexed, so nothing to drop at query time"
    );
    assert_eq!(found(&search(&cs, "openaccess"), &root), ["src/main.rs"]);
}

#[test]
fn project_policy_denies_directories_before_they_are_read() {
    let (_d, root) = proj();
    write(
        &root,
        ".phazeai/privacy.toml",
        "deny = [\"gen/\", \"docs/internal/*.md\"]\n",
    );
    write(&root, "src/main.rs", "fn main() { visibleword(); }\n");
    write(&root, "gen/out.rs", "fn zebrafish() {}\n");
    write(&root, "docs/internal/plan.md", "zebrafish plan\n");
    write(&root, "docs/public.md", "zebrafish is public\n");
    let _a = activate(&root);
    let cs = CodeSearch::new(&root);
    let st = build(&cs);
    assert_eq!(indexed(&cs), ["docs/public.md", "src/main.rs"]);
    assert_eq!(
        st.privacy_skipped, 2,
        "the gen/ directory and the internal plan"
    );
    assert_eq!(found(&search(&cs, "zebrafish"), &root), ["docs/public.md"]);
}

#[test]
fn a_policy_change_after_indexing_applies_at_query_time_and_at_the_next_sync() {
    let (_d, root) = proj();
    write(&root, "src/main.rs", "fn main() { visibleword(); }\n");
    write(
        &root,
        "private/notes.md",
        "# notes\nhighly confidential zebrafish plan\n",
    );
    write(&root, "pub/readme.md", "the zebrafish is a small fish\n");
    let _a = activate(&root);
    let cs = CodeSearch::new(&root);
    let st = build(&cs);
    assert_eq!((st.total_files, st.privacy_skipped), (3, 0));
    assert_eq!(found(&search(&cs, "zebrafish"), &root).len(), 2);

    // The policy appears: the indexed file is dropped from results and counted.
    write(&root, ".phazeai/privacy.toml", "deny = [\"private/**\"]\n");
    let r = search(&cs, "zebrafish");
    assert_eq!(found(&r, &root), ["pub/readme.md"]);
    assert_eq!(r.privacy_skipped, 1);
    assert_eq!(serde_json::to_value(&r).unwrap()["privacy_skipped"], 1);
    assert_eq!(
        cs.status().files,
        3,
        "removed from the index only by the next sync"
    );

    // The next sync removes it for good.
    let st = build(&cs);
    assert_eq!((st.removed, st.total_files), (1, 2));
    assert!(st.privacy_skipped >= 1);
    let r = search(&cs, "zebrafish");
    assert_eq!(r.privacy_skipped, 0);
    assert_eq!(found(&r, &root), ["pub/readme.md"]);

    // Lifting the policy brings it back.
    fs::remove_file(root.join(".phazeai/privacy.toml")).unwrap();
    let st = build(&cs);
    assert_eq!((st.indexed, st.total_files), (1, 3));
    assert_eq!(found(&search(&cs, "zebrafish"), &root).len(), 2);
}

#[test]
fn denied_files_do_not_use_up_result_slots() {
    let (_d, root) = proj();
    for i in 0..6 {
        write(
            &root,
            &format!("private/p{i}.md"),
            &"needle needle needle\n".repeat(5),
        );
    }
    write(&root, "src/a.rs", "fn needle() {}\n");
    write(&root, "src/b.rs", "fn other() { needle(); }\n");
    let _a = activate(&root);
    let cs = CodeSearch::new(&root);
    build(&cs);
    write(&root, ".phazeai/privacy.toml", "deny = [\"private/**\"]\n");
    let r = cs.search_blocking("needle", 2).unwrap();
    assert_eq!(r.matches.len(), 2);
    assert!(found(&r, &root).iter().all(|f| f.starts_with("src/")));
    assert_eq!(r.privacy_skipped, 6);
}

#[tokio::test]
async fn async_search_notes_surviving_files_for_the_audit_scope() {
    let (_d, root) = proj();
    write(&root, "src/main.rs", "fn needle() {}\n");
    let cs = CodeSearch::new(&root);
    let noted = privacy::audit::scope(Vec::new(), async {
        let r = cs.search("needle", 5).await.unwrap();
        assert_eq!(r.matches.len(), 1);
        privacy::audit::take_files()
    })
    .await;
    assert!(noted.iter().any(|f| f.ends_with("main.rs")), "{noted:?}");
}

// ── staleness ────────────────────────────────────────────────────────────────

#[test]
fn edits_after_indexing_are_reflected_in_the_hit() {
    let (_d, root) = proj();
    with_fillers(&root);
    let a = write(&root, "src/a.rs", "fn zebrafish() {}\nfn other() {}\n");
    touch(&a, 1);
    let cs = CodeSearch::new(&root);
    build(&cs);
    let r = search(&cs, "zebrafish");
    assert_eq!((r.matches[0].start_line, r.matches[0].end_line), (1, 2));
    // Ten lines are added on top; the index has not been told.
    let edited = format!(
        "{}fn zebrafish() {{}}\nfn other() {{}}\n",
        "// pad\n".repeat(10)
    );
    fs::write(&a, edited).unwrap();
    touch(&a, 2);
    let r = search(&cs, "zebrafish");
    let h = &r.matches[0];
    assert_eq!(rel(&root, &h.file), "src/a.rs");
    assert!(h.start_line <= 11 && h.end_line >= 11, "{h:?}");
    assert!(h.snippet.contains("fn zebrafish"));
    // The index itself was refreshed on the way.
    assert_eq!(
        cs.stamp_of(&a).unwrap().size,
        fs::metadata(&a).unwrap().len()
    );
    // The match moved out of the file entirely: it is gone from the results.
    fs::write(&a, "fn unrelated() {}\n").unwrap();
    touch(&a, 3);
    assert!(search(&cs, "zebrafish").matches.is_empty());
}

#[test]
fn deleted_files_are_dropped_without_error() {
    let (_d, root) = proj();
    with_fillers(&root);
    let a = write(&root, "src/a.rs", "fn zebrafish() {}\n");
    write(&root, "src/b.rs", "fn zebrafish_two() {}\n");
    let cs = CodeSearch::new(&root);
    build(&cs);
    let before = cs.status().files;
    fs::remove_file(&a).unwrap();
    let r = cs.search_blocking("zebrafish", 5).unwrap();
    assert_eq!(found(&r, &root), ["src/b.rs"]);
    assert_eq!(cs.status().files, before - 1);
    cs.with_index(Index::assert_consistent);
    // Whole directory gone.
    fs::remove_dir_all(root.join("src")).unwrap();
    assert!(cs
        .search_blocking("zebrafish", 5)
        .unwrap()
        .matches
        .is_empty());
}

#[test]
fn a_file_turned_binary_or_replaced_by_a_directory_leaves_the_results() {
    let (_d, root) = proj();
    with_fillers(&root);
    let a = write(&root, "src/a.rs", "fn zebrafish() {}\n");
    let b = write(&root, "src/b.rs", "fn zebrafish_two() {}\n");
    let cs = CodeSearch::new(&root);
    build(&cs);
    fs::write(&a, b"\0\0\0 zebrafish").unwrap();
    touch(&a, 4);
    fs::remove_file(&b).unwrap();
    fs::create_dir(&b).unwrap();
    let r = search(&cs, "zebrafish");
    assert!(r.matches.is_empty(), "{:?}", found(&r, &root));
    assert!(!indexed(&cs).contains(&"src/a.rs".to_string()));
    assert!(!indexed(&cs).contains(&"src/b.rs".to_string()));
}

// ── incremental == fresh ─────────────────────────────────────────────────────

/// A tiny deterministic generator (no rand crate).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const POOL: &[&str] = &[
    "a.rs",
    "src/b.rs",
    "src/lsp/manager.rs",
    "src/lsp/bridge.rs",
    "docs/notes.md",
    "docs/deep/guide.md",
    "web/app.ts",
    "web/util.ts",
    "Makefile",
    "x/y/z/config.toml",
    "src/c.py",
    "src/d.go",
];
const DIRS: &[&str] = &["src", "src/lsp", "docs", "web", "x"];
const WORDS: &[&str] = &[
    "parse",
    "json",
    "config",
    "manager",
    "lspBridge",
    "fooBar",
    "bazQux",
    "render",
    "window",
    "handle_request",
    "HttpServer",
    "utf8",
    "alpha",
    "beta",
    "gamma",
    "delta",
    "token",
    "stream",
    "buffer",
    "cache",
    "parseJsonConfig",
    "MAX_RETRY",
    "the",
    "of",
];
const QUERIES: &[&str] = &[
    "parse json config",
    "lsp manager",
    "handle request",
    "http server",
    "alpha beta",
    "buffer",
    "render window cache",
    "foo bar",
    "utf8",
    "max retry token",
    "guide notes",
];

fn random_text(rng: &mut Lcg) -> String {
    let mut text = String::new();
    for _ in 0..rng.below(130) {
        match rng.below(8) {
            0 => text.push('\n'),
            _ => {
                let n = 1 + rng.below(5);
                let line: Vec<&str> = (0..n).map(|_| WORDS[rng.below(WORDS.len())]).collect();
                text.push_str(&format!("{};\n", line.join(" ")));
            }
        }
    }
    text
}

fn stamps(cs: &CodeSearch) -> BTreeMap<String, Stamp> {
    let root = cs.root();
    cs.with_index(|i| {
        i.entries()
            .map(|(k, m)| {
                (
                    k.strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    m.stamp,
                )
            })
            .collect()
    })
}

fn assert_same_as_fresh(cs: &CodeSearch, root: &Path, step: usize) {
    let fresh = CodeSearch::new(root);
    for q in QUERIES {
        assert_eq!(
            cs.search_blocking(q, 50).unwrap(),
            fresh.search_blocking(q, 50).unwrap(),
            "step {step}: query {q:?} differs from a fresh build"
        );
    }
    assert_eq!(
        cs.status().files,
        fresh.status().files,
        "step {step}: file count"
    );
    assert_eq!(
        cs.status().chunks,
        fresh.status().chunks,
        "step {step}: chunk count"
    );
    assert_eq!(stamps(cs), stamps(&fresh), "step {step}: indexed files");
    cs.with_index(Index::assert_consistent);
}

#[test]
fn any_sequence_of_updates_ranks_like_a_fresh_build() {
    for seed in [1u64, 7, 42] {
        let (_d, root) = proj();
        let mut rng = Lcg(seed);
        let cs = CodeSearch::new(&root);
        build(&cs);
        let mut clock = 0u64;
        for step in 0..160 {
            match rng.below(11) {
                // Create or modify a file, then tell the index in some way.
                0..=3 => {
                    let rel = POOL[rng.below(POOL.len())];
                    let path = write(&root, rel, &random_text(&mut rng));
                    clock += 1;
                    touch(&path, clock);
                    match rng.below(4) {
                        0 => cs.refresh_file(&path),
                        1 => {
                            build(&cs);
                        }
                        2 => {
                            cs.build_blocking(&[path.parent().unwrap().to_path_buf()])
                                .unwrap();
                        }
                        _ => {
                            cs.build_blocking(std::slice::from_ref(&path)).unwrap();
                        }
                    }
                }
                // Delete a file.
                4 | 5 => {
                    let path = root.join(POOL[rng.below(POOL.len())]);
                    let existed = fs::remove_file(&path).is_ok();
                    match rng.below(4) {
                        0 => cs.remove_file(&path),
                        1 => cs.refresh_file(&path),
                        2 => {
                            cs.build_blocking(&[path]).unwrap();
                        }
                        _ => {
                            let _ = existed;
                            build(&cs);
                        }
                    }
                }
                // Delete a directory.
                6 => {
                    let dir = root.join(DIRS[rng.below(DIRS.len())]);
                    let _ = fs::remove_dir_all(&dir);
                    if rng.below(2) == 0 {
                        cs.remove_file(&dir);
                    } else {
                        build(&cs);
                    }
                }
                // Redundant operations.
                7 => cs.refresh_file(&root.join(POOL[rng.below(POOL.len())])),
                8 => {
                    build(&cs);
                    build(&cs);
                }
                9 => {
                    // remove_file of a file that still exists must be repaired by a refresh.
                    let path = root.join(POOL[rng.below(POOL.len())]);
                    cs.remove_file(&path);
                    cs.refresh_file(&path);
                }
                _ => assert_same_as_fresh(&cs, &root, step),
            }
        }
        assert_same_as_fresh(&cs, &root, 160);
    }
}

#[test]
fn compaction_keeps_results_and_bounds_the_chunk_table() {
    let (_d, root) = proj();
    let mut rng = Lcg(99);
    for rel in POOL {
        write(&root, rel, &random_text(&mut rng));
    }
    let cs = CodeSearch::new(&root);
    build(&cs);
    let mut clock = 0;
    let mut peak = 0;
    for _ in 0..200 {
        let path = write(&root, POOL[rng.below(POOL.len())], &random_text(&mut rng));
        clock += 1;
        touch(&path, clock);
        cs.refresh_file(&path);
        peak = peak.max(cs.with_index(|i| i.table_len()));
    }
    let (live, table, dead) = cs.with_index(|i| (i.chunk_count(), i.table_len(), i.dead_chunks()));
    assert_eq!(table, live + dead);
    // Tombstones never exceed the compaction trigger by more than one batch.
    assert!(dead <= live / 2 + 16 + 40, "dead {dead}, live {live}");
    assert!(peak < 6 * (live + 40), "peak {peak}, live {live}");
    assert_same_as_fresh(&cs, &root, 0);
}

// ── index internals ──────────────────────────────────────────────────────────

fn parsed(path: &str, text: &str) -> ParsedFile {
    ParsedFile {
        path: PathBuf::from(path),
        stamp: Stamp {
            mtime: UNIX_EPOCH,
            size: text.len() as u64,
        },
        chunks: scan::parse_text(text, &scan::path_terms(Path::new(path), Path::new("/w"))),
    }
}

fn ranking(index: &Index, query: &str) -> Vec<(String, u32, u32, f64)> {
    let terms = tokenize::query_terms(query);
    index
        .rank(&terms, 100)
        .candidates
        .iter()
        .map(|c| (c.path.display().to_string(), c.start, c.end, c.score))
        .collect()
}

fn corpus() -> Vec<(String, String)> {
    let mut rng = Lcg(5);
    (0..30)
        .map(|i| (format!("/w/dir{}/file{i}.rs", i % 4), random_text(&mut rng)))
        .collect()
}

#[test]
fn removing_files_gives_the_same_index_as_never_adding_them() {
    // 3 removals take the per-range drain path, 12 the single retain pass.
    for removed in [1usize, 3, 8, 9, 12, 30] {
        let docs = corpus();
        let mut full = Index::new();
        full.apply(
            u64::MAX,
            &[],
            docs.iter()
                .map(|(p, t)| Parsed::File(parsed(p, t)))
                .collect(),
        );
        let gone: Vec<PathBuf> = docs
            .iter()
            .take(removed)
            .map(|(p, _)| PathBuf::from(p))
            .collect();
        let applied: Applied = full.apply(u64::MAX, &gone, Vec::new());
        assert_eq!(applied.removed, removed);
        full.assert_consistent();

        let mut fresh = Index::new();
        fresh.apply(
            u64::MAX,
            &[],
            docs.iter()
                .skip(removed)
                .map(|(p, t)| Parsed::File(parsed(p, t)))
                .collect(),
        );
        for q in QUERIES {
            assert_eq!(
                ranking(&full, q),
                ranking(&fresh, q),
                "removed {removed}: {q}"
            );
        }
        assert_eq!(full.file_count(), fresh.file_count());
        assert_eq!(full.chunk_count(), fresh.chunk_count());
    }
}

#[test]
fn replacing_a_file_replaces_all_of_its_chunks() {
    let docs = corpus();
    let mut idx = Index::new();
    idx.apply(
        u64::MAX,
        &[],
        docs.iter()
            .map(|(p, t)| Parsed::File(parsed(p, t)))
            .collect(),
    );
    let chunks_before = idx.chunk_count();
    let (path, text) = &docs[3];
    let applied = idx.apply(u64::MAX, &[], vec![Parsed::File(parsed(path, text))]);
    assert_eq!((applied.updated, applied.indexed), (1, 0));
    assert_eq!(idx.chunk_count(), chunks_before);
    assert_eq!(idx.file_count(), docs.len());
    idx.assert_consistent();
}

#[test]
fn a_batch_never_overwrites_a_newer_entry() {
    let mut idx = Index::new();
    idx.apply(
        u64::MAX,
        &[],
        vec![Parsed::File(parsed("/w/a.rs", "fn old_version() {}\n"))],
    );
    let plan_seq = idx.seq();
    // Someone else (an editor save) writes the file after the batch was planned.
    idx.apply(
        u64::MAX,
        &[],
        vec![Parsed::File(parsed("/w/a.rs", "fn newer_version() {}\n"))],
    );
    let applied = idx.apply(
        plan_seq,
        &[PathBuf::from("/w/a.rs")],
        vec![Parsed::File(parsed("/w/a.rs", "fn stale_version() {}\n"))],
    );
    assert_eq!(
        (applied.removed, applied.updated, applied.indexed),
        (0, 0, 0)
    );
    assert_eq!(ranking(&idx, "newer").len(), 1);
    assert!(ranking(&idx, "stale").is_empty());
    assert!(ranking(&idx, "old").is_empty());
}

// ── concurrency ──────────────────────────────────────────────────────────────

/// Pause the build after its first committed batch until `resume` is sent.
struct Pause {
    reached: mpsc::Receiver<()>,
    resume: mpsc::Sender<()>,
}

fn pause_after_first_commit(cs: &CodeSearch) -> Pause {
    let (reached_tx, reached) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel::<()>();
    let resume_rx = Mutex::new(resume_rx);
    let first = AtomicBool::new(true);
    cs.set_after_commit(move || {
        if first.swap(false, Ordering::SeqCst) {
            reached_tx.send(()).unwrap();
            resume_rx.lock().unwrap().recv().unwrap();
        }
    });
    Pause { reached, resume }
}

fn within<T: Send + 'static>(secs: u64, what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs))
        .unwrap_or_else(|_| panic!("{what} did not finish in {secs}s"))
}

#[test]
fn searches_run_against_the_last_commit_while_a_build_is_in_progress() {
    let (_d, root) = proj();
    for i in 0..20 {
        write(&root, &format!("old/f{i}.rs"), "fn alphaword() {}\n");
    }
    let cs = CodeSearch::new(&root);
    build(&cs);
    for i in 0..20 {
        write(
            &root,
            &format!("new/f{i}.rs"),
            "fn alphaword() { betaword(); }\n",
        );
    }
    let pause = pause_after_first_commit(&cs);
    let builder = {
        let cs = cs.clone();
        std::thread::spawn(move || cs.build_blocking(&[]))
    };
    pause
        .reached
        .recv_timeout(Duration::from_secs(30))
        .expect("build never committed");

    // The build is mid-flight with no lock held.
    let status = within(10, "status()", {
        let cs = cs.clone();
        move || cs.status()
    });
    assert!(status.building && status.built);
    let during = within(20, "search during a build", {
        let cs = cs.clone();
        move || cs.search_blocking("alphaword", 50)
    })
    .unwrap();
    // Every old file is there; the new ones are there as far as they are committed.
    let n = during.matches.len();
    assert!((20..=40).contains(&n), "{n} hits");
    assert!(during.matches.iter().all(|h| Path::new(&h.file).exists()));
    // A concurrent explicit refresh also goes through.
    within(10, "refresh_file during a build", {
        let cs = cs.clone();
        let p = root.join("old/f0.rs");
        move || cs.refresh_file(&p)
    });

    pause.resume.send(()).unwrap();
    let stats = builder.join().unwrap().unwrap();
    assert_eq!(stats.indexed, 20);
    assert!(!cs.status().building);
    assert_eq!(search(&cs, "betaword").matches.len(), 20);
    cs.with_index(Index::assert_consistent);
}

#[test]
fn concurrent_builds_are_single_flight_and_never_duplicate() {
    let (_d, root) = proj();
    for i in 0..30 {
        write(
            &root,
            &format!("src/f{i}.rs"),
            "fn needle() {}\nfn other() {}\n",
        );
    }
    let cs = CodeSearch::new(&root);
    let pause = pause_after_first_commit(&cs);
    let first = {
        let cs = cs.clone();
        std::thread::spawn(move || cs.build_blocking(&[]))
    };
    pause
        .reached
        .recv_timeout(Duration::from_secs(30))
        .expect("build never committed");
    let second_done = Arc::new(AtomicBool::new(false));
    let second = {
        let cs = cs.clone();
        let done = Arc::clone(&second_done);
        std::thread::spawn(move || {
            let r = cs.build_blocking(&[]);
            done.store(true, Ordering::SeqCst);
            r
        })
    };
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !second_done.load(Ordering::SeqCst),
        "the second build must wait for the first"
    );
    pause.resume.send(()).unwrap();
    let a = first.join().unwrap().unwrap();
    let b = second.join().unwrap().unwrap();
    assert_eq!(a.indexed, 30);
    assert_eq!(
        (b.indexed, b.updated, b.unchanged, b.total_files),
        (0, 0, 30, 30)
    );
    assert_eq!(cs.status().files, 30);
    assert_eq!(cs.status().chunks, 30);
    cs.with_index(Index::assert_consistent);
}

#[test]
fn many_threads_searching_an_unbuilt_index_trigger_one_build() {
    let (_d, root) = proj();
    for i in 0..40 {
        write(&root, &format!("src/f{i}.rs"), "fn needle() {}\n");
    }
    let cs = CodeSearch::new(&root);
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let cs = cs.clone();
            std::thread::spawn(move || cs.search_blocking("needle", 50).unwrap().matches.len())
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().unwrap(), 40);
    }
    assert_eq!(cs.status().files, 40);
    cs.with_index(Index::assert_consistent);
}

#[test]
fn searching_and_updating_from_many_threads_stays_consistent() {
    let (_d, root) = proj();
    for i in 0..20 {
        write(&root, &format!("src/f{i}.rs"), "fn needle() {}\n");
    }
    let cs = CodeSearch::new(&root);
    build(&cs);
    let stop = Arc::new(AtomicBool::new(false));
    let searchers: Vec<_> = (0..3)
        .map(|_| {
            let (cs, stop) = (cs.clone(), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut n = 0;
                while !stop.load(Ordering::Relaxed) {
                    cs.search_blocking("needle", 50).unwrap();
                    n += 1;
                }
                n
            })
        })
        .collect();
    for round in 0..30u64 {
        let p = write(
            &root,
            &format!("src/f{}.rs", round % 20),
            &format!("fn needle() {{ v{round}(); }}\n"),
        );
        touch(&p, round + 1);
        cs.refresh_file(&p);
        if round % 7 == 0 {
            build(&cs);
        }
    }
    stop.store(true, Ordering::Relaxed);
    for s in searchers {
        assert!(s.join().unwrap() > 0);
    }
    cs.with_index(Index::assert_consistent);
    assert_same_as_fresh(&cs, &root, 0);
}

// ── tools ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn code_search_tool_json_shape() {
    let (_d, root) = proj();
    with_fillers(&root);
    write(
        &root,
        "src/settings.rs",
        "pub fn parseJsonConfig(text: &str) -> Config {\n    todo()\n}\n",
    );
    let cs = CodeSearch::new(&root);
    let tool = CodeSearchTool::new(cs.clone());
    assert_eq!(tool.name(), "code_search");
    let schema = tool.parameters_schema();
    assert_eq!(schema["required"], serde_json::json!(["query"]));
    assert!(schema["properties"]["top_k"].is_object());
    for banned in ["semantic", "embedding", "TF-IDF", "tf-idf"] {
        assert!(!tool.description().contains(banned), "{banned}");
    }

    let out = tool
        .execute(serde_json::json!({"query": "json config"}))
        .await
        .unwrap();
    let hit = &out["matches"][0];
    assert!(hit["file"].as_str().unwrap().ends_with("settings.rs"));
    assert!(Path::new(hit["file"].as_str().unwrap()).is_absolute());
    assert!(hit["score"].as_f64().unwrap() > 0.0);
    assert_eq!(hit["start_line"], 1);
    assert_eq!(hit["end_line"], 3);
    assert!(hit["snippet"].as_str().unwrap().contains("parseJsonConfig"));
    assert!(out.get("privacy_skipped").is_none());
    assert!(out.get("index_truncated").is_none());
    assert!(out.get("note").is_none());

    // top_k is honoured and capped at 20.
    for i in 0..30 {
        write(&root, &format!("many/m{i}.rs"), "fn commonword() {}\n");
    }
    build(&cs);
    let some = tool
        .execute(serde_json::json!({"query": "commonword", "top_k": 3}))
        .await
        .unwrap();
    assert_eq!(some["matches"].as_array().unwrap().len(), 3);
    let capped = tool
        .execute(serde_json::json!({"query": "commonword", "top_k": 500}))
        .await
        .unwrap();
    assert_eq!(capped["matches"].as_array().unwrap().len(), 20);
    let default = tool
        .execute(serde_json::json!({"query": "commonword"}))
        .await
        .unwrap();
    assert_eq!(default["matches"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn code_search_tool_errors_and_notes() {
    let (_d, root) = proj();
    write(&root, "a.rs", "fn main() {}\n");
    let tool = CodeSearchTool::new(CodeSearch::new(&root));
    let err = tool.execute(serde_json::json!({})).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("Missing required parameter: query"),
        "{err}"
    );
    let err = tool
        .execute(serde_json::json!({"query": 42}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("query"));
    // Nothing searchable: an empty result with an explanation, not an error.
    let out = tool
        .execute(serde_json::json!({"query": "the of"}))
        .await
        .unwrap();
    assert_eq!(out["matches"], serde_json::json!([]));
    assert!(out["note"].as_str().unwrap().contains("no searchable"));
    // No match is a plain empty result.
    let out = tool
        .execute(serde_json::json!({"query": "zzzzunknown"}))
        .await
        .unwrap();
    assert_eq!(out["matches"], serde_json::json!([]));
    assert!(out.get("note").is_none());
    // Index problems surface as tool errors, not panics.
    let broken = CodeSearchTool::new(CodeSearch::new(root.join("missing")));
    assert!(broken
        .execute(serde_json::json!({"query": "main"}))
        .await
        .is_err());
}

#[tokio::test]
async fn code_search_tool_reports_truncation_and_privacy() {
    let (_d, root) = proj();
    for i in 0..4 {
        write(&root, &format!("f{i}.rs"), "fn needle() {}\n");
    }
    let limits = SearchLimits {
        max_files: 2,
        ..SearchLimits::default()
    };
    let tool = CodeSearchTool::new(CodeSearch::with_limits(&root, limits));
    let out = tool
        .execute(serde_json::json!({"query": "needle"}))
        .await
        .unwrap();
    assert_eq!(out["index_truncated"], true);
    assert!(out["note"].as_str().unwrap().contains("incomplete"));
    assert_eq!(out["matches"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn build_index_tool_json_shape() {
    let (_d, root) = proj();
    write(&root, "src/a.rs", "fn one() {}\n");
    write(&root, "src/b.rs", "fn two() {}\n");
    write(&root, "docs/c.md", "three\n");
    fs::write(root.join("src/bin.rs"), b"\0\0").unwrap();
    let tool = BuildIndexTool::new(CodeSearch::new(&root));
    assert_eq!(tool.name(), "build_search_index");
    assert_eq!(
        tool.parameters_schema()["properties"]["paths"]["type"],
        "array"
    );
    assert!(tool.parameters_schema().get("required").is_none());

    let out = tool
        .execute(serde_json::json!({"paths": ["src"]}))
        .await
        .unwrap();
    assert_eq!(out["indexed"], 2);
    assert_eq!(out["skipped"], 1);
    assert_eq!(out["errors"], 0);
    assert_eq!(out["total_files"], 2);
    for key in [
        "updated",
        "unchanged",
        "removed",
        "privacy_skipped",
        "chunks",
        "truncated",
        "elapsed_ms",
    ] {
        assert!(out.get(key).is_some(), "{key}");
    }
    assert_eq!(out["truncated"], false);
    assert!(out.get("warnings").is_none());

    // No paths: the whole workspace, incrementally.
    let out = tool.execute(serde_json::json!({})).await.unwrap();
    assert_eq!(
        (out["indexed"].as_u64(), out["unchanged"].as_u64()),
        (Some(1), Some(2))
    );
    assert_eq!(out["total_files"], 3);

    // Bad paths come back as warnings.
    let out = tool
        .execute(serde_json::json!({"paths": ["nope", "/definitely/not/here"]}))
        .await
        .unwrap();
    assert_eq!(out["errors"], 2);
    assert_eq!(out["warnings"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn build_index_tool_says_when_a_limit_was_hit() {
    let (_d, root) = proj();
    for i in 0..3 {
        write(&root, &format!("f{i}.rs"), "fn x() {}\n");
    }
    let limits = SearchLimits {
        max_files: 1,
        ..SearchLimits::default()
    };
    let tool = BuildIndexTool::new(CodeSearch::with_limits(&root, limits));
    let out = tool.execute(serde_json::json!({})).await.unwrap();
    assert_eq!(out["truncated"], true);
    assert!(out["message"].as_str().unwrap().contains("limit"));
    assert_eq!(out["total_files"], 1);
}

#[test]
fn search_results_serialize_as_the_old_sidecar_did() {
    let hit = SearchHit {
        file: "/w/a.rs".into(),
        start_line: 3,
        end_line: 5,
        score: 1.2346,
        snippet: "x".into(),
    };
    let plain = SearchResults {
        matches: vec![hit.clone()],
        ..SearchResults::default()
    };
    let v = serde_json::to_value(&plain).unwrap();
    assert_eq!(
        v.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["matches"]
    );
    assert_eq!(v["matches"][0]["file"], "/w/a.rs");
    assert_eq!(v["matches"][0]["score"], 1.2346);
    let denied = SearchResults {
        matches: vec![hit],
        privacy_skipped: 2,
        index_truncated: false,
    };
    assert_eq!(serde_json::to_value(&denied).unwrap()["privacy_skipped"], 2);
}

#[test]
fn defaults_match_the_contract() {
    let l = SearchLimits::default();
    assert_eq!(
        (l.max_files, l.max_file_bytes, l.max_total_bytes),
        (50_000, 1 << 20, 256 << 20)
    );
}
