//! Black-box specification tests for the in-process code search
//! (`phazeai_core::search`), the replacement of the Python search sidecar.
//!
//! Everything here goes through the public API only: `CodeSearch`, its result
//! types, the two agent tools and the real `privacy` module. The expectations
//! come from the contract (names, signatures, JSON shapes), the module docs
//! and the old sidecar's behaviour, not from the engine's internals.
//!
//! Layout:
//!  1. helpers and universal result invariants
//!  2. a realistic multi-language repo, identifier splitting, ranking
//!  3. awkward files: long lines, big files, limits, CRLF, BOM, unicode, ...
//!  4. what the walker indexes: .gitignore, skip dirs, symlinks, odd paths
//!  5. files that change under the index, root deletion, reset_root
//!  6. idempotence, queries, top_k
//!  7. concurrency
//!  8. the two tools
//!  9. privacy
//! 10. seeded randomized differential tests against a fresh index and an oracle
//! 11. more adversarial cases (identifier styles, snippets, odd roots, churn, size caps, fuzz)
//!
//! `CODE_SEARCH_SPEC_EXTRA_SEEDS=N` makes `differential_soak` replay N more seeds.

use phazeai_core::privacy;
use phazeai_core::search::{
    BuildStats, CodeSearch, IndexStatus, SearchError, SearchHit, SearchLimits, SearchResults,
};
use phazeai_core::tools::Tool;
use phazeai_core::{BuildIndexTool, CodeSearchTool};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, UNIX_EPOCH};

// ═════════════════════════════ 1. helpers ═══════════════════════════════════

/// A throw-away project directory.
struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("code-search-spec-")
            .tempdir()
            .unwrap();
        let root = dir.path().to_path_buf();
        Repo { _dir: dir, root }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        self.write_bytes(rel, text.as_bytes())
    }

    fn write_bytes(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.p(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        p
    }

    fn remove(&self, rel: &str) {
        let p = self.p(rel);
        if p.is_dir() {
            fs::remove_dir_all(p).unwrap();
        } else {
            fs::remove_file(p).unwrap();
        }
    }

    fn cs(&self) -> CodeSearch {
        CodeSearch::new(self.root.clone())
    }

    fn cs_with(&self, limits: SearchLimits) -> CodeSearch {
        CodeSearch::with_limits(self.root.clone(), limits)
    }
}

/// Give `path` a modification time that only depends on `n`. File systems
/// stamp writes with a coarse clock, so two same-size edits made in quick
/// succession can carry equal timestamps (the engine documents that limit);
/// tests that edit a file to different content of the same size use this to
/// make the edit observable.
fn set_mtime(path: &Path, n: u64) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000_000 + n))
        .unwrap();
}

/// `file` relative to `root`, with `/` separators.
fn rel(root: &Path, file: &str) -> String {
    Path::new(file)
        .strip_prefix(root)
        .unwrap_or_else(|_| panic!("{file} is not below {}", root.display()))
        .to_string_lossy()
        .replace('\\', "/")
}

fn names(cs: &CodeSearch, res: &SearchResults) -> Vec<String> {
    let root = cs.root();
    res.matches.iter().map(|h| rel(&root, &h.file)).collect()
}

fn blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// What the module promises about one hit, checked against the file on disk:
/// an absolute path inside the root, a 1-based inclusive line range of at most
/// ten lines, and a snippet that is exactly those lines.
fn check_hit(root: &Path, h: &SearchHit) {
    let path = Path::new(&h.file);
    assert!(path.is_absolute(), "hit path {} is not absolute", h.file);
    assert!(
        path.starts_with(root),
        "hit {} is outside the root {}",
        h.file,
        root.display()
    );
    assert!(
        h.start_line >= 1 && h.start_line <= h.end_line,
        "bad line range {}..{} for {}",
        h.start_line,
        h.end_line,
        h.file
    );
    assert!(
        h.end_line - h.start_line < 10,
        "snippet of {} spans {} lines",
        h.file,
        h.end_line - h.start_line + 1
    );
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("hit {} cannot be read back: {e}", h.file));
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        h.end_line <= lines.len(),
        "{}: end_line {} past the {} lines of the file",
        h.file,
        h.end_line,
        lines.len()
    );
    let expected = lines[h.start_line - 1..h.end_line]
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    if h.snippet != expected {
        // One enormous line is shortened to a window of it (about 600
        // characters around the match), each cut side marked with an ellipsis.
        let window = h.snippet.strip_prefix('\u{2026}').unwrap_or(&h.snippet);
        let window = window.strip_suffix('\u{2026}').unwrap_or(window);
        assert!(
            h.start_line == h.end_line
                && h.snippet.chars().count() <= 600
                && expected.contains(window),
            "{}: snippet is not lines {}..={}\n--- snippet ---\n{}\n--- file lines ---\n{}",
            h.file,
            h.start_line,
            h.end_line,
            h.snippet,
            expected
        );
    }
    let shown: Vec<&str> = h.snippet.lines().collect();
    assert!(
        !blank(shown[0]) && !blank(shown[shown.len() - 1]),
        "{}: snippet starts or ends with a blank line: {:?}",
        h.file,
        h.snippet
    );
}

/// Invariants of every result list, whatever the query: bounded by `top_k`,
/// one hit per file, best first, positive finite scores, correct hits.
fn check_results(cs: &CodeSearch, res: &SearchResults, top_k: usize) {
    assert!(
        res.matches.len() <= top_k,
        "{} hits for top_k {top_k}",
        res.matches.len()
    );
    let root = cs.root();
    let mut seen = HashSet::new();
    let mut prev = f32::INFINITY;
    for h in &res.matches {
        assert!(seen.insert(h.file.clone()), "two hits for {}", h.file);
        assert!(
            h.score.is_finite() && h.score >= 0.0,
            "score {} for {}",
            h.score,
            h.file
        );
        assert!(
            h.score <= prev,
            "hits are not best-first: {} after {}",
            h.score,
            prev
        );
        prev = h.score;
        check_hit(&root, h);
    }
}

/// Search with `top_k`, panicking on errors, and check the invariants.
fn search_k(cs: &CodeSearch, q: &str, k: usize) -> SearchResults {
    let res = cs
        .search_blocking(q, k)
        .unwrap_or_else(|e| panic!("search {q:?} failed: {e}"));
    check_results(cs, &res, k);
    res
}

fn search(cs: &CodeSearch, q: &str) -> SearchResults {
    search_k(cs, q, 50)
}

/// Relative paths of the hits for `q`, best first.
fn find(cs: &CodeSearch, q: &str) -> Vec<String> {
    names(cs, &search(cs, q))
}

fn top1(cs: &CodeSearch, q: &str) -> Option<String> {
    find(cs, q).into_iter().next()
}

/// A full build whose bookkeeping is checked: files added minus files removed
/// is the change in the number of indexed files, and the stats agree with
/// `status()`.
fn sync(cs: &CodeSearch, paths: &[PathBuf]) -> BuildStats {
    let before = cs.status().files;
    let st = cs
        .build_blocking(paths)
        .unwrap_or_else(|e| panic!("build failed: {e}"));
    assert_eq!(
        st.total_files + st.removed,
        before + st.indexed,
        "stats do not add up: before={before} {st:?}"
    );
    let status = cs.status();
    assert_eq!(status.files, st.total_files, "{st:?} vs {status:?}");
    assert_eq!(status.chunks, st.chunks, "{st:?} vs {status:?}");
    assert!(status.built && !status.building, "{status:?}");
    st
}

/// Run `f` on its own thread and fail instead of hanging if it takes too long.
fn within<T: Send + 'static>(secs: u64, what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let handle = std::thread::spawn(f);
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !handle.is_finished() {
        assert!(
            Instant::now() < deadline,
            "{what}: still running after {secs}s (hang or deadlock)"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    match handle.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    }
}

/// Small deterministic generator (xorshift64*), so failures replay from the seed.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        ((self.next() >> 11) % n as u64) as usize
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// `n` as a lowercase letter word (base 26): a token that survives the
/// tokenizer whole, is never split and never collides with another `n`.
fn letters(mut n: usize) -> String {
    let mut s = String::new();
    loop {
        s.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
        if n == 0 {
            break;
        }
    }
    s.chars().rev().collect()
}

/// 1-based number of the first line of `text` containing `needle`.
fn line_of(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not found in the text"))
        + 1
}

fn covers(h: &SearchHit, line: usize) -> bool {
    h.start_line <= line && line <= h.end_line
}

/// Serialises the tests that change the process-wide active privacy project.
static PRIVACY: Mutex<()> = Mutex::new(());

struct ActiveProject {
    _guard: MutexGuard<'static, ()>,
}

impl ActiveProject {
    /// Make `root` the active project (its policy file is whatever is on disk).
    fn set(root: &Path) -> Self {
        let guard = PRIVACY.lock().unwrap_or_else(|e| e.into_inner());
        privacy::set_active_project(root);
        ActiveProject { _guard: guard }
    }

    /// No active project: only the built-in secrets list applies.
    fn none() -> Self {
        let guard = PRIVACY.lock().unwrap_or_else(|e| e.into_inner());
        privacy::clear_active_project();
        ActiveProject { _guard: guard }
    }
}

impl Drop for ActiveProject {
    fn drop(&mut self) {
        privacy::clear_active_project();
    }
}

// ═══════════════ 2. a realistic repo, identifier splitting, ranking ═════════

const MINI_REPO: &[(&str, &str)] = &[
    (
        "src/main.rs",
        "mod config;\nmod network;\n\nfn main() {\n    let settings = config::loader::load_settings(\"app.json\");\n    network::server::start_server(settings.port);\n}\n",
    ),
    (
        "src/network/server.rs",
        "use std::io::{Read, Write};\nuse std::net::{TcpListener, TcpStream};\n\n/// Bind the socket and serve clients until shutdown.\npub fn start_server(port: u16) -> std::io::Result<()> {\n    let listener = TcpListener::bind((\"0.0.0.0\", port))?;\n    for stream in listener.incoming() {\n        let stream = stream?;\n        std::thread::spawn(move || handle_connection(stream));\n    }\n    Ok(())\n}\n\nfn handle_connection(mut stream: TcpStream) {\n    let mut buf = [0u8; 1024];\n    let n = stream.read(&mut buf).unwrap_or(0);\n    let _ = stream.write_all(&buf[..n]);\n}\n",
    ),
    (
        "src/config/loader.rs",
        "use serde_json::Value;\nuse std::path::Path;\n\npub struct Settings {\n    pub port: u16,\n    pub data_dir: String,\n}\n\n/// Parse the application settings from a JSON document.\npub fn parse_json_config(text: &str) -> Result<Settings, String> {\n    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;\n    Ok(Settings {\n        port: v[\"port\"].as_u64().unwrap_or(8080) as u16,\n        data_dir: v[\"data_dir\"].as_str().unwrap_or(\".\").to_string(),\n    })\n}\n\npub fn load_settings(path: &Path) -> Result<Settings, String> {\n    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;\n    parse_json_config(&text)\n}\n",
    ),
    (
        "src/auth/token.rs",
        "use hmac::{Hmac, Mac};\nuse sha2::Sha256;\n\n/// Check the HMAC signature of a JWT and return its claims.\npub fn verify_jwt_signature(token: &str, secret: &[u8]) -> Option<Claims> {\n    let mut parts = token.split('.');\n    let header = parts.next()?;\n    let payload = parts.next()?;\n    let signature = parts.next()?;\n    let mut mac = Hmac::<Sha256>::new_from_slice(secret).ok()?;\n    mac.update(header.as_bytes());\n    mac.update(b\".\");\n    mac.update(payload.as_bytes());\n    mac.verify_slice(&decode(signature)?).ok()?;\n    Claims::from_payload(payload)\n}\n",
    ),
    (
        "web/api.js",
        "export async function fetchUserProfile() {\n  const res = await fetch('/api/profile');\n  if (!res.ok) throw new Error('profile request failed');\n  return res.json();\n}\n",
    ),
    (
        "web/app.js",
        "import { fetchUserProfile } from './api.js';\n\nexport async function renderDashboard(container) {\n  const profile = await fetchUserProfile();\n  const chartData = profile.activity.map((a) => ({ x: a.day, y: a.count }));\n  container.innerHTML = `<h1>${profile.name}</h1>`;\n  drawChart(container, chartData);\n}\n",
    ),
    (
        "web/util.ts",
        "export function debounce<T extends (...args: any[]) => void>(fn: T, delayMs: number): T {\n  let timer: ReturnType<typeof setTimeout> | undefined;\n  return ((...args: any[]) => {\n    if (timer) clearTimeout(timer);\n    timer = setTimeout(() => fn(...args), delayMs);\n  }) as T;\n}\n",
    ),
    (
        "scripts/backup.py",
        "import os\nimport shutil\nimport time\n\n\ndef rotate_backups(directory, keep_days=30):\n    \"\"\"Delete archives older than keep_days.\"\"\"\n    cutoff = time.time() - keep_days * 86400\n    for name in os.listdir(directory):\n        path = os.path.join(directory, name)\n        if os.path.getmtime(path) < cutoff:\n            shutil.rmtree(path, ignore_errors=True)\n",
    ),
    (
        "cmd/hooks/main.go",
        "package main\n\nimport (\n\t\"io\"\n\t\"net/http\"\n)\n\nfunc handleWebhook(w http.ResponseWriter, r *http.Request) {\n\tpayload, err := io.ReadAll(r.Body)\n\tif err != nil {\n\t\thttp.Error(w, \"bad body\", http.StatusBadRequest)\n\t\treturn\n\t}\n\tprocess(payload)\n}\n",
    ),
    (
        "java/com/acme/billing/InvoiceService.java",
        "package com.acme.billing;\n\npublic class InvoiceService {\n    private final TaxTable taxTable;\n\n    public BigDecimal computeTax(Invoice invoice) {\n        BigDecimal rate = taxTable.rateFor(invoice.getRegion());\n        return invoice.getSubtotal().multiply(rate);\n    }\n}\n",
    ),
    (
        "native/vec.c",
        "#include <stddef.h>\n\nvoid matrix_multiply(const float *a, const float *b, float *out, size_t n) {\n    for (size_t i = 0; i < n; i++)\n        for (size_t j = 0; j < n; j++) {\n            float acc = 0.0f;\n            for (size_t k = 0; k < n; k++) acc += a[i * n + k] * b[k * n + j];\n            out[i * n + j] = acc;\n        }\n}\n",
    ),
    (
        "docs/ARCHITECTURE.md",
        "# Architecture\n\nThe scheduler assigns queued jobs to a pool of workers. Each worker pulls\njobs from its own queue and reports completion back to the scheduler.\n\n## Storage\n\nResults are written to the data directory, one file per finished job.\n",
    ),
    (
        "Cargo.toml",
        "[package]\nname = \"acme-tool\"\nversion = \"0.3.1\"\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nserde_json = \"1\"\nhmac = \"0.12\"\n",
    ),
    (
        "README.md",
        "# Acme Tool\n\nA small server that reads a JSON config, checks tokens and schedules backup jobs.\nSee docs/ARCHITECTURE.md for the design.\n",
    ),
];

fn mini_repo() -> (Repo, CodeSearch) {
    let repo = Repo::new();
    for (rel, text) in MINI_REPO {
        repo.write(rel, text);
    }
    let cs = repo.cs();
    (repo, cs)
}

#[test]
fn natural_queries_find_the_defining_file_of_a_multi_language_repo() {
    let (_repo, cs) = mini_repo();
    // (query, expected first file, a word the snippet must show)
    let cases: &[(&str, &str, &str)] = &[
        (
            "tcp listener incoming connection",
            "src/network/server.rs",
            "listener",
        ),
        ("json config", "src/config/loader.rs", "json"),
        ("verify jwt signature", "src/auth/token.rs", "signature"),
        ("dashboard chart", "web/app.js", "chart"),
        ("debounce delay timer", "web/util.ts", "debounce"),
        ("rotate backups keep days", "scripts/backup.py", "keep_days"),
        ("handle webhook", "cmd/hooks/main.go", "webhook"),
        (
            "invoice tax",
            "java/com/acme/billing/InvoiceService.java",
            "tax",
        ),
        ("matrix multiply", "native/vec.c", "matrix"),
        ("scheduler worker pool", "docs/ARCHITECTURE.md", "scheduler"),
        ("serde derive features", "Cargo.toml", "derive"),
    ];
    for (q, file, word) in cases {
        let res = search_k(&cs, q, 5);
        let got = names(&cs, &res);
        assert_eq!(
            got.first().map(String::as_str),
            Some(*file),
            "{q:?} -> {got:?}"
        );
        let hit = &res.matches[0];
        assert!(
            hit.snippet.to_lowercase().contains(word),
            "{q:?}: the snippet does not show {word:?}:\n{}",
            hit.snippet
        );
    }
    // The prose in README.md mentions several of these words, but the files
    // that define them rank above it.
    let got = find(&cs, "json config");
    let readme = got.iter().position(|f| f == "README.md");
    assert!(readme.is_none_or(|i| i > 0), "{got:?}");
}

#[test]
fn hits_point_at_the_line_that_matches() {
    let (repo, cs) = mini_repo();
    for (q, file, needle) in [
        (
            "TcpListener bind",
            "src/network/server.rs",
            "TcpListener::bind",
        ),
        (
            "rate multiply subtotal",
            "java/com/acme/billing/InvoiceService.java",
            "getSubtotal",
        ),
        ("hmac verify_slice", "src/auth/token.rs", "verify_slice"),
        ("clearTimeout", "web/util.ts", "clearTimeout"),
        ("rmtree", "scripts/backup.py", "rmtree"),
    ] {
        let res = search_k(&cs, q, 3);
        let hit = &res.matches[0];
        assert_eq!(rel(&repo.root, &hit.file), file, "{q}");
        let text = fs::read_to_string(&hit.file).unwrap();
        let line = line_of(&text, needle);
        assert!(
            covers(hit, line),
            "{q}: {needle} is on line {line}, hit is {}..{}",
            hit.start_line,
            hit.end_line
        );
        assert!(hit.snippet.contains(needle), "{q}:\n{}", hit.snippet);
    }
}

#[test]
fn identifier_spellings_find_each_other() {
    let repo = Repo::new();
    repo.write(
        "a/camel.js",
        "export function parseJsonConfig(path) { return load(path); }\n",
    );
    repo.write(
        "b/snake.py",
        "def parse_json_config(path):\n    return load(path)\n",
    );
    repo.write("c/pascal.cs", "class ParseJSONConfig { void Run() {} }\n");
    repo.write(
        "d/screaming.rs",
        "const MAX_JSON_CONFIG_SIZE: usize = 10;\n",
    );
    // Each distractor has only part of the phrase.
    repo.write("e/xml.rs", "fn parse_xml_manifest() {}\n");
    repo.write("f/schema.rs", "fn json_schema_validate() {}\n");
    repo.write("g/file.rs", "fn load_config_file() {}\n");
    let cs = repo.cs();

    let four: BTreeSet<&str> = ["a/camel.js", "b/snake.py", "c/pascal.cs", "d/screaming.rs"].into();
    for q in [
        "json config",
        "JSON CONFIG",
        "Json Config",
        "json_config",
        "jsonConfig",
        "JSONConfig",
    ] {
        let got = find(&cs, q);
        let head: BTreeSet<&str> = got.iter().take(4).map(String::as_str).collect();
        assert_eq!(head, four, "query {q:?} -> {got:?}");
    }
    // The identifier itself, in any of its spellings, also reaches the others
    // through its words, and the exact spelling ranks first.
    let got = find(&cs, "parse_json_config");
    assert_eq!(got[0], "b/snake.py", "{got:?}");
    for f in ["a/camel.js", "c/pascal.cs"] {
        assert!(got.iter().any(|g| g == f), "{f} missing from {got:?}");
    }
    let got = find(&cs, "parseJsonConfig");
    let head: BTreeSet<&str> = got.iter().take(2).map(String::as_str).collect();
    assert_eq!(head, ["a/camel.js", "c/pascal.cs"].into(), "{got:?}");
    let got = find(&cs, "ParseJSONConfig");
    let head: BTreeSet<&str> = got.iter().take(2).map(String::as_str).collect();
    assert_eq!(head, ["a/camel.js", "c/pascal.cs"].into(), "{got:?}");
    // Distractors never outrank files that have the whole phrase.
    let got = find(&cs, "json config");
    for (i, f) in got.iter().enumerate() {
        if ["e/xml.rs", "f/schema.rs", "g/file.rs"].contains(&f.as_str()) {
            assert!(i >= 4, "{f} at rank {i}: {got:?}");
        }
    }
}

#[test]
fn query_case_order_and_repeats_do_not_change_the_answer() {
    let (_repo, cs) = mini_repo();
    let base = search(&cs, "json config");
    let base_json = serde_json::to_value(&base).unwrap();
    for q in [
        "JSON CONFIG",
        "  json   config  ",
        "json, config!",
        "json json config config",
        "(json) [config]",
    ] {
        let v = serde_json::to_value(search(&cs, q)).unwrap();
        assert_eq!(v, base_json, "query {q:?} differs");
    }
    // Word order changes nothing but possibly the last digit of a score.
    let swapped = search(&cs, "config json");
    assert_eq!(names(&cs, &swapped), names(&cs, &base));
    for (a, b) in swapped.matches.iter().zip(&base.matches) {
        assert!(
            (a.score - b.score).abs() < 1e-3,
            "{} vs {}",
            a.score,
            b.score
        );
        assert_eq!((a.start_line, a.end_line), (b.start_line, b.end_line));
    }
}

#[test]
fn a_rare_word_outweighs_common_words_and_a_definition_outweighs_mentions() {
    let repo = Repo::new();
    for i in 0..40 {
        repo.write(
            &format!("noise/n{i:02}.rs"),
            &format!("// widget {i}\nfn helper_{i}() {{ let widget = make(); widget.run(); }}\n"),
        );
    }
    repo.write(
        "rare/zebra.rs",
        "fn crossing() { let widget = make(); zebra_stripes(widget); }\n",
    );
    let cs = repo.cs();
    assert_eq!(
        top1(&cs, "widget zebra_stripes").as_deref(),
        Some("rare/zebra.rs")
    );
    assert_eq!(
        top1(&cs, "widget stripes").as_deref(),
        Some("rare/zebra.rs")
    );
    assert_eq!(top1(&cs, "zebra").as_deref(), Some("rare/zebra.rs"));

    // Three single-word mentions each, one real definition containing all.
    let repo = Repo::new();
    for i in 0..12 {
        repo.write(
            &format!("p/parse{i}.rs"),
            &format!("// we parse stuff {i}\nfn p{i}() {{ parse(); }}\n"),
        );
        repo.write(
            &format!("j/json{i}.rs"),
            &format!("// we use json {i}\nfn j{i}() {{ json(); }}\n"),
        );
        repo.write(
            &format!("c/conf{i}.rs"),
            &format!("// we need config {i}\nfn c{i}() {{ config(); }}\n"),
        );
    }
    repo.write("spam/json_spam.rs", "// json json json json json json json json json json\nfn s() { json(); json(); json(); }\n");
    repo.write(
        "real/definition.rs",
        "fn load() { let c = parse(json(config())); }\n",
    );
    let cs = repo.cs();
    let got = find(&cs, "parse json config");
    assert_eq!(got[0], "real/definition.rs", "{got:?}");
}

#[test]
fn the_same_word_in_many_files_gives_one_hit_per_file_and_honours_top_k() {
    let repo = Repo::new();
    for i in 0..100 {
        repo.write(
            &format!("many/f{i:03}.rs"),
            &format!("fn shared_marker() {{}}\nfn uniq_{i:03}() {{}}\n"),
        );
    }
    let cs = repo.cs();
    for k in [1, 2, 5, 20] {
        let res = search_k(&cs, "shared_marker", k);
        assert_eq!(res.matches.len(), k, "top_k {k}");
    }
    // Far more than any sane page: bounded, distinct, no overflow.
    for k in [100, 1000, usize::MAX / 4, usize::MAX] {
        let res = search_k(&cs, "shared_marker", k);
        assert!(
            res.matches.len() >= 20 && res.matches.len() <= 100,
            "top_k {k}: {}",
            res.matches.len()
        );
    }
    // The whole identifier decides between files that share its pieces.
    for i in [0, 1, 7, 42, 99] {
        let q = format!("uniq_{i:03}");
        let got = find(&cs, &q);
        assert_eq!(got[0], format!("many/f{i:03}.rs"), "{q}");
    }
}

#[test]
fn the_best_chunk_of_a_file_is_the_hit() {
    let repo = Repo::new();
    // 200 lines without blank lines: several chunks. One mention early, a
    // dense cluster late.
    let mut lines: Vec<String> = (0..200).map(|i| format!("let filler_{i} = {i};")).collect();
    lines[19] = "// the quokka appears once here".into();
    lines[149] = "quokka quokka quokka quokka".into();
    repo.write("zoo.rs", &(lines.join("\n") + "\n"));
    let cs = repo.cs();
    let res = search(&cs, "quokka");
    assert_eq!(res.matches.len(), 1, "one hit per file");
    assert!(covers(&res.matches[0], 150), "{:?}", res.matches[0]);
    assert!(res.matches[0].snippet.contains("quokka quokka quokka"));
}

#[test]
fn directory_and_file_names_count_as_keywords() {
    let repo = Repo::new();
    repo.write("lsp/manager.rs", "pub struct Registry;\n");
    repo.write("lsp/client.rs", "pub struct Handle;\n");
    repo.write("ui/panel.rs", "pub struct Widget;\n");
    repo.write("my dir/some file.rs", "pub struct Quux;\n");
    let cs = repo.cs();
    assert_eq!(top1(&cs, "lsp manager").as_deref(), Some("lsp/manager.rs"));
    assert_eq!(top1(&cs, "lsp client").as_deref(), Some("lsp/client.rs"));
    assert_eq!(
        top1(&cs, "some file").as_deref(),
        Some("my dir/some file.rs")
    );
    assert_eq!(top1(&cs, "panel").as_deref(), Some("ui/panel.rs"));
    // A path word alone is enough to match.
    let got = find(&cs, "lsp");
    assert_eq!(got.len(), 2, "{got:?}");
}

// ═════════════════ 3. awkward files, limits and encodings ═══════════════════

#[test]
fn a_minified_one_line_file_is_found_and_its_snippet_stays_small() {
    let repo = Repo::new();
    let filler = |out: &mut String, until: usize| {
        let mut i = 0;
        while out.len() < until {
            out.push_str(&format!(
                "n.a{}=function(e){{return e+{}}};",
                i % 97,
                i % 13
            ));
            i += 1;
        }
    };
    let mut line = String::from("!function(e,t){var n={};");
    filler(&mut line, 350_000);
    line.push_str("function zxqUniqueMinifiedMarker(){return 1}");
    filler(&mut line, 700_000);
    line.push_str("}(window);");
    assert!(line.len() < 1 << 20);
    repo.write("static/app.min.js", &line);
    repo.write("static/other.js", "function other() { return 2; }\n");
    // A base64 blob on one line next to a normal key: the blob is noise, the key is found.
    let blob: String = (0..200_000)
        .map(|i| (b'A' + (i * 7 % 26) as u8) as char)
        .collect();
    repo.write(
        "static/blob.json",
        &format!("{{\n  \"data\": \"{blob}\",\n  \"needle_in_json\": 1\n}}\n"),
    );
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.skipped, st.errors), (3, 0, 0), "{st:?}");

    let res = search(&cs, "zxqUniqueMinifiedMarker");
    assert_eq!(names(&cs, &res), ["static/app.min.js"]);
    let h = &res.matches[0];
    assert_eq!((h.start_line, h.end_line), (1, 1));
    assert!(
        h.snippet.chars().count() <= 1000,
        "snippet of {} chars",
        h.snippet.chars().count()
    );
    assert!(serde_json::to_string(&res).unwrap().len() < 20_000);

    let res = search(&cs, "needle_in_json");
    assert_eq!(names(&cs, &res), ["static/blob.json"]);
    assert!(covers(&res.matches[0], 3), "{:?}", res.matches[0]);
}

#[test]
fn a_file_over_the_default_size_limit_is_skipped_not_fatal() {
    let repo = Repo::new();
    repo.write(
        "big/huge.js",
        &format!("var x = 'hugefilemarker';{}\n", " ".repeat(1_100_000)),
    );
    repo.write("big/small.js", "var y = 'smallfilemarker';\n");
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.skipped, st.errors), (1, 1, 0), "{st:?}");
    assert!(find(&cs, "hugefilemarker").is_empty());
    assert_eq!(find(&cs, "smallfilemarker"), ["big/small.js"]);
}

#[test]
fn a_ten_thousand_line_file_has_exact_line_numbers_everywhere() {
    let repo = Repo::new();
    let n = 10_000;
    let plain: String = (0..n)
        .map(|i| format!("fn f{i}() {{ pl{}(); }}\n", letters(i)))
        .collect();
    // Paragraphs of 25 lines separated by a blank line.
    let paragraphs: String = (0..n)
        .map(|i| {
            let sep = if i % 25 == 24 { "\n" } else { "" };
            format!("let v{i} = pg{}();\n{sep}", letters(i))
        })
        .collect();
    repo.write("big/plain.rs", &plain);
    repo.write("big/paragraphs.rs", &paragraphs);
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!(st.indexed, 2);
    assert!(
        st.chunks >= 2 * n / 80,
        "{} chunks for {} lines",
        st.chunks,
        2 * n
    );
    assert!(st.chunks <= 2 * n);

    let mut rng = Rng::new(10_000);
    let mut sample: Vec<usize> = (0..40).map(|_| rng.below(n)).collect();
    sample.extend([0, 1, 79, 80, 81, n - 2, n - 1]);
    for i in sample {
        let res = search(&cs, &format!("pl{}", letters(i)));
        assert_eq!(names(&cs, &res), ["big/plain.rs"], "line {}", i + 1);
        assert!(
            covers(&res.matches[0], i + 1),
            "line {} not in {:?}",
            i + 1,
            res.matches[0]
        );
        assert!(
            res.matches[0].snippet.contains(&format!("fn f{i}()")),
            "{:?}",
            res.matches[0]
        );

        let line_no = i + 1 + i / 25; // blank separators shift later lines
        let res = search(&cs, &format!("pg{}", letters(i)));
        assert_eq!(names(&cs, &res), ["big/paragraphs.rs"]);
        assert!(
            covers(&res.matches[0], line_no),
            "line {line_no} not in {:?}",
            res.matches[0]
        );
    }
}

#[test]
fn files_exactly_at_the_size_limit_are_indexed_and_one_byte_more_is_not() {
    let repo = Repo::new();
    let limits = SearchLimits {
        max_file_bytes: 2000,
        ..SearchLimits::default()
    };
    let body = |word: &str, size: usize| {
        let mut s = format!("fn {word}() {{}}\n// ");
        let pad = size - s.len() - 1;
        s.push_str(&"x".repeat(pad));
        s.push('\n');
        assert_eq!(s.len(), size);
        s
    };
    repo.write("limit/at.rs", &body("atlimitword", 2000));
    repo.write("limit/under.rs", &body("underlimitword", 1999));
    repo.write("limit/over.rs", &body("overlimitword", 2001));
    let cs = repo.cs_with(limits);
    let st = sync(&cs, &[]);
    assert_eq!(
        (st.indexed, st.skipped, st.errors, st.truncated),
        (2, 1, 0, false),
        "{st:?}"
    );
    assert_eq!(find(&cs, "atlimitword"), ["limit/at.rs"]);
    assert_eq!(find(&cs, "underlimitword"), ["limit/under.rs"]);
    assert!(find(&cs, "overlimitword").is_empty());

    // The boundary moves with the file: grow `at` by one byte, shrink `over` by one.
    let at = repo.write("limit/at.rs", &body("atlimitword", 2001));
    set_mtime(&at, 5);
    repo.write("limit/over.rs", &body("overlimitword", 2000));
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.removed, st.skipped), (1, 1, 1), "{st:?}");
    assert!(
        find(&cs, "atlimitword").is_empty(),
        "a file that outgrew the limit must leave the index"
    );
    assert_eq!(find(&cs, "overlimitword"), ["limit/over.rs"]);

    // refresh_file obeys the same limit.
    let under = repo.write("limit/under.rs", &body("underlimitword", 2001));
    cs.refresh_file(&under);
    assert!(find(&cs, "underlimitword").is_empty());
    repo.write("limit/under.rs", &body("underlimitword", 2000));
    cs.refresh_file(&under);
    assert_eq!(find(&cs, "underlimitword"), ["limit/under.rs"]);
}

#[test]
fn extreme_limits_do_not_overflow_or_panic() {
    let repo = Repo::new();
    repo.write("a.rs", "fn limitprobealpha() {}\n");
    repo.write("b.rs", "fn limitprobebeta() {}\n");
    for limits in [
        SearchLimits {
            max_file_bytes: u64::MAX,
            max_total_bytes: u64::MAX,
            max_files: usize::MAX,
        },
        SearchLimits {
            max_file_bytes: 0,
            max_total_bytes: u64::MAX,
            max_files: usize::MAX,
        },
        SearchLimits {
            max_file_bytes: u64::MAX,
            max_total_bytes: 0,
            max_files: usize::MAX,
        },
        SearchLimits {
            max_file_bytes: u64::MAX,
            max_total_bytes: u64::MAX,
            max_files: 0,
        },
        SearchLimits {
            max_file_bytes: 0,
            max_total_bytes: 0,
            max_files: 0,
        },
    ] {
        let cs = repo.cs_with(limits);
        let st = cs.build_blocking(&[]).unwrap();
        let everything =
            limits.max_file_bytes > 0 && limits.max_total_bytes > 0 && limits.max_files > 0;
        assert_eq!(
            st.total_files,
            if everything { 2 } else { 0 },
            "{limits:?} {st:?}"
        );
        // Files that are too big on their own are skipped, not a reason to truncate.
        let capped =
            limits.max_file_bytes > 0 && (limits.max_total_bytes == 0 || limits.max_files == 0);
        assert_eq!(st.truncated, capped, "{limits:?} {st:?}");
        let res = search(&cs, "limitprobealpha");
        assert_eq!(res.matches.len(), usize::from(everything), "{limits:?}");
        // Re-running changes nothing, truncated or not.
        let again = cs.build_blocking(&[]).unwrap();
        assert_eq!(
            (again.indexed, again.updated, again.removed),
            (0, 0, 0),
            "{limits:?} {again:?}"
        );
        assert_eq!(again.truncated, st.truncated);
    }
}

#[test]
fn the_default_limits_are_the_contracts() {
    let d = SearchLimits::default();
    assert_eq!(
        (d.max_files, d.max_file_bytes, d.max_total_bytes),
        (50_000, 1 << 20, 256 << 20)
    );
}

#[test]
fn max_files_is_a_cap_that_is_reported_everywhere() {
    let repo = Repo::new();
    let limits = SearchLimits {
        max_files: 3,
        ..SearchLimits::default()
    };
    for n in ["aa", "bb", "cc"] {
        repo.write(
            &format!("{n}.rs"),
            &format!("fn shared_cap_word() {{ {n}capmarker(); }}\n"),
        );
    }
    let cs = repo.cs_with(limits);
    let st = sync(&cs, &[]);
    assert_eq!(
        (st.total_files, st.truncated),
        (3, false),
        "exactly at the cap is not over it: {st:?}"
    );
    assert!(!cs.status().truncated && !search(&cs, "shared_cap_word").index_truncated);

    repo.write("dd.rs", "fn shared_cap_word() { ddcapmarker(); }\n");
    let st = sync(&cs, &[]);
    assert_eq!((st.total_files, st.truncated), (3, true), "{st:?}");
    assert!(cs.status().truncated);
    let res = search(&cs, "shared_cap_word");
    assert!(res.index_truncated);
    assert_eq!(res.matches.len(), 3);
    assert_eq!(
        serde_json::to_value(&res).unwrap()["index_truncated"],
        json!(true)
    );

    // Truncation is stable: building again neither churns nor forgets that it is truncated.
    let again = sync(&cs, &[]);
    assert_eq!(
        (again.indexed, again.updated, again.removed, again.truncated),
        (0, 0, 0, true),
        "{again:?}"
    );

    // Make room: the file that did not fit is admitted by the next build and the flag clears.
    repo.remove("aa.rs");
    let st = sync(&cs, &[]);
    assert_eq!((st.total_files, st.truncated), (3, false), "{st:?}");
    assert!(!cs.status().truncated);
    assert!(!search(&cs, "shared_cap_word").index_truncated);
    assert_eq!(find(&cs, "ddcapmarker").len(), 1);
}

#[test]
fn max_total_bytes_is_inclusive_and_never_serves_stale_content_of_an_evicted_file() {
    let repo = Repo::new();
    let text = |word: &str, size: usize| {
        let mut s = format!("fn {word}() {{}}\n// ");
        s.push_str(&"y".repeat(size - s.len() - 1));
        s.push('\n');
        s
    };
    repo.write("a.rs", &text("firstbytesword", 100));
    repo.write("b.rs", &text("secondbytesword", 100));
    repo.write("c.rs", &text("thirdbytesword", 100));
    let cs = repo.cs_with(SearchLimits {
        max_total_bytes: 300,
        ..SearchLimits::default()
    });
    let st = sync(&cs, &[]);
    assert_eq!(
        (st.total_files, st.truncated),
        (3, false),
        "300 bytes of 300 fit: {st:?}"
    );

    let tight = repo.cs_with(SearchLimits {
        max_total_bytes: 299,
        ..SearchLimits::default()
    });
    let st = sync(&tight, &[]);
    assert_eq!((st.total_files, st.truncated), (2, true), "{st:?}");

    // A file that grows past the budget leaves the index; its old words must not linger.
    let a = repo.write("a.rs", &text("firstbytesword2", 250));
    set_mtime(&a, 9);
    let st = sync(&cs, &[]);
    assert!(st.truncated, "{st:?}");
    assert!(
        find(&cs, "firstbytesword").is_empty(),
        "stale content of an evicted file"
    );
    assert!(find(&cs, "firstbytesword2").len() <= 1);
    assert_eq!(find(&cs, "secondbytesword"), ["b.rs"]);
    assert_eq!(find(&cs, "thirdbytesword"), ["c.rs"]);
}

#[test]
fn empty_and_blank_files_are_harmless() {
    let repo = Repo::new();
    repo.write("empty.rs", "");
    repo.write("blank.rs", "\n\n\n   \n\t\n");
    repo.write("newline.py", "\n");
    repo.write("real.rs", "fn realonemarkword() {}\n");
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!((st.errors, st.skipped), (0, 0), "{st:?}");
    assert!(st.indexed >= 1 && st.indexed <= 4, "{st:?}");
    assert_eq!(find(&cs, "realonemarkword"), ["real.rs"]);
    for _ in 0..3 {
        let again = sync(&cs, &[]);
        assert_eq!(
            (again.indexed, again.updated, again.removed),
            (0, 0, 0),
            "{again:?}"
        );
    }
    // An empty file that gets content is found; a file emptied is forgotten.
    repo.write("empty.rs", "fn wasemptymarkword() {}\n");
    let st = sync(&cs, &[]);
    assert_eq!(st.indexed + st.updated, 1, "{st:?}");
    assert_eq!(find(&cs, "wasemptymarkword"), ["empty.rs"]);
    repo.write("real.rs", "");
    assert!(
        find(&cs, "realonemarkword").is_empty(),
        "emptied file still matches"
    );
    sync(&cs, &[]);
    assert!(find(&cs, "realonemarkword").is_empty());
    assert_eq!(find(&cs, "wasemptymarkword"), ["empty.rs"]);
}

#[test]
fn crlf_files_have_the_same_line_numbers_as_lf_files() {
    let repo = Repo::new();
    let logical: Vec<String> = (1..=120)
        .map(|i| match i {
            57 => "fn crlf_marker_target() {}".to_string(),
            _ if i % 10 == 0 => String::new(),
            _ => format!("let v{i} = {i};"),
        })
        .collect();
    repo.write("eol/lf.rs", &(logical.join("\n") + "\n"));
    repo.write("eol/crlf.rs", &(logical.join("\r\n") + "\r\n"));
    // Same text, no trailing newline, and a mix of both endings.
    repo.write("eol/crlf_no_final_eol.rs", &logical.join("\r\n"));
    let mixed: String = logical
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{l}{}", if i % 2 == 0 { "\r\n" } else { "\n" }))
        .collect();
    repo.write("eol/mixed.rs", &mixed);
    let cs = repo.cs();
    let res = search(&cs, "crlf_marker_target");
    assert_eq!(res.matches.len(), 4, "{:?}", names(&cs, &res));
    let lf = res
        .matches
        .iter()
        .find(|h| h.file.ends_with("lf.rs"))
        .unwrap();
    assert!(covers(lf, 57));
    for h in &res.matches {
        assert!(
            covers(h, 57),
            "{}: {}..{}",
            h.file,
            h.start_line,
            h.end_line
        );
        assert!(
            !h.snippet.contains('\r'),
            "carriage return in snippet of {}",
            h.file
        );
        assert_eq!(
            (h.start_line, h.end_line),
            (lf.start_line, lf.end_line),
            "{}",
            h.file
        );
        assert_eq!(h.snippet, lf.snippet, "{}", h.file);
    }
    // Lines after the marker are still right.
    let res = search(&cs, "v118");
    for h in &res.matches {
        assert!(
            covers(h, 118),
            "{}: {}..{}",
            h.file,
            h.start_line,
            h.end_line
        );
    }
    assert_eq!(res.matches.len(), 4);
}

#[test]
fn a_utf8_bom_does_not_break_the_first_line() {
    let repo = Repo::new();
    repo.write_bytes(
        "enc/lib.rs",
        b"\xEF\xBB\xBFfn bomFirstLineMarker() {}\r\nfn second_line_fn() {}\r\n",
    );
    repo.write_bytes(
        "enc/plain.py",
        b"\xEF\xBB\xBF# comment\nprint('pythonbomword')\n",
    );
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.skipped, st.errors), (2, 0, 0), "{st:?}");
    for q in ["bomFirstLineMarker", "bom first line marker"] {
        let res = search(&cs, q);
        assert_eq!(names(&cs, &res), ["enc/lib.rs"], "{q}");
        assert!(covers(&res.matches[0], 1));
        assert!(res.matches[0].snippet.contains("bomFirstLineMarker"));
    }
    let res = search(&cs, "pythonbomword");
    assert_eq!(names(&cs, &res), ["enc/plain.py"]);
    assert!(covers(&res.matches[0], 2));
}

#[test]
fn binary_and_non_utf8_files_are_skipped_and_never_returned() {
    let repo = Repo::new();
    // UTF-16LE with a BOM (what some Windows editors write), Latin-1, a PNG
    // renamed to .txt, and a NUL in the first bytes.
    let mut utf16 = vec![0xFF, 0xFE];
    for u in "fn utfsixteenword() {}\n".encode_utf16() {
        utf16.extend(u.to_le_bytes());
    }
    repo.write_bytes("bin/utf16.rs", &utf16);
    repo.write_bytes(
        "bin/latin1.rs",
        b"fn latinonemarkword() { let s = \"caf\xe9\"; }\n",
    );
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(b" pngmarkword ".iter().cycle().take(500));
    repo.write_bytes("bin/image.txt", &png);
    repo.write_bytes("bin/nul.rs", b"fn nulmarkword() {}\0\0\0");
    repo.write("bin/good.rs", "fn goodmarkword() {}\n");
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.skipped, st.errors), (1, 4, 0), "{st:?}");
    for m in [
        "utfsixteenword",
        "latinonemarkword",
        "pngmarkword",
        "nulmarkword",
    ] {
        assert!(
            find(&cs, m).is_empty(),
            "{m} found in a file that is not text"
        );
    }
    assert_eq!(find(&cs, "goodmarkword"), ["bin/good.rs"]);
    // Turning a text file into binary drops it on the next build.
    let good = repo.write_bytes("bin/good.rs", b"\0\0\0 goodmarkword \0");
    set_mtime(&good, 3);
    let st = sync(&cs, &[]);
    assert_eq!(st.removed, 1, "{st:?}");
    assert!(find(&cs, "goodmarkword").is_empty());
}

#[test]
fn unicode_text_and_file_names() {
    let repo = Repo::new();
    repo.write("src/größe.rs", "fn berechne_größe(x: f64) -> f64 { x }\n");
    repo.write("日本語/テスト.py", "def 計算する(値):\n    return 値\n");
    repo.write("café/menu.rs", "fn naïve_résumé() {}\n");
    repo.write("emoji/😀.rs", "fn smile() { /* 😀 */ }\n");
    repo.write("greek/αβγ.js", "function Ωμέγα() { return 'Ελλάδα'; }\n");
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.errors), (5, 0), "{st:?}");
    let cases: &[(&str, &str)] = &[
        ("größe", "src/größe.rs"),
        ("Größe", "src/größe.rs"),
        ("BERECHNE_GRÖßE", "src/größe.rs"),
        ("berechne", "src/größe.rs"),
        ("計算する", "日本語/テスト.py"),
        ("テスト", "日本語/テスト.py"),
        ("日本語", "日本語/テスト.py"),
        ("naïve", "café/menu.rs"),
        ("NAÏVE", "café/menu.rs"),
        ("résumé", "café/menu.rs"),
        ("RÉSUMÉ", "café/menu.rs"),
        ("café", "café/menu.rs"),
        ("smile", "emoji/😀.rs"),
        ("ωμέγα", "greek/αβγ.js"),
        ("ΩΜΈΓΑ", "greek/αβγ.js"),
        ("ελλάδα", "greek/αβγ.js"),
        ("αβγ", "greek/αβγ.js"),
    ];
    for (q, file) in cases {
        assert_eq!(top1(&cs, q).as_deref(), Some(*file), "query {q:?}");
    }
    // Nothing but emoji or symbols is not a query.
    assert!(find(&cs, "😀").is_empty());
}

#[test]
fn unicode_normalization_variants_never_break_a_query() {
    let repo = Repo::new();
    let nfc = "r\u{e9}sum\u{e9}";
    let nfd = "re\u{301}sume\u{301}";
    repo.write(
        "n/precomposed.rs",
        &format!("fn {nfc}_precomposed_marker() {{}}\n"),
    );
    repo.write(
        "n/decomposed.rs",
        &format!("fn {nfd}_decomposed_marker() {{}}\n"),
    );
    repo.write(
        &format!("n/{nfc}_name.rs"),
        "fn composed_file_name_marker() {}\n",
    );
    repo.write(
        &format!("n/{nfd}_name2.rs"),
        "fn decomposed_file_name_marker() {}\n",
    );
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.errors), (4, 0), "{st:?}");
    // The exact spelling used in the file always finds it (the other spelling
    // finding it too would be nicer; see the report).
    assert!(find(&cs, nfc).contains(&"n/precomposed.rs".to_string()));
    assert!(find(&cs, nfd).contains(&"n/decomposed.rs".to_string()));
    for q in [
        nfc,
        nfd,
        "e\u{301}",
        "\u{301}",
        "\u{e9}\u{301}",
        &format!("{nfd}{nfc}{nfd}"),
    ] {
        let res = cs.search_blocking(q, 10).unwrap();
        check_results(&cs, &res, 10);
    }
}

#[cfg(unix)]
#[test]
fn awkward_root_and_file_names_work() {
    let base = tempfile::Builder::new()
        .prefix("code-search-spec-")
        .tempdir()
        .unwrap();
    let root = base.path().join("my proj [v2] {x} #1 'q' & co $HOME *?");
    let files = [
        "src dir/some file.rs",
        "a&b/c;d.rs",
        "100%/pct%20name.rs",
        "[brackets]/{braces}.rs",
        "star*/q?.rs",
        "tab\tname.rs",
        "new\nline.rs",
        "-leading dash.rs",
        "trailing space .rs",
        "dots...rs",
        "quote'd/\"double\".rs",
        "back\\slash.rs",
        "uni/ünï cödé.rs",
        "Makefile",
    ];
    let mut tokens = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let p = root.join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let token = format!("odd{}name", letters(i));
        fs::write(&p, format!("fn {token}() {{}}\n")).unwrap();
        tokens.push((token, p));
    }
    let cs = CodeSearch::new(root.clone());
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.errors), (files.len(), 0), "{st:?}");
    for (token, p) in &tokens {
        let res = search(&cs, token);
        assert_eq!(res.matches.len(), 1, "{token} {:?}", p);
        assert_eq!(Path::new(&res.matches[0].file), p.as_path());
    }
    // And the JSON of such paths round-trips.
    let v = serde_json::to_value(search(&cs, &tokens[6].0)).unwrap();
    let back = v["matches"][0]["file"].as_str().unwrap();
    assert_eq!(Path::new(back), tokens[6].1.as_path());
}

#[test]
fn very_deep_directories_are_walked() {
    let repo = Repo::new();
    let deep: String = (0..50).map(|i| format!("d{i}/")).collect();
    repo.write(&format!("{deep}deep.rs"), "fn verydeepmarker() {}\n");
    repo.write("shallow.rs", "fn shallowmarker() {}\n");
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.errors), (2, 0), "{st:?}");
    let res = search(&cs, "verydeepmarker");
    assert_eq!(res.matches.len(), 1);
    assert!(res.matches[0].file.ends_with("d49/deep.rs"));
}

#[test]
fn a_few_thousand_files_index_completely_and_each_is_findable() {
    let repo = Repo::new();
    let n = 3000;
    for i in 0..n {
        repo.write(
            &format!("pkg{}/mod{}/file{i}.rs", i % 17, i % 5),
            &format!("pub fn bulk_{}() {{ common_bulk_word(); }}\n", letters(i)),
        );
    }
    let cs = repo.cs();
    let st = within(120, "build of 3000 files", {
        let cs = cs.clone();
        move || sync(&cs, &[])
    });
    assert_eq!((st.indexed, st.errors, st.skipped), (n, 0, 0), "{st:?}");
    assert_eq!(st.chunks, n);
    let mut rng = Rng::new(3000);
    for _ in 0..60 {
        let i = rng.below(n);
        let got = find(&cs, &format!("bulk_{}", letters(i)));
        assert_eq!(
            got.first().map(String::as_str),
            Some(format!("pkg{}/mod{}/file{i}.rs", i % 17, i % 5)).as_deref()
        );
    }
    assert_eq!(search_k(&cs, "common_bulk_word", 20).matches.len(), 20);
}

// ═══════════════════ 4. what the walker indexes ═══════════════════════════════

/// Which of `files` (path, token) are found by searching their token.
fn found_set(cs: &CodeSearch, files: &[(String, String)]) -> BTreeSet<String> {
    let root = cs.root();
    files
        .iter()
        .filter(|(path, token)| {
            let res = search(cs, token);
            res.matches.iter().any(|h| rel(&root, &h.file) == *path)
        })
        .map(|(p, _)| p.clone())
        .collect()
}

/// Write one file per path, each holding its own unique word.
fn write_tokens(repo: &Repo, paths: &[&str]) -> Vec<(String, String)> {
    paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let token = format!("tok{}", letters(i + 100));
            repo.write(p, &format!("// {token}\n"));
            (p.to_string(), token)
        })
        .collect()
}

#[test]
fn the_old_sidecars_extensions_are_indexed_and_other_files_are_not() {
    let repo = Repo::new();
    let yes = [
        "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "cpp", "cc", "cxx", "h", "hpp",
        "md", "toml", "json", "yaml", "yml", "txt",
    ];
    let no = [
        "png", "jpg", "lock", "bin", "so", "o", "class", "exe", "pdf", "zip", "dll", "pyc", "wasm",
        "ico", "ttf",
    ];
    let mut want = Vec::new();
    let mut unwanted = Vec::new();
    for (i, e) in yes.iter().enumerate() {
        let token = format!("yes{}ext", letters(i));
        repo.write(&format!("exts/file.{e}"), &format!("{token}\n"));
        want.push((format!("exts/file.{e}"), token));
    }
    for (i, e) in no.iter().enumerate() {
        let token = format!("no{}ext", letters(i));
        repo.write(&format!("exts/file.{e}"), &format!("{token}\n"));
        unwanted.push((format!("exts/file.{e}"), token));
    }
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!(st.indexed, yes.len(), "{st:?}");
    assert_eq!(found_set(&cs, &want).len(), yes.len());
    assert!(found_set(&cs, &unwanted).is_empty());
}

#[test]
fn skip_directories_are_skipped_at_any_depth_but_only_by_exact_name() {
    let repo = Repo::new();
    let skipped = [
        ".git",
        "node_modules",
        "target",
        "__pycache__",
        "dist",
        "build",
        ".next",
        ".venv",
        "venv",
        "vendor",
    ];
    let mut never = Vec::new();
    let mut always = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    for d in skipped {
        paths.push(format!("{d}/top.js"));
        paths.push(format!("src/{d}/mid.js"));
        paths.push(format!("a/b/c/{d}/deep/nested.js"));
    }
    let keep = [
        "src/main.js",
        "builder/x.js",
        "distance/y.js",
        "targets/z.js",
        "my_vendor/w.js",
        "vendors/v.js",
        "node_modules_old/u.js",
        "venv2/t.js",
        "src/build_utils/s.js",
    ];
    let all: Vec<&str> = paths.iter().map(String::as_str).chain(keep).collect();
    for (p, t) in write_tokens(&repo, &all) {
        if keep.contains(&p.as_str()) {
            always.push((p, t));
        } else {
            never.push((p, t));
        }
    }
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!(st.indexed, keep.len(), "{st:?}");
    assert_eq!(found_set(&cs, &always).len(), keep.len());
    assert_eq!(found_set(&cs, &never), BTreeSet::new());
    // Not even when asked for by name.
    let st = sync(
        &cs,
        &[
            repo.p("node_modules"),
            repo.p("src/target"),
            repo.p("a/b/c/dist/deep/nested.js"),
        ],
    );
    assert_eq!(st.indexed, 0, "{st:?}");
    assert_eq!(found_set(&cs, &never), BTreeSet::new());
    for (p, _) in &never {
        cs.refresh_file(&repo.p(p));
    }
    assert_eq!(found_set(&cs, &never), BTreeSet::new());
}

#[test]
fn hidden_files_are_not_indexed() {
    let repo = Repo::new();
    let files = write_tokens(
        &repo,
        &[
            ".hidden.rs",
            "src/.secret_notes.md",
            "src/visible.rs",
            ".env.rs",
        ],
    );
    let cs = repo.cs();
    sync(&cs, &[]);
    let found = found_set(&cs, &files);
    assert_eq!(found, BTreeSet::from(["src/visible.rs".to_string()]));
}

#[test]
fn nested_gitignore_files_and_negations_are_honoured() {
    let repo = Repo::new();
    repo.write(".gitignore", "*.generated.rs\n/scratch/\nlogs.txt\n");
    repo.write("src/.gitignore", "local/\n*.tmp.rs\n");
    repo.write("src/deep/.gitignore", "!keep.generated.rs\n");
    let ignored = [
        "src/api.generated.rs",
        "src/deep/other.generated.rs",
        "scratch/notes.rs",
        "logs.txt",
        "src/logs.txt",
        "src/local/cache.rs",
        "src/x.tmp.rs",
        "src/deep/y.tmp.rs",
    ];
    let kept = [
        "src/main.rs",
        "src/deep/keep.generated.rs",
        "src/scratch/notes.rs",
        "local/other.rs",
        "y.tmp.rs",
        "docs/logs.md",
    ];
    let a = write_tokens(&repo, &ignored);
    let k: Vec<(String, String)> = kept
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let t = format!("keep{}tok", letters(i));
            repo.write(p, &format!("// {t}\n"));
            (p.to_string(), t)
        })
        .collect();
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!(st.indexed, kept.len(), "{st:?}");
    assert_eq!(found_set(&cs, &a), BTreeSet::new());
    assert_eq!(found_set(&cs, &k).len(), kept.len());
}

#[test]
fn gitignore_negations_that_git_supports() {
    let repo = Repo::new();
    repo.write(
        ".gitignore",
        "docs/*\n!docs/api.md\n*.txt\n!important.txt\n",
    );
    let files = write_tokens(
        &repo,
        &[
            "docs/api.md",
            "docs/guide.md",
            "docs/sub/x.md",
            "notes.txt",
            "important.txt",
            "sub/important.txt",
            "sub/other.txt",
            "README.md",
        ],
    );
    let cs = repo.cs();
    sync(&cs, &[]);
    let found = found_set(&cs, &files);
    let expect: BTreeSet<String> = [
        "docs/api.md",
        "important.txt",
        "sub/important.txt",
        "README.md",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(found, expect);
}

#[test]
fn dot_ignore_files_and_crlf_gitignores_work_without_a_git_repository() {
    let repo = Repo::new();
    repo.write(".ignore", "scratch.rs\n");
    // A .gitignore written on Windows.
    repo.write_bytes(".gitignore", b"ignored_dir/\r\n*.skip.rs\r\n");
    let files = write_tokens(
        &repo,
        &[
            "scratch.rs",
            "ignored_dir/a.rs",
            "x.skip.rs",
            "src/scratch.rs",
            "kept.rs",
            "kept_dir/b.rs",
        ],
    );
    let cs = repo.cs();
    sync(&cs, &[]);
    let found = found_set(&cs, &files);
    let expect: BTreeSet<String> = ["kept.rs", "kept_dir/b.rs"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(found, expect);
}

#[test]
fn editing_gitignore_between_builds_drops_and_restores_files() {
    let repo = Repo::new();
    let files = write_tokens(&repo, &["a.rs", "b.rs", "c.rs"]);
    let cs = repo.cs();
    assert_eq!(sync(&cs, &[]).total_files, 3);
    repo.write(".gitignore", "b.rs\n");
    let st = sync(&cs, &[]);
    assert_eq!(
        (st.removed, st.unchanged, st.total_files),
        (1, 2, 2),
        "{st:?}"
    );
    assert_eq!(found_set(&cs, &files).len(), 2);
    assert!(!found_set(&cs, &files).contains("b.rs"));
    repo.write(".gitignore", "a.rs\nb.rs\nc.rs\n");
    let st = sync(&cs, &[]);
    assert_eq!(st.total_files, 0, "{st:?}");
    fs::remove_file(repo.p(".gitignore")).unwrap();
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.total_files), (3, 3), "{st:?}");
    assert_eq!(found_set(&cs, &files).len(), 3);
}

#[test]
fn refresh_file_applies_the_same_rules_as_a_build() {
    let repo = Repo::new();
    repo.write(".gitignore", "ignored.rs\n");
    let elsewhere = Repo::new();
    let paths = [
        "ignored.rs",
        "node_modules/x.js",
        ".hidden.rs",
        "pic.png",
        "src/.dot.rs",
        "target/t.rs",
    ];
    let files = write_tokens(&repo, &paths);
    repo.write("ok.rs", "// refreshokword\n");
    let outside = elsewhere.write("outside.rs", "// outsiderefreshword\n");
    let cs = repo.cs();
    sync(&cs, &[]);
    for (p, _) in &files {
        cs.refresh_file(&repo.p(p));
    }
    cs.refresh_file(&outside);
    cs.refresh_file(&repo.root);
    cs.refresh_file(&repo.p("does/not/exist.rs"));
    cs.refresh_file(&repo.p("src"));
    assert_eq!(found_set(&cs, &files), BTreeSet::new());
    assert!(find(&cs, "outsiderefreshword").is_empty());
    assert_eq!(find(&cs, "refreshokword"), ["ok.rs"]);
    assert_eq!(cs.status().files, 1);
    // A new indexable file becomes searchable the moment it is refreshed.
    let fresh = repo.write("sub/new.rs", "// brandnewrefreshword\n");
    cs.refresh_file(&fresh);
    assert_eq!(find(&cs, "brandnewrefreshword"), ["sub/new.rs"]);
    // `..` and `.` segments name the same file; no second entry appears.
    cs.refresh_file(&repo.p("sub/../sub/./new.rs"));
    assert_eq!(cs.status().files, 2);
    assert_eq!(find(&cs, "brandnewrefreshword"), ["sub/new.rs"]);
    // ... and a lexical escape from the root is ignored.
    cs.refresh_file(&repo.p("sub/../../outside.rs"));
    assert_eq!(cs.status().files, 2);
}

#[test]
fn remove_file_drops_files_and_whole_directories_until_the_disk_is_looked_at_again() {
    let repo = Repo::new();
    let files = write_tokens(
        &repo,
        &[
            "keep/a.rs",
            "drop/b.rs",
            "drop/sub/c.rs",
            "dropper/d.rs",
            "e.rs",
        ],
    );
    let cs = repo.cs();
    sync(&cs, &[]);
    cs.remove_file(&repo.p("e.rs"));
    cs.remove_file(&repo.p("drop"));
    cs.remove_file(&repo.p("never/was/there.rs"));
    // `drop` must not take `dropper` with it (component-wise prefix): what is
    // left is `keep/a.rs` and `dropper/d.rs`. (Counted, not searched for: a
    // search looks at the disk too and would bring the files straight back.)
    assert_eq!(cs.status().files, 2);
    assert_eq!(cs.status().chunks, 2);
    // The disk is the truth: a build brings them back...
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.total_files), (3, 5), "{st:?}");
    // ...and so does the first search after that (no search has looked yet).
    cs.remove_file(&repo.p("drop"));
    assert_eq!(cs.status().files, 3);
    assert_eq!(found_set(&cs, &files).len(), 5);
    assert_eq!(cs.status().files, 5);
}

#[test]
fn directories_named_like_source_files_and_overlapping_paths() {
    let repo = Repo::new();
    repo.write("weird.rs/inner.rs", "// innerdirword\n");
    repo.write("src/main.rs", "// maindirword\n");
    repo.write("src/lib.rs", "// libdirword\n");
    let cs = repo.cs();
    // Overlapping, repeated and relative paths: each file once.
    let st = sync(
        &cs,
        &[
            repo.root.clone(),
            repo.p("src"),
            repo.p("src/main.rs"),
            PathBuf::from("src"),
            PathBuf::from("weird.rs"),
            repo.p("src"),
        ],
    );
    assert_eq!((st.indexed, st.total_files, st.errors), (3, 3, 0), "{st:?}");
    assert_eq!(find(&cs, "innerdirword"), ["weird.rs/inner.rs"]);
    let again = sync(&cs, std::slice::from_ref(&repo.root));
    assert_eq!((again.indexed, again.unchanged), (0, 3), "{again:?}");
}

#[test]
fn paths_outside_the_root_are_never_indexed() {
    let repo = Repo::new();
    let elsewhere = Repo::new();
    elsewhere.write("secret/leak.rs", "// leakoutsideword\n");
    elsewhere.write("top.rs", "// topoutsideword\n");
    repo.write("in.rs", "// insideword\n");
    let cs = repo.cs();
    sync(&cs, &[]);
    // Absolute, relative-with-.., and rooted-then-escaping paths.
    let sibling = elsewhere
        .root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let attempts = [
        elsewhere.root.clone(),
        elsewhere.p("top.rs"),
        PathBuf::from(format!("../{sibling}")),
        PathBuf::from(format!("../{sibling}/secret/leak.rs")),
        repo.p("sub/../../x"),
        PathBuf::from("/"),
        PathBuf::from("/etc"),
    ];
    for a in attempts {
        let st = cs.build_blocking(std::slice::from_ref(&a)).unwrap();
        assert_eq!(st.indexed, 0, "{a:?}: {st:?}");
        assert!(
            st.errors >= 1 || !st.warnings.is_empty(),
            "{a:?} was not reported: {st:?}"
        );
    }
    assert!(find(&cs, "leakoutsideword").is_empty());
    assert!(find(&cs, "topoutsideword").is_empty());
    assert_eq!(cs.status().files, 1);
}

#[test]
fn building_a_path_that_does_not_exist_is_reported_not_fatal() {
    let repo = Repo::new();
    repo.write("in.rs", "// existsword\n");
    let cs = repo.cs();
    sync(&cs, &[]);
    let st = cs
        .build_blocking(&[
            repo.p("nope"),
            repo.p("in.rs"),
            PathBuf::from("also/missing.rs"),
        ])
        .unwrap();
    assert!(st.errors >= 2, "{st:?}");
    assert_eq!(st.total_files, 1);
    assert_eq!(find(&cs, "existsword"), ["in.rs"]);
    // A path to a file that is not indexable is a no-op, not an error and not an entry.
    repo.write("pic.png", "// pngword\n");
    let st = cs.build_blocking(&[repo.p("pic.png")]).unwrap();
    assert_eq!((st.indexed, st.total_files), (0, 1), "{st:?}");
}

#[cfg(unix)]
#[test]
fn symlink_loops_and_links_leaving_the_root_are_never_followed() {
    use std::os::unix::fs::symlink;
    let outside = Repo::new();
    outside.write("secret/leak.rs", "fn leakoutsidemarker() {}\n");
    outside.write("top.rs", "fn topoutsidemarker() {}\n");
    let repo = Repo::new();
    repo.write("src/real.rs", "fn realinsidemarker() {}\n");
    fs::create_dir_all(repo.p("a/b")).unwrap();
    symlink(&repo.root, repo.p("loop")).unwrap();
    symlink("../..", repo.p("a/b/up")).unwrap();
    symlink("loop2", repo.p("loop1")).unwrap();
    symlink("loop1", repo.p("loop2")).unwrap();
    symlink(&outside.root, repo.p("escape")).unwrap();
    symlink(outside.p("top.rs"), repo.p("link.rs")).unwrap();
    symlink(repo.p("src/real.rs"), repo.p("alias.rs")).unwrap();
    symlink(repo.p("does-not-exist"), repo.p("dangling.rs")).unwrap();
    let cs = repo.cs();
    let st = within(60, "build with symlink loops", {
        let cs = cs.clone();
        move || cs.build_blocking(&[]).unwrap()
    });
    assert_eq!(st.indexed, 1, "{st:?}");
    assert_eq!(find(&cs, "realinsidemarker"), ["src/real.rs"]);
    assert!(find(&cs, "leakoutsidemarker").is_empty());
    assert!(find(&cs, "topoutsidemarker").is_empty());
    // Asking for the links directly, or refreshing paths that go through them, indexes nothing.
    for p in [
        "escape",
        "loop",
        "a/b/up",
        "link.rs",
        "alias.rs",
        "escape/top.rs",
        "loop/src/real.rs",
    ] {
        let st = cs.build_blocking(&[repo.p(p)]).unwrap();
        assert_eq!(st.indexed, 0, "{p}: {st:?}");
        cs.refresh_file(&repo.p(p));
    }
    assert_eq!(cs.status().files, 1);
    assert_eq!(find(&cs, "realinsidemarker"), ["src/real.rs"]);
    assert!(find(&cs, "leakoutsidemarker").is_empty());
}

#[cfg(unix)]
#[test]
fn a_root_that_is_itself_a_symlink_is_searched() {
    let real = Repo::new();
    real.write("src/a.rs", "fn viasymlinkrootmarker() {}\n");
    let holder = tempfile::tempdir().unwrap();
    let link = holder.path().join("link-root");
    std::os::unix::fs::symlink(&real.root, &link).unwrap();
    let cs = CodeSearch::new(link.clone());
    let st = sync(&cs, &[]);
    assert_eq!(
        (st.indexed, st.errors),
        (1, 0),
        "a symlinked workspace root indexed nothing: {st:?}"
    );
    let res = search(&cs, "viasymlinkrootmarker");
    assert_eq!(res.matches.len(), 1);
    assert!(
        Path::new(&res.matches[0].file).starts_with(&link),
        "{}",
        res.matches[0].file
    );
    // Editor saves arrive under the path the workspace was opened with.
    let p = real.write("src/b.rs", "fn secondsymlinkrootmarker() {}\n");
    cs.refresh_file(&link.join("src/b.rs"));
    assert_eq!(cs.status().files, 2);
    assert!(p.exists());
    assert_eq!(find(&cs, "secondsymlinkrootmarker"), ["src/b.rs"]);
}

#[cfg(unix)]
#[test]
fn a_fifo_with_a_source_name_does_not_hang_anything() {
    let repo = Repo::new();
    repo.write("ok.rs", "fn fifoneighbourmarker() {}\n");
    let fifo = repo.p("pipe.rs");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    if !made.is_ok_and(|s| s.success()) {
        eprintln!("mkfifo is not available; skipping");
        return;
    }
    let cs = repo.cs();
    let st = within(20, "build next to a fifo", {
        let cs = cs.clone();
        move || cs.build_blocking(&[]).unwrap()
    });
    assert_eq!(st.indexed, 1, "{st:?}");
    within(20, "refresh of a fifo", {
        let cs = cs.clone();
        let fifo = fifo.clone();
        move || cs.refresh_file(&fifo)
    });
    within(20, "build of a fifo path", {
        let cs = cs.clone();
        let fifo = fifo.clone();
        move || cs.build_blocking(&[fifo]).unwrap()
    });
    assert_eq!(find(&cs, "fifoneighbourmarker"), ["ok.rs"]);
}

#[test]
fn a_workspace_inside_a_hidden_directory_is_indexed() {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join(".hidden_parent").join(".proj");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "fn hiddenparentmarker() {}\n").unwrap();
    let cs = CodeSearch::new(root.clone());
    let st = sync(&cs, &[]);
    assert_eq!(st.indexed, 1, "{st:?}");
    assert_eq!(find(&cs, "hiddenparentmarker"), ["src/lib.rs"]);
}

#[test]
fn root_spelling_does_not_change_what_is_found() {
    let repo = Repo::new();
    repo.write("sub/a.rs", "fn rootspellingmarker() {}\n");
    let spelled = repo.root.join("sub").join("..").join(".");
    for root in [repo.root.clone(), spelled, repo.root.join("")] {
        let cs = CodeSearch::new(root);
        assert!(cs.root().is_absolute());
        assert!(
            !cs.root().components().any(|c| matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )),
            "{:?}",
            cs.root()
        );
        sync(&cs, &[]);
        let res = search(&cs, "rootspellingmarker");
        assert_eq!(res.matches.len(), 1);
        assert_eq!(Path::new(&res.matches[0].file), repo.p("sub/a.rs"));
    }
}

// ═════════════ 5. files that change under the index, root changes ═══════════

/// Three small files, indexed.
fn change_repo() -> (Repo, CodeSearch) {
    let repo = Repo::new();
    repo.write(
        "a.rs",
        "fn alphaoriginalword() {}\nfn alphasharedword() {}\n",
    );
    repo.write("b.rs", "fn betaoriginalword() {}\n");
    repo.write("c.rs", "fn gammaoriginalword() {}\n");
    let cs = repo.cs();
    assert_eq!(sync(&cs, &[]).indexed, 3);
    (repo, cs)
}

#[test]
fn an_edit_is_seen_after_the_editor_saves_it() {
    let (repo, cs) = change_repo();
    let a = repo.write("a.rs", "// a new first line\n\n\nfn alphaeditedword() {}\n");
    // The text that was replaced must not be served, even before anything refreshes the file.
    assert!(
        find(&cs, "alphaoriginalword").is_empty(),
        "the old text of an edited file must not match"
    );
    assert!(find(&cs, "alphasharedword").is_empty());
    assert_eq!(find(&cs, "betaoriginalword"), ["b.rs"]);
    cs.refresh_file(&a);
    let res = search(&cs, "alphaeditedword");
    assert_eq!(names(&cs, &res), ["a.rs"]);
    assert!(covers(&res.matches[0], 4), "{:?}", res.matches[0]);
    assert_eq!(cs.status().files, 3);
}

#[test]
fn a_same_size_edit_with_a_new_timestamp_is_seen() {
    let (repo, cs) = change_repo();
    let old = "fn betaoriginalword() {}\n";
    let new = old.replace("betaoriginalword", "betaswappedwordx");
    assert_eq!(old.len(), new.len());
    let b = repo.write("b.rs", &new);
    set_mtime(&b, 77);
    assert!(find(&cs, "betaoriginalword").is_empty());
    cs.refresh_file(&b);
    assert_eq!(find(&cs, "betaswappedwordx"), ["b.rs"]);
    // and a build agrees
    let st = sync(&cs, &[]);
    assert_eq!(
        (st.total_files, st.indexed, st.updated),
        (3, 0, 0),
        "{st:?}"
    );
    assert_eq!(find(&cs, "betaswappedwordx"), ["b.rs"]);
    // A build alone (no refresh) picks up a second same-size edit.
    let again = new.replace("betaswappedwordx", "betaswappedwordy");
    let b = repo.write("b.rs", &again);
    set_mtime(&b, 78);
    let st = sync(&cs, &[]);
    assert_eq!(st.updated, 1, "{st:?}");
    assert_eq!(find(&cs, "betaswappedwordy"), ["b.rs"]);
    assert!(find(&cs, "betaswappedwordx").is_empty());
    // A timestamp that moves backwards (a restore, `touch -d`) is a change too.
    let older = again.replace("betaswappedwordy", "betaswappedwordz");
    let b = repo.write("b.rs", &older);
    set_mtime(&b, 5);
    let st = sync(&cs, &[]);
    assert_eq!(st.updated, 1, "{st:?}");
    assert_eq!(find(&cs, "betaswappedwordz"), ["b.rs"]);
}

// code_search's tool description promises "code_search builds the index on first use
// and refreshes files it finds out of date, so build_search_index is only needed after
// many files changed at once". Re-reading the files that are already *hits* is not
// enough: a word that an edit added to an indexed file is in no posting list yet, so an
// agent that edits a file with the file tools and then searches for the symbol it just
// wrote would get nothing back. Searches therefore look at the disk too.
#[test]
fn a_word_added_by_an_edit_is_found_by_the_next_search_without_a_manual_refresh() {
    let (repo, cs) = change_repo();
    repo.write(
        "a.rs",
        "// a new first line\n\n\nfn alphaaddedbyeditword() {}\n",
    );
    let res = search(&cs, "alphaaddedbyeditword");
    assert_eq!(
        names(&cs, &res),
        ["a.rs"],
        "an edit added a word to an indexed file; a search for it finds nothing"
    );
}

// Same, for files that did not exist when the index was built (an agent that writes a
// new file and then searches for what it wrote).
#[test]
fn a_new_file_is_found_by_the_next_search_without_a_manual_refresh() {
    let (repo, cs) = change_repo();
    repo.write("src/brandnew.rs", "fn zetabrandnewfileword() {}\n");
    let res = search(&cs, "zetabrandnewfileword");
    assert_eq!(
        names(&cs, &res),
        ["src/brandnew.rs"],
        "a file created after the build is not searchable"
    );
}

#[test]
fn a_deleted_file_disappears_without_an_error() {
    let (repo, cs) = change_repo();
    repo.remove("c.rs");
    assert!(find(&cs, "gammaoriginalword").is_empty());
    assert_eq!(find(&cs, "alphaoriginalword"), ["a.rs"]);
    let st = sync(&cs, &[]);
    assert_eq!(st.total_files, 2, "{st:?}");
    // Delete everything the index knows about: searches still answer.
    repo.remove("a.rs");
    repo.remove("b.rs");
    assert!(find(&cs, "alphaoriginalword").is_empty());
    assert!(find(&cs, "betaoriginalword").is_empty());
    let st = sync(&cs, &[]);
    assert_eq!((st.total_files, st.chunks), (0, 0), "{st:?}");
}

#[test]
fn line_numbers_follow_a_file_that_grew_or_shrank() {
    let repo = Repo::new();
    let body = |n: usize| -> String { (0..n).map(|i| format!("let filler{i} = {i};\n")).collect() };
    repo.write(
        "shift.rs",
        &(body(90) + "fn shiftytarget() {}\n" + &body(10)),
    );
    let cs = repo.cs();
    let res = search(&cs, "shiftytarget");
    assert!(covers(&res.matches[0], 91));
    // 30 lines are inserted above the marker.
    repo.write(
        "shift.rs",
        &(body(30) + &body(90) + "fn shiftytarget() {}\n" + &body(10)),
    );
    let res = search(&cs, "shiftytarget");
    assert_eq!(res.matches.len(), 1);
    assert!(covers(&res.matches[0], 121), "{:?}", res.matches[0]);
    // The file shrinks to fewer lines than the old hit's range.
    repo.write("shift.rs", "fn shiftytarget() {}\nlet other = 1;\n");
    let res = search(&cs, "shiftytarget");
    assert_eq!(res.matches.len(), 1);
    assert!(covers(&res.matches[0], 1), "{:?}", res.matches[0]);
    // It becomes empty.
    repo.write("shift.rs", "");
    assert!(find(&cs, "shiftytarget").is_empty());
}

#[test]
fn a_file_replaced_by_binary_data_or_a_directory_leaves_the_results() {
    let (repo, cs) = change_repo();
    repo.write_bytes(
        "a.rs",
        b"\0\x01\x02 alphaoriginalword \0\0\0 and some more bytes",
    );
    assert!(find(&cs, "alphaoriginalword").is_empty());
    repo.remove("b.rs");
    repo.write("b.rs/inner.rs", "fn betainnerword() {}\n");
    assert!(find(&cs, "betaoriginalword").is_empty());
    let st = sync(&cs, &[]);
    assert_eq!(find(&cs, "betainnerword"), ["b.rs/inner.rs"]);
    assert_eq!(st.total_files, 2, "c.rs and b.rs/inner.rs: {st:?}");
    // And back: the directory goes, a file comes.
    repo.remove("b.rs");
    repo.write("b.rs", "fn betareturnedword() {}\n");
    sync(&cs, &[]);
    assert_eq!(find(&cs, "betareturnedword"), ["b.rs"]);
    assert!(find(&cs, "betainnerword").is_empty());
}

#[test]
fn a_new_file_is_found_after_refresh_file_or_a_build() {
    let (repo, cs) = change_repo();
    let d = repo.write("d.rs", "fn deltabrandnewword() {}\n");
    cs.refresh_file(&d);
    assert_eq!(find(&cs, "deltabrandnewword"), ["d.rs"]);
    repo.write("e.rs", "fn epsilonbrandnewword() {}\n");
    sync(&cs, &[]);
    assert_eq!(find(&cs, "epsilonbrandnewword"), ["e.rs"]);
    assert_eq!(cs.status().files, 5);
}

#[test]
fn a_renamed_directory_moves_its_files_in_the_index() {
    let repo = Repo::new();
    for i in 0..6 {
        repo.write(
            &format!("old/f{i}.rs"),
            &format!("fn renamed{}word() {{}}\n", letters(i)),
        );
    }
    let cs = repo.cs();
    sync(&cs, &[]);
    fs::rename(repo.p("old"), repo.p("new")).unwrap();
    let st = sync(&cs, &[]);
    assert_eq!(
        (st.indexed, st.removed, st.total_files),
        (6, 6, 6),
        "{st:?}"
    );
    for i in 0..6 {
        let got = find(&cs, &format!("renamed{}word", letters(i)));
        assert_eq!(got, [format!("new/f{i}.rs")]);
    }
}

#[test]
fn deleting_the_whole_root_is_survivable_and_the_index_recovers() {
    let (repo, cs) = change_repo();
    fs::remove_dir_all(&repo.root).unwrap();
    match cs.search_blocking("alphaoriginalword", 5) {
        Ok(res) => assert!(res.matches.is_empty(), "{res:?}"),
        Err(SearchError::Io(_)) => {}
        Err(e) => panic!("unexpected error {e}"),
    }
    match cs.build_blocking(&[]) {
        Ok(st) => assert_eq!(st.total_files, 0, "{st:?}"),
        Err(SearchError::Io(_)) => {}
        Err(e) => panic!("unexpected error {e}"),
    }
    let _ = cs.status();
    cs.refresh_file(&repo.p("a.rs"));
    cs.remove_file(&repo.p("a.rs"));
    // The directory comes back with different content.
    repo.write("fresh.rs", "fn phoenixrebornword() {}\n");
    let st = sync(&cs, &[]);
    assert_eq!((st.total_files, st.indexed), (1, 1), "{st:?}");
    assert_eq!(find(&cs, "phoenixrebornword"), ["fresh.rs"]);
    for old in ["alphaoriginalword", "betaoriginalword", "gammaoriginalword"] {
        assert!(
            find(&cs, old).is_empty(),
            "{old} survived the deletion of the root"
        );
    }
}

#[test]
fn a_root_that_never_existed_is_an_error_not_a_panic() {
    let repo = Repo::new();
    let cs = CodeSearch::new(repo.p("no/such/dir"));
    assert!(matches!(cs.build_blocking(&[]), Err(SearchError::Io(_))));
    assert!(matches!(
        cs.search_blocking("anything", 5),
        Err(SearchError::Io(_))
    ));
    let st = cs.status();
    assert!(!st.built && !st.building && st.files == 0, "{st:?}");
    // Creating it makes the very same handle work.
    repo.write("no/such/dir/x.rs", "fn lateappearingword() {}\n");
    assert_eq!(find(&cs, "lateappearingword"), ["x.rs"]);
}

#[test]
fn reset_root_switches_projects_and_forgets_the_old_one() {
    let a = Repo::new();
    let b = Repo::new();
    a.write(
        "a1.rs",
        "fn projectaonlyword() {}\nfn sharedprojectword() {}\n",
    );
    a.write("a2.rs", "fn projectaotherword() {}\n");
    b.write(
        "b1.rs",
        "fn projectbonlyword() {}\nfn sharedprojectword() {}\n",
    );
    let cs = a.cs();
    assert_eq!(find(&cs, "projectaonlyword"), ["a1.rs"]);
    assert_eq!(cs.root(), a.root);
    let seen_by_clone = cs.clone();

    cs.reset_root(b.root.clone());
    assert_eq!(cs.root(), b.root);
    assert_eq!(seen_by_clone.root(), b.root, "clones share the workspace");
    assert_eq!(cs.status(), IndexStatus::default());
    assert!(find(&cs, "projectaonlyword").is_empty());
    assert_eq!(find(&cs, "projectbonlyword"), ["b1.rs"]);
    let res = search(&cs, "sharedprojectword");
    assert_eq!(res.matches.len(), 1);
    assert!(Path::new(&res.matches[0].file).starts_with(&b.root));
    assert_eq!(cs.status().files, 1);

    cs.reset_root(a.root.clone());
    assert_eq!(cs.status().files, 0);
    assert!(find(&cs, "projectbonlyword").is_empty());
    assert_eq!(find(&cs, "projectaotherword"), ["a2.rs"]);

    // Resetting to the same root rebuilds from nothing.
    cs.reset_root(a.root.clone());
    let st = sync(&cs, &[]);
    assert_eq!((st.indexed, st.unchanged), (2, 0), "{st:?}");

    // Resetting to a missing directory is an error on use, and resetting back heals it.
    cs.reset_root(a.p("missing"));
    assert!(matches!(
        cs.search_blocking("projectaotherword", 5),
        Err(SearchError::Io(_))
    ));
    cs.reset_root(a.root.clone());
    assert_eq!(find(&cs, "projectaotherword"), ["a2.rs"]);
}

#[test]
fn reset_root_during_a_build_never_mixes_projects() {
    let a = Repo::new();
    let b = Repo::new();
    for i in 0..1500 {
        a.write(
            &format!("d{}/a{i}.rs", i % 30),
            &format!("fn fromprojecta{}() {{}}\n", letters(i)),
        );
    }
    for i in 0..40 {
        b.write(
            &format!("b{i}.rs"),
            &format!("fn fromprojectb{}() {{}}\n", letters(i)),
        );
    }
    let cs = a.cs();
    for round in 0..12 {
        cs.reset_root(a.root.clone());
        let builder = {
            let cs = cs.clone();
            std::thread::spawn(move || cs.build_blocking(&[]))
        };
        std::thread::sleep(Duration::from_millis((round % 4) * 3));
        cs.reset_root(b.root.clone());
        match builder.join().unwrap() {
            Ok(_) | Err(SearchError::RootChanged) => {}
            Err(e) => panic!("round {round}: unexpected {e}"),
        }
        let st = sync(&cs, &[]);
        assert_eq!(
            st.total_files, 40,
            "round {round}: files of the old root leaked into the new index: {st:?}"
        );
        for i in [0, 1, 2, 700, 1499] {
            let word = format!("fromprojecta{}", letters(i));
            assert!(
                find(&cs, &word).is_empty(),
                "round {round}: {word} found after the switch"
            );
        }
        let res = search(&cs, "fromprojectba");
        assert_eq!(res.matches.len(), 1, "round {round}");
        assert!(
            Path::new(&res.matches[0].file).starts_with(&b.root),
            "round {round}: {}",
            res.matches[0].file
        );
    }
}

// ══════════════════ 6. repeated builds, queries, top_k ═══════════════════════

const MINI_QUERIES: &[&str] = &[
    "json config",
    "tcp listener",
    "verify jwt",
    "dashboard chart",
    "debounce",
    "rotate backups",
    "handle webhook",
    "invoice tax",
    "matrix multiply",
    "scheduler worker",
    "serde",
    "server",
    "settings port",
    "fetch user profile",
    "readme",
    "use",
    "fn",
    "config",
];

fn answers(cs: &CodeSearch, queries: &[&str]) -> Vec<Value> {
    queries
        .iter()
        .map(|q| serde_json::to_value(search(cs, q)).unwrap())
        .collect()
}

#[test]
fn repeated_builds_change_nothing() {
    let (repo, cs) = mini_repo();
    let n = MINI_REPO.len();
    let first = sync(&cs, &[]);
    assert_eq!(
        (first.indexed, first.unchanged, first.updated, first.removed),
        (n, 0, 0, 0),
        "{first:?}"
    );
    assert_eq!(
        (
            first.skipped,
            first.errors,
            first.privacy_skipped,
            first.truncated
        ),
        (0, 0, 0, false)
    );
    assert!(first.chunks >= n);
    let before = answers(&cs, MINI_QUERIES);
    assert!(before
        .iter()
        .any(|v| !v["matches"].as_array().unwrap().is_empty()));
    for round in 0..3 {
        let again = sync(&cs, &[]);
        assert_eq!(
            (
                again.indexed,
                again.updated,
                again.removed,
                again.unchanged,
                again.total_files,
                again.chunks
            ),
            (0, 0, 0, n, n, first.chunks),
            "round {round}: {again:?}"
        );
        assert_eq!(
            answers(&cs, MINI_QUERIES),
            before,
            "round {round}: results changed"
        );
    }
    // Spelling the same scope differently is the same build.
    for paths in [
        vec![repo.root.clone()],
        vec![repo.p("src")],
        vec![PathBuf::from("src"), PathBuf::from("web")],
        vec![repo.p("src/main.rs")],
        vec![
            repo.p("src"),
            repo.p("src/network/server.rs"),
            repo.p("src"),
        ],
    ] {
        let st = sync(&cs, &paths);
        assert_eq!(
            (
                st.indexed,
                st.updated,
                st.removed,
                st.total_files,
                st.chunks
            ),
            (0, 0, 0, n, first.chunks),
            "{paths:?}: {st:?}"
        );
    }
    assert_eq!(answers(&cs, MINI_QUERIES), before);
    // A brand-new engine over the same tree answers identically.
    let fresh = repo.cs();
    assert_eq!(answers(&fresh, MINI_QUERIES), before);
    assert_eq!(fresh.status().files, n);
}

#[test]
fn touching_a_file_updates_it_in_place_without_duplicating_it() {
    let (repo, cs) = mini_repo();
    let n = MINI_REPO.len();
    let first = sync(&cs, &[]);
    let before = answers(&cs, MINI_QUERIES);
    set_mtime(&repo.p("src/main.rs"), 123);
    set_mtime(&repo.p("web/app.js"), 124);
    let st = sync(&cs, &[]);
    assert_eq!(
        (
            st.indexed,
            st.updated,
            st.removed,
            st.unchanged,
            st.total_files,
            st.chunks
        ),
        (0, 2, 0, n - 2, n, first.chunks),
        "{st:?}"
    );
    assert_eq!(answers(&cs, MINI_QUERIES), before);
    cs.refresh_file(&repo.p("src/main.rs"));
    cs.refresh_file(&repo.p("src/main.rs"));
    assert_eq!(cs.status().files, n);
    assert_eq!(cs.status().chunks, first.chunks);
    assert_eq!(answers(&cs, MINI_QUERIES), before);
}

#[test]
fn clones_share_one_index() {
    let (_repo, cs) = mini_repo();
    let other = cs.clone();
    assert_eq!(other.status(), cs.status());
    other.build_blocking(&[]).unwrap();
    let st = cs.status();
    assert!(st.built && st.files == MINI_REPO.len(), "{st:?}");
    assert_eq!(answers(&cs, MINI_QUERIES), answers(&other, MINI_QUERIES));
}

#[test]
fn queries_without_words_return_nothing_and_never_everything() {
    let (_repo, cs) = mini_repo();
    for q in [
        "",
        " ",
        "\t\n  \r\n",
        "!!!",
        "()",
        ".*",
        "***",
        "*",
        "[]",
        "\\",
        "\\\\",
        "a",
        "?",
        "the",
        "of the and",
        "-",
        "--",
        "::",
        "=>",
        "<>",
        "/",
        "\u{0}",
        "\u{1b}[31m",
        "😀",
        "...",
        "$^",
        "|||",
        "~",
        "@",
        "#",
        "%",
        "&&",
        "<<<",
        "'",
        "\"",
        "`",
        "a b c d e",
        "\u{a0}\u{2003}\u{200b}",
    ] {
        let res = cs
            .search_blocking(q, 10)
            .unwrap_or_else(|e| panic!("query {q:?} failed: {e}"));
        assert!(
            res.matches.is_empty(),
            "{q:?} matched {:?}",
            names(&cs, &res)
        );
        assert_eq!(res.privacy_skipped, 0, "{q:?}");
    }
}

#[test]
fn regex_and_glob_syntax_in_a_query_is_only_punctuation() {
    let (repo, cs) = mini_repo();
    let top = |q: &str, n: usize| -> Vec<String> {
        let res = search_k(&cs, q, 10);
        names(&cs, &res).into_iter().take(n).collect()
    };
    for (q, expect, within) in [
        ("fn main()", "src/main.rs", 3),
        ("json.*config", "src/config/loader.rs", 1),
        ("(?i)json", "src/config/loader.rs", 3),
        ("^json|config$", "src/config/loader.rs", 3),
        ("{json,config}", "src/config/loader.rs", 1),
        ("json\\", "src/config/loader.rs", 3),
        ("[a-z]+ matrix", "native/vec.c", 1),
        ("start_server(port: u16)", "src/network/server.rs", 2),
        ("\u{0}json", "src/config/loader.rs", 3),
        ("**/matrix_multiply*", "native/vec.c", 1),
        ("matrix.multiply", "native/vec.c", 1),
        ("matrix\\s+multiply", "native/vec.c", 3),
    ] {
        let got = top(q, within);
        assert!(
            got.iter().any(|f| f == expect),
            "{q:?}: expected {expect} in the top {within}, got {got:?}"
        );
    }
    // Hostile-looking text is still just words.
    for q in [
        "'; DROP TABLE users; --",
        "<script>alert(1)</script>",
        "../../etc/passwd",
        "$(rm -rf /)",
        "`id`",
        "%s%s%s%s",
        "{{7*7}}",
        "src/**/*.rs",
        "*.rs",
        "(((((((((((((((((((((((((((((",
        "a**b",
        "[[[json",
    ] {
        let res = cs
            .search_blocking(q, 10)
            .unwrap_or_else(|e| panic!("{q:?}: {e}"));
        check_results(&cs, &res, 10);
    }
    assert!(
        repo.p("src/main.rs").exists(),
        "no query may touch the files"
    );
}

#[test]
fn very_long_queries() {
    let (_repo, cs) = mini_repo();
    // ~10 KB of repeated words.
    let ten_kb = "parse_json_config ".repeat(600);
    assert!(ten_kb.len() > 10_000);
    let res = search_k(&cs, &ten_kb, 3);
    assert_eq!(
        names(&cs, &res).first().map(String::as_str),
        Some("src/config/loader.rs")
    );
    // One 10 KB token is junk, not a crash.
    assert!(search(&cs, &"x".repeat(10_000)).matches.is_empty());
    assert!(search(&cs, &"Ab".repeat(5_000)).matches.is_empty());
    // Far more than 64 distinct words, the useful one among them.
    let many: String = (0..300)
        .map(|i| format!("zzfiller{} ", letters(i)))
        .collect::<String>()
        + "matrix";
    let res = cs.search_blocking(&many, 5).unwrap();
    check_results(&cs, &res, 5);
    let many_first: String = "matrix ".to_string()
        + &(0..300)
            .map(|i| format!("zzfiller{} ", letters(i)))
            .collect::<String>();
    let res = search_k(&cs, &many_first, 5);
    assert_eq!(
        names(&cs, &res).first().map(String::as_str),
        Some("native/vec.c")
    );
    // Right at the documented limit of 16 KiB, and one byte past it.
    let at_limit = format!("{}{}", "json ", " ".repeat(16 * 1024 - 5));
    assert_eq!(at_limit.len(), 16 * 1024);
    assert!(cs.search_blocking(&at_limit, 3).is_ok());
    let over = format!("{at_limit} ");
    assert!(matches!(
        cs.search_blocking(&over, 3),
        Err(SearchError::InvalidQuery(_))
    ));
    assert!(matches!(
        cs.search_blocking(&"json ".repeat(10_000), 3),
        Err(SearchError::InvalidQuery(_))
    ));
}

#[test]
fn top_k_zero_one_and_more_than_there_are() {
    let (_repo, cs) = mini_repo();
    let zero = cs.search_blocking("server", 0).unwrap();
    assert!(zero.matches.is_empty());
    let all = search_k(&cs, "server", 100);
    assert!(
        all.matches.len() >= 2 && all.matches.len() < MINI_REPO.len(),
        "{:?}",
        names(&cs, &all)
    );
    let one = search_k(&cs, "server", 1);
    assert_eq!(one.matches.len(), 1);
    assert_eq!(one.matches[0].file, all.matches[0].file);
    for k in 1..=all.matches.len() {
        let res = search_k(&cs, "server", k);
        assert_eq!(
            names(&cs, &res),
            names(&cs, &all)[..k],
            "top_k {k} is not a prefix of the full ranking"
        );
    }
    // Nothing matches: an empty list, and the JSON still has the key.
    let none = search(&cs, "qqqzzznothingmatchesthis");
    assert!(none.matches.is_empty());
    assert_eq!(serde_json::to_value(&none).unwrap(), json!({"matches": []}));
}

#[test]
fn equal_scores_are_ordered_by_path_every_time() {
    let repo = Repo::new();
    for d in [
        "d07", "d03", "d09", "d01", "d05", "d02", "d08", "d04", "d06",
    ] {
        repo.write(&format!("{d}/same.rs"), "fn identicalcontentword() {}\n");
    }
    let cs = repo.cs();
    let expect: Vec<String> = (1..=9).map(|i| format!("d0{i}/same.rs")).collect();
    for _ in 0..3 {
        assert_eq!(find(&cs, "identicalcontentword"), expect);
    }
    assert_eq!(
        names(&cs, &search_k(&cs, "identicalcontentword", 4)),
        expect[..4]
    );
    let fresh = repo.cs();
    assert_eq!(find(&fresh, "identicalcontentword"), expect);
}

#[test]
fn scores_serialize_with_at_most_four_decimals() {
    let (_repo, cs) = mini_repo();
    for q in MINI_QUERIES {
        let v = serde_json::to_value(search(&cs, q)).unwrap();
        for m in v["matches"].as_array().unwrap() {
            let text = m["score"].to_string();
            let decimals = text.split('.').nth(1).map_or(0, str::len);
            assert!(decimals <= 4, "{q}: score {text}");
            assert!(m["score"].is_number());
        }
        let obj = v.as_object().unwrap();
        assert!(
            !obj.contains_key("privacy_skipped") && !obj.contains_key("index_truncated"),
            "{v}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_async_api_matches_the_blocking_one() {
    let (_repo, cs) = mini_repo();
    let st = cs.build(&[]).await.unwrap();
    assert_eq!(st.indexed, MINI_REPO.len());
    for q in MINI_QUERIES {
        let a = cs.search(q, 7).await.unwrap();
        let b = cs.search_blocking(q, 7).unwrap();
        assert_eq!(a, b, "{q}");
    }
    // Many tasks at once, mixing builds and searches.
    let mut tasks = Vec::new();
    for i in 0..16 {
        let cs = cs.clone();
        tasks.push(tokio::spawn(async move {
            if i % 4 == 0 {
                cs.build(&[]).await.map(|_| ()).unwrap();
            }
            let res = cs.search("json config", 3).await.unwrap();
            res.matches[0].file.clone()
        }));
    }
    for t in tasks {
        assert!(t.await.unwrap().ends_with("loader.rs"));
    }
    assert!(matches!(
        cs.search(&"json ".repeat(10_000), 3).await,
        Err(SearchError::InvalidQuery(_))
    ));
}

#[test]
fn the_public_types_can_be_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync + Clone + std::fmt::Debug>() {}
    assert_send_sync::<CodeSearch>();
    fn assert_value<T: Send + Sync + Clone + std::fmt::Debug + serde::Serialize>() {}
    assert_value::<BuildStats>();
    assert_value::<SearchResults>();
    assert_value::<SearchHit>();
    assert_value::<IndexStatus>();
    fn assert_error<T: std::error::Error + Send + Sync + 'static>() {}
    assert_error::<SearchError>();
}

// ═══════════════════════════ 7. concurrency ══════════════════════════════════

/// The words of `bytes` that look like a stamp written by the randomized tests.
fn stamps_in(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| {
            w.len() > 5 && w.starts_with("stamp") && w[5..].bytes().all(|b| b.is_ascii_lowercase())
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_search_on_an_unbuilt_index_waits_for_the_complete_index() {
    let repo = Repo::new();
    for i in 0..1500 {
        repo.write(
            &format!("bulk/d{}/b{i:04}.rs", i % 25),
            &format!("fn bulk{}() {{ commonneedle(); }}\n", letters(i)),
        );
    }
    // The best file sorts last by path: an index that is only half built would miss it.
    repo.write(
        "zz/zz_best.rs",
        "commonneedle commonneedle commonneedle fn winnermarkword\n",
    );
    let cs = repo.cs();
    let threads = 12;
    let barrier = Arc::new(std::sync::Barrier::new(threads));
    let results = within(120, "twelve searches on an unbuilt index", {
        let cs = cs.clone();
        move || {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    let (cs, barrier) = (cs.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        let a = cs.search_blocking("commonneedle", 5).unwrap();
                        let b = cs.search_blocking("winnermarkword", 5).unwrap();
                        (a, b)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        }
    });
    for (a, b) in &results {
        assert!(
            a.matches[0].file.ends_with("zz_best.rs"),
            "a search saw an incomplete index: {:?}",
            a.matches[0]
        );
        assert_eq!(b.matches.len(), 1);
    }
    let st = cs.status();
    assert!(st.built && !st.building && st.files == 1501, "{st:?}");
}

#[test]
fn concurrent_builds_are_single_flight_and_leave_no_duplicates() {
    let repo = Repo::new();
    for i in 0..800 {
        repo.write(
            &format!("p{}/f{i}.rs", i % 20),
            &format!("fn concurrentbuild{}() {{}}\n", letters(i)),
        );
    }
    let cs = repo.cs();
    let threads = 8;
    let barrier = Arc::new(std::sync::Barrier::new(threads));
    let stats: Vec<BuildStats> = within(120, "eight concurrent builds", {
        let cs = cs.clone();
        move || {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    let (cs, barrier) = (cs.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        cs.build_blocking(&[]).unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        }
    });
    assert_eq!(
        stats.iter().map(|s| s.indexed).sum::<usize>(),
        800,
        "every file is indexed exactly once: {stats:?}"
    );
    for st in &stats {
        assert_eq!(st.total_files, 800, "{st:?}");
        assert_eq!(st.indexed + st.unchanged, 800, "{st:?}");
        assert_eq!((st.updated, st.removed, st.errors), (0, 0, 0), "{st:?}");
    }
    let fresh = repo.cs();
    let fst = fresh.build_blocking(&[]).unwrap();
    assert_eq!(cs.status().chunks, fst.chunks, "chunks were indexed twice");
    assert_eq!(cs.status().files, 800);
}

#[test]
fn status_never_blocks_and_never_shows_a_half_published_build() {
    let repo = Repo::new();
    let n = 3000;
    for i in 0..n {
        repo.write(
            &format!("s{}/f{i}.rs", i % 30),
            &format!("fn statusprobe{}() {{}}\n", letters(i)),
        );
    }
    let cs = repo.cs();
    let builder = {
        let cs = cs.clone();
        std::thread::spawn(move || cs.build_blocking(&[]).unwrap())
    };
    let started = Instant::now();
    let (mut worst, mut polls, mut saw_building) = (Duration::ZERO, 0u64, false);
    let (mut last_files, mut last_chunks) = (0, 0);
    let mut built_snapshots = Vec::new();
    while !builder.is_finished() {
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "the build did not finish"
        );
        let t = Instant::now();
        let st = cs.status();
        worst = worst.max(t.elapsed());
        polls += 1;
        saw_building |= st.building;
        assert!(
            st.files >= last_files && st.chunks >= last_chunks,
            "the index shrank during its first build: {st:?}"
        );
        (last_files, last_chunks) = (st.files, st.chunks);
        if st.built {
            built_snapshots.push(st);
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    let stats = builder.join().unwrap();
    assert!(
        worst < Duration::from_secs(2),
        "status() took {worst:?} during a build ({polls} polls)"
    );
    for st in built_snapshots {
        assert_eq!(
            (st.files, st.chunks),
            (n, stats.chunks),
            "built=true was published before the index was complete: {st:?}"
        );
    }
    let st = cs.status();
    assert_eq!(
        st,
        IndexStatus {
            built: true,
            building: false,
            files: n,
            chunks: stats.chunks,
            truncated: false
        }
    );
    eprintln!(
        "status(): {polls} polls during a build, worst {worst:?}, saw building={saw_building}"
    );
}

/// Everything the stress test needs to know about the churning files.
struct Churn {
    next_version: AtomicUsize,
    stamps: Mutex<Vec<String>>,
}

impl Churn {
    /// Content of version `v`; strictly longer for every new version so that two
    /// versions never share a size (the engine's change detection is mtime + size).
    fn content(&self, v: usize) -> String {
        let stamp = format!("stamp{}", letters(v));
        self.stamps.lock().unwrap().push(stamp.clone());
        format!(
            "// {stamp}\nfn commonchurnword() {{}}\n// {}\n",
            "p".repeat(v % 60) + &"-".repeat(v / 60)
        )
    }
}

#[test]
fn searches_stay_exact_while_builds_edits_and_refreshes_race() {
    let repo = Repo::new();
    let stable = 200;
    for i in 0..stable {
        repo.write(
            &format!("stable/g{}/s{i:03}.rs", i % 10),
            &format!("fn stabletoken{}() {{}}\n", letters(i)),
        );
    }
    let churn_files = 24;
    let churn_path = |j: usize| repo.p(&format!("churn/c{j:02}.rs"));
    fs::create_dir_all(repo.p("churn")).unwrap();
    let cs = repo.cs();
    assert_eq!(sync(&cs, &[]).total_files, stable);

    let churn = Arc::new(Churn {
        next_version: AtomicUsize::new(1),
        stamps: Mutex::new(Vec::new()),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let root = repo.root.clone();
    let paths: Vec<PathBuf> = (0..churn_files).map(churn_path).collect();

    let outcome = within(240, "the stress test", {
        let (cs, churn, stop, paths, root) = (
            cs.clone(),
            churn.clone(),
            stop.clone(),
            paths.clone(),
            root.clone(),
        );
        move || {
            let mut handles = Vec::new();
            // Writers: edit, refresh, delete, remove, sometimes forgetting to tell the index.
            for w in 0..2u64 {
                let (cs, churn, paths) = (cs.clone(), churn.clone(), paths.clone());
                handles.push(std::thread::spawn(move || {
                    let mut rng = Rng::new(900 + w);
                    for _ in 0..250 {
                        let p = rng.pick(&paths).clone();
                        match rng.below(10) {
                            0..=4 => {
                                let v = churn.next_version.fetch_add(1, Ordering::SeqCst);
                                fs::write(&p, churn.content(v)).unwrap();
                                cs.refresh_file(&p);
                            }
                            5 | 6 => {
                                let v = churn.next_version.fetch_add(1, Ordering::SeqCst);
                                fs::write(&p, churn.content(v)).unwrap();
                            }
                            7 => {
                                let _ = fs::remove_file(&p);
                                cs.remove_file(&p);
                            }
                            8 => {
                                let _ = fs::remove_file(&p);
                            }
                            _ => cs.refresh_file(&p),
                        }
                    }
                }));
            }
            let mut watchers = Vec::new();
            // Searchers: the stable files must always be found, whatever else is going on.
            for t in 0..4u64 {
                let (cs, stop, root) = (cs.clone(), stop.clone(), root.clone());
                watchers.push(std::thread::spawn(move || {
                    let mut rng = Rng::new(100 + t);
                    let mut searches = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        if rng.chance(70) {
                            let i = rng.below(stable);
                            let res = cs
                                .search_blocking(&format!("stabletoken{}", letters(i)), 3)
                                .unwrap();
                            assert_eq!(
                                res.matches.len(),
                                1,
                                "stable file {i} lost during concurrent work"
                            );
                            let want = root.join(format!("stable/g{}/s{i:03}.rs", i % 10));
                            assert_eq!(Path::new(&res.matches[0].file), want.as_path());
                            check_hit(&root, &res.matches[0]);
                        } else {
                            let res = cs.search_blocking("commonchurnword", 30).unwrap();
                            let mut seen = HashSet::new();
                            let mut prev = f32::INFINITY;
                            for h in &res.matches {
                                assert!(seen.insert(h.file.clone()), "duplicate hit {}", h.file);
                                assert!(
                                    h.score <= prev
                                        && h.start_line >= 1
                                        && h.start_line <= h.end_line
                                );
                                assert!(
                                    Path::new(&h.file).starts_with(root.join("churn")),
                                    "{}",
                                    h.file
                                );
                                prev = h.score;
                            }
                        }
                        searches += 1;
                    }
                    searches
                }));
            }
            // A builder looping over the whole tree.
            let builder = {
                let (cs, stop) = (cs.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut builds = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        let st = cs.build_blocking(&[]).unwrap();
                        assert!(st.total_files >= stable, "{st:?}");
                        builds += 1;
                    }
                    builds
                })
            };
            // A status poller.
            let poller = {
                let (cs, stop) = (cs.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut worst = Duration::ZERO;
                    while !stop.load(Ordering::Relaxed) {
                        let t = Instant::now();
                        let st = cs.status();
                        worst = worst.max(t.elapsed());
                        assert!(st.built && st.files >= stable, "{st:?}");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    worst
                })
            };
            for h in handles {
                h.join().unwrap();
            }
            stop.store(true, Ordering::Relaxed);
            let searches: u64 = watchers.into_iter().map(|h| h.join().unwrap()).sum();
            let builds = builder.join().unwrap();
            let worst = poller.join().unwrap();
            (searches, builds, worst)
        }
    });
    let (searches, builds, worst) = outcome;
    assert!(
        worst < Duration::from_secs(5),
        "status() was blocked for {worst:?}"
    );
    eprintln!("stress: {searches} searches, {builds} builds, status worst {worst:?}");

    // Quiet again: one more build must leave exactly what is on disk.
    let live: Vec<(String, PathBuf)> = paths
        .iter()
        .filter(|p| p.exists())
        .map(|p| (stamps_in(&fs::read(p).unwrap()).remove(0), p.clone()))
        .collect();
    let st = sync(&cs, &[]);
    assert_eq!(st.total_files, stable + live.len(), "{st:?}");
    let again = sync(&cs, &[]);
    assert_eq!(
        (again.indexed, again.updated, again.removed),
        (0, 0, 0),
        "{again:?}"
    );
    for (stamp, p) in &live {
        let res = search(&cs, stamp);
        assert_eq!(res.matches.len(), 1, "{stamp}");
        assert_eq!(Path::new(&res.matches[0].file), p.as_path());
    }
    let live_stamps: HashSet<&String> = live.iter().map(|(s, _)| s).collect();
    for dead in churn
        .stamps
        .lock()
        .unwrap()
        .iter()
        .filter(|s| !live_stamps.contains(s))
    {
        assert!(
            find(&cs, dead).is_empty(),
            "stale content {dead} is still served"
        );
    }
    let fresh = repo.cs();
    let mut queries: Vec<String> = live.iter().map(|(s, _)| s.clone()).collect();
    queries.push("commonchurnword".into());
    queries.extend(
        (0..stable)
            .step_by(7)
            .map(|i| format!("stabletoken{}", letters(i))),
    );
    for q in &queries {
        assert_eq!(
            serde_json::to_value(search(&cs, q)).unwrap(),
            serde_json::to_value(search(&fresh, q)).unwrap(),
            "{q}: the long-lived index differs from a fresh one"
        );
    }
    assert_eq!(
        (cs.status().files, cs.status().chunks),
        (fresh.status().files, fresh.status().chunks)
    );
}

#[test]
fn concurrent_refreshes_of_one_file_end_with_the_last_write() {
    let repo = Repo::new();
    let p = repo.write("hot.rs", "fn hotfileseed() {}\n");
    repo.write("other.rs", "fn otherfileword() {}\n");
    let cs = repo.cs();
    sync(&cs, &[]);
    for round in 0..20usize {
        // Several threads each write a longer version and refresh; the longest wins on disk.
        let versions: Vec<String> = (0..4)
            .map(|t| {
                format!(
                    "fn hotfileversion{}() {{}}\n{}",
                    letters(round * 4 + t),
                    "// pad\n".repeat(round * 4 + t + 1)
                )
            })
            .collect();
        let lock = Arc::new(Mutex::new(()));
        let handles: Vec<_> = versions
            .iter()
            .map(|text| {
                let (cs, p, lock, text) = (cs.clone(), p.clone(), lock.clone(), text.clone());
                std::thread::spawn(move || {
                    // The write and the refresh of one saver are not atomic together.
                    {
                        let _g = lock.lock().unwrap();
                        fs::write(&p, &text).unwrap();
                    }
                    cs.refresh_file(&p);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // Whatever the interleaving was, once every saver has finished the index
        // must describe the file as it is now: each write is followed by a
        // refresh, so the last refresh to commit read the last write.
        let on_disk = fs::read_to_string(&p).unwrap();
        let word = on_disk
            .split(|c: char| !c.is_alphanumeric())
            .find(|w| w.starts_with("hotfileversion"))
            .unwrap()
            .to_owned();
        assert_eq!(find(&cs, &word), ["hot.rs"], "round {round}");
        for v in &versions {
            let w = v
                .split(|c: char| !c.is_alphanumeric())
                .find(|w| w.starts_with("hotfileversion"))
                .unwrap();
            if w != word {
                assert!(find(&cs, w).is_empty(), "round {round}: {w} is stale");
            }
        }
        assert_eq!(cs.status().files, 2);
    }
}

// ═══════════════════════════ 8. the two tools ════════════════════════════════

async fn run(tool: &dyn Tool, params: Value) -> Value {
    tool.execute(params)
        .await
        .unwrap_or_else(|e| panic!("{} failed: {e}", tool.name()))
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn code_search_tool_returns_the_documented_json() {
    let (_repo, cs) = mini_repo();
    let tool = CodeSearchTool::new(cs.clone());
    assert_eq!(tool.name(), "code_search");
    assert!(tool.description().len() > 40);
    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["query"]["type"], "string");
    assert!(schema["properties"]["top_k"].is_object());
    assert_eq!(schema["required"], json!(["query"]));
    let def = tool.to_definition();
    assert_eq!(
        (def.name.as_str(), &def.parameters),
        ("code_search", &schema)
    );

    let out = run(&tool, json!({"query": "json config", "top_k": 3})).await;
    assert_eq!(keys(&out), set(&["matches"]));
    let matches = out["matches"].as_array().unwrap();
    assert!(!matches.is_empty() && matches.len() <= 3);
    for m in matches {
        assert_eq!(
            keys(m),
            set(&["file", "start_line", "end_line", "score", "snippet"])
        );
        assert!(m["file"].is_string() && m["snippet"].is_string());
        assert!(m["start_line"].is_u64() && m["end_line"].is_u64());
        assert!(m["score"].is_number());
        assert!(Path::new(m["file"].as_str().unwrap()).is_absolute());
    }
    assert!(matches[0]["file"].as_str().unwrap().ends_with("loader.rs"));
    assert_eq!(
        out,
        serde_json::to_value(cs.search_blocking("json config", 3).unwrap()).unwrap()
    );

    // No match: the empty list, nothing else.
    assert_eq!(
        run(&tool, json!({"query": "qqqzzznothingmatchesthis"})).await,
        json!({"matches": []})
    );
    // Nothing searchable in the query: an empty list and a note that says why.
    for q in ["", "   ", "the of a", "!!! ???", "x"] {
        let out = run(&tool, json!({"query": q})).await;
        assert_eq!(out["matches"], json!([]), "{q:?}");
        assert!(
            out["note"].as_str().is_some_and(|n| !n.is_empty()),
            "{q:?}: {out}"
        );
    }
}

#[tokio::test]
async fn code_search_tool_rejects_bad_input_with_an_error_not_a_panic() {
    let (_repo, cs) = mini_repo();
    let tool = CodeSearchTool::new(cs);
    for params in [
        json!({}),
        json!({"query": null}),
        json!({"query": 5}),
        json!({"query": ["json"]}),
        json!({"query": {"q": "json"}}),
        json!({"top_k": 3}),
        json!({"Query": "json"}),
        json!(null),
        json!("json config"),
        json!([]),
        json!(7),
    ] {
        let err = tool
            .execute(params.clone())
            .await
            .expect_err(&format!("{params} must be rejected"));
        assert!(
            err.to_string().to_lowercase().contains("query"),
            "{params}: {err}"
        );
    }
    let too_long = json!({"query": "json ".repeat(10_000)});
    assert!(tool.execute(too_long).await.is_err());
    // A workspace that is gone is an error the model can read, not a panic.
    let gone = CodeSearchTool::new(CodeSearch::new("/no/such/workspace/for/code/search/spec"));
    let err = gone
        .execute(json!({"query": "json"}))
        .await
        .expect_err("missing workspace");
    assert!(!err.to_string().is_empty());
    let err = BuildIndexTool::new(CodeSearch::new("/no/such/workspace/for/code/search/spec"))
        .execute(json!({}))
        .await
        .expect_err("missing workspace");
    assert!(!err.to_string().is_empty());
    // Extra parameters are ignored.
    assert!(tool
        .execute(json!({"query": "json", "unknown": true}))
        .await
        .is_ok());
}

#[tokio::test]
async fn code_search_tool_clamps_top_k() {
    let repo = Repo::new();
    for i in 0..60 {
        repo.write(&format!("t/f{i:02}.rs"), "fn topkclampword() {}\n");
    }
    let tool = CodeSearchTool::new(repo.cs());
    let count = |v: Value| v["matches"].as_array().unwrap().len();
    for (top_k, expect) in [
        (json!(1), 1),
        (json!(2), 2),
        (json!(7), 7),
        (json!(20), 20),
        (json!(21), 20),
        (json!(1000), 20),
        (json!(u64::MAX), 20),
        (json!(3.0), 3),
        (json!("4"), 4),
        (json!(" 6 "), 6),
        (json!(1e30), 20),
        (json!(0), 1),
        (json!(0.0), 1),
        (json!("abc"), 5),
        (json!(null), 5),
        (json!(true), 5),
        (json!([3]), 5),
        (json!({"n": 3}), 5),
    ] {
        let out = run(
            &tool,
            json!({"query": "topkclampword", "top_k": top_k.clone()}),
        )
        .await;
        assert_eq!(count(out), expect, "top_k = {top_k}");
    }
    assert_eq!(
        count(run(&tool, json!({"query": "topkclampword"})).await),
        5,
        "default"
    );
    // Negative and fractional values: something sensible within the range, never an error.
    for top_k in [json!(-1), json!(-5.5), json!(2.9), json!(0.4)] {
        let n = count(
            run(
                &tool,
                json!({"query": "topkclampword", "top_k": top_k.clone()}),
            )
            .await,
        );
        assert!((1..=20).contains(&n), "top_k = {top_k}: {n} results");
    }
}

#[tokio::test]
async fn code_search_tool_says_when_the_index_is_truncated() {
    let repo = Repo::new();
    for i in 0..6 {
        repo.write(&format!("f{i}.rs"), "fn truncatedtoolword() {}\n");
    }
    let cs = repo.cs_with(SearchLimits {
        max_files: 2,
        ..SearchLimits::default()
    });
    let search_tool = CodeSearchTool::new(cs.clone());
    let build_tool = BuildIndexTool::new(cs.clone());
    let out = run(&search_tool, json!({"query": "truncatedtoolword"})).await;
    assert_eq!(out["matches"].as_array().unwrap().len(), 2);
    assert_eq!(out["index_truncated"], json!(true), "{out}");
    assert!(out["note"].as_str().is_some_and(|n| !n.is_empty()), "{out}");
    let built = run(&build_tool, json!({})).await;
    assert_eq!(built["truncated"], json!(true), "{built}");
    assert!(
        built["message"].as_str().is_some_and(|m| !m.is_empty()),
        "{built}"
    );
    assert_eq!(built["total_files"], json!(2));

    // Under the cap there is no flag and no note.
    let repo = Repo::new();
    repo.write("a.rs", "fn truncatedtoolword() {}\n");
    let out = run(
        &CodeSearchTool::new(repo.cs()),
        json!({"query": "truncatedtoolword"}),
    )
    .await;
    assert_eq!(keys(&out), set(&["matches"]), "{out}");
    let built = run(&BuildIndexTool::new(repo.cs()), json!({})).await;
    assert_eq!(built["truncated"], json!(false));
    assert!(
        built.get("message").is_none() && built.get("warnings").is_none(),
        "{built}"
    );
}

#[tokio::test]
async fn build_index_tool_returns_the_documented_json() {
    let (repo, cs) = mini_repo();
    let tool = BuildIndexTool::new(cs.clone());
    assert_eq!(tool.name(), "build_search_index");
    assert!(tool.description().len() > 40);
    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["paths"]["type"], "array");
    assert_eq!(schema["properties"]["paths"]["items"]["type"], "string");
    assert!(schema
        .get("required")
        .is_none_or(|r| r.as_array().is_some_and(|a| a.is_empty())));

    let n = MINI_REPO.len() as u64;
    let out = run(&tool, json!({})).await;
    assert_eq!(
        keys(&out),
        set(&[
            "indexed",
            "updated",
            "unchanged",
            "removed",
            "skipped",
            "errors",
            "privacy_skipped",
            "total_files",
            "chunks",
            "truncated",
            "elapsed_ms"
        ]),
        "{out}"
    );
    for k in [
        "indexed",
        "updated",
        "unchanged",
        "removed",
        "skipped",
        "errors",
        "privacy_skipped",
        "total_files",
        "chunks",
        "elapsed_ms",
    ] {
        assert!(out[k].is_u64(), "{k} in {out}");
    }
    assert!(out["truncated"].is_boolean());
    assert_eq!(
        (
            out["indexed"].as_u64(),
            out["total_files"].as_u64(),
            out["unchanged"].as_u64()
        ),
        (Some(n), Some(n), Some(0))
    );
    let again = run(&tool, json!({})).await;
    assert_eq!(
        (
            again["indexed"].as_u64(),
            again["unchanged"].as_u64(),
            again["updated"].as_u64()
        ),
        (Some(0), Some(n), Some(0))
    );

    // `paths`: a list, a single string, junk entries, an empty list, junk.
    for (params, expect_files) in [
        (json!({"paths": ["src"]}), 4u64),
        (json!({"paths": ["src", "web"]}), 7),
        (json!({"paths": "web"}), 3),
        (json!({"paths": [1, null, {"a": 1}, "src"]}), 4),
        (json!({"paths": []}), n),
        (json!({"paths": ""}), n),
        (json!({"paths": 5}), n),
        (json!({}), n),
    ] {
        let fresh = BuildIndexTool::new(repo.cs());
        let out = run(&fresh, params.clone()).await;
        let indexed = out["indexed"].as_u64().unwrap();
        assert_eq!(indexed, expect_files, "{params}: {out}");
    }
}

#[tokio::test]
async fn build_index_tool_reports_bad_paths_and_never_leaves_the_workspace() {
    let (repo, cs) = mini_repo();
    let elsewhere = Repo::new();
    elsewhere.write("outside.rs", "fn outsideworkspaceword() {}\n");
    let tool = BuildIndexTool::new(cs.clone());
    run(&tool, json!({})).await;
    let out = run(&tool, json!({"paths": ["does/not/exist", "src/nope.rs"]})).await;
    assert!(out["errors"].as_u64().unwrap() >= 2, "{out}");
    assert!(
        out["warnings"].as_array().is_some_and(|w| !w.is_empty()),
        "{out}"
    );
    for outside in [
        elsewhere.root.to_string_lossy().into_owned(),
        elsewhere.p("outside.rs").to_string_lossy().into_owned(),
        "../../../../../../etc".to_string(),
        "/etc/passwd".to_string(),
        "src/../../..".to_string(),
    ] {
        let out = run(&tool, json!({"paths": [outside.clone()]})).await;
        assert_eq!(out["indexed"], json!(0), "{outside}: {out}");
        assert!(
            out["errors"].as_u64().unwrap() >= 1 || out.get("warnings").is_some(),
            "{outside}: {out}"
        );
        assert_eq!(
            out["total_files"].as_u64(),
            Some(MINI_REPO.len() as u64),
            "{outside}: {out}"
        );
    }
    let search_tool = CodeSearchTool::new(cs);
    assert_eq!(
        run(&search_tool, json!({"query": "outsideworkspaceword"})).await["matches"],
        json!([])
    );
    let _ = repo;
}

#[tokio::test]
async fn both_tools_work_through_the_registry_and_in_parallel() {
    use phazeai_core::tools::ToolRegistry;
    let (_repo, cs) = mini_repo();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CodeSearchTool::new(cs.clone())));
    registry.register(Box::new(BuildIndexTool::new(cs.clone())));
    let defs = registry.definitions();
    let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["build_search_index", "code_search"]);
    let registry = Arc::new(registry);
    let mut tasks = Vec::new();
    for i in 0..12 {
        let registry = registry.clone();
        tasks.push(tokio::spawn(async move {
            if i % 3 == 0 {
                registry
                    .get("build_search_index")
                    .unwrap()
                    .execute(json!({}))
                    .await
                    .unwrap()
            } else {
                registry
                    .get("code_search")
                    .unwrap()
                    .execute(json!({"query": "tcp listener", "top_k": 2}))
                    .await
                    .unwrap()
            }
        }));
    }
    for (i, t) in tasks.into_iter().enumerate() {
        let out = t.await.unwrap();
        if i % 3 == 0 {
            assert_eq!(out["total_files"].as_u64(), Some(MINI_REPO.len() as u64));
        } else {
            assert!(out["matches"][0]["file"]
                .as_str()
                .unwrap()
                .ends_with("server.rs"));
        }
    }
}

// ═══════════════════════════ 9. privacy ══════════════════════════════════════

const POLICY: &str = ".phazeai/privacy.toml";

#[test]
fn built_in_secrets_are_never_indexed_or_returned() {
    let _active = ActiveProject::none();
    let repo = Repo::new();
    let secrets = [
        ("credentials.json", "leakcredentialsword"),
        ("config/secrets.yaml", "leaksecretsyamlword"),
        ("gcp/credentials-prod.json", "leakgcpcredword"),
        ("deploy/my-service-account-key.json", "leakserviceacctword"),
        (".docker/config.json", "leakdockerauthword"),
        ("a/b/credentials.json", "leaknestedcredword"),
        // Indexable file names that the built-in list covers, in other spellings.
        ("secrets.toml", "leaksecretstomlword"),
        ("config/Secrets.TXT", "leaksecretsuppertxtword"),
        ("CREDENTIALS.JSON", "leakuppercredword"),
        ("prod/credentials-aws.json", "leakawscredword"),
        ("ci/my-service-account.json", "leakserviceaccword"),
        ("ci/gcp-service-account-prod.json", "leakgcpserviceword"),
        (".ssh/notes.txt", "leaksshnotesword"),
        ("home/.aws/config.json", "leakawsconfigword"),
        (".gnupg/readme.md", "leakgnupgword"),
        ("infra/terraform.tfstate.json", "leaktfstateword"),
        ("keys/id_rsa.txt", "leakidrsaword"),
    ];
    // Look-alikes that are fine to search.
    let allowed = [
        ("src/keys.rs", "allowedkeysword"),
        ("src/environment.ts", "allowedenvword"),
        ("docker/config.json", "allowedconfigword"),
        ("README.md", "allowedreadmeword"),
        ("secret.toml", "allowedsecrettomlword"),
        ("src/my_secrets.rs", "allowedmysecretsword"),
        ("src/secrets/mod.rs", "allowedsecretsmodword"),
        ("src/credentials.rs", "allowedcredentialsrsword"),
        ("src/service_accounts.rs", "allowedserviceaccountsword"),
    ];
    for (path, token) in secrets.iter().chain(&allowed) {
        repo.write(
            path,
            &format!("{{\"value\": \"{token}\", \"shared\": \"sharedlookalikeword\"}}\n"),
        );
    }
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!(st.indexed, allowed.len(), "{st:?}");
    assert!(st.privacy_skipped >= 1, "{st:?}");
    assert_eq!(cs.status().files, allowed.len());
    for (_, token) in &secrets {
        assert!(find(&cs, token).is_empty(), "{token} leaked into the index");
    }
    for (path, token) in &allowed {
        assert_eq!(find(&cs, token), [*path], "{token}");
    }
    let res = search(&cs, "sharedlookalikeword");
    assert_eq!(res.matches.len(), allowed.len());
    for h in &res.matches {
        assert!(!h.snippet.contains("leak"), "{}", h.snippet);
    }
    // Asking for a secret by name, building around it or refreshing it changes nothing.
    for (path, token) in &secrets {
        cs.refresh_file(&repo.p(path));
        let st = cs.build_blocking(&[repo.p(path)]).unwrap();
        assert_eq!(st.indexed, 0, "{path}: {st:?}");
        assert!(st.privacy_skipped >= 1, "{path}: {st:?}");
        assert!(find(&cs, token).is_empty(), "{path}");
    }
    let st = cs
        .build_blocking(&[repo.p("config"), repo.p("gcp"), repo.p(".docker")])
        .unwrap();
    assert_eq!(st.indexed, 0, "{st:?}");
    assert_eq!(cs.status().files, allowed.len());
    // The same holds when some other project is the active one.
    drop(_active);
    let other = Repo::new();
    let _active = ActiveProject::set(&other.root);
    let cs2 = repo.cs();
    assert_eq!(sync(&cs2, &[]).indexed, allowed.len());
    for (_, token) in &secrets {
        assert!(find(&cs2, token).is_empty());
    }
}

#[test]
fn project_policy_keeps_denied_paths_out_of_the_index() {
    let repo = Repo::new();
    repo.write(
        POLICY,
        "deny = [\"internal/**\", \"*.sqlite.json\", \"notes/private.md\", \"/top_secret.rs\"]\n",
    );
    let files = [
        ("internal/plan.md", false),
        ("internal/deep/x.rs", false),
        ("data.sqlite.json", false),
        ("notes/private.md", false),
        ("top_secret.rs", false),
        ("notes/public.md", true),
        ("sub/top_secret.rs", true),
        ("src/internal.rs", true),
        ("internal_docs.md", true),
        // The policy and the IDE's own files are not code.
        (".phazeai/designs/proto.md", false),
    ];
    let _active = ActiveProject::set(&repo.root);
    let mut tokens = Vec::new();
    for (i, (path, allowed)) in files.iter().enumerate() {
        let token = format!("policytok{}word", letters(i));
        repo.write(path, &format!("{token}\n"));
        tokens.push((*path, token, *allowed));
    }
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    let allowed = files.iter().filter(|f| f.1).count();
    assert_eq!(st.indexed, allowed, "{st:?}");
    assert!(st.privacy_skipped >= 1, "{st:?}");
    for (path, token, ok) in &tokens {
        let got = find(&cs, token);
        if *ok {
            assert_eq!(got, [*path]);
        } else {
            assert!(got.is_empty(), "{path} is denied but was found");
        }
    }
    // Asking for a denied directory is refused and reported.
    let st = cs.build_blocking(&[repo.p("internal")]).unwrap();
    assert_eq!(st.indexed, 0, "{st:?}");
    assert!(st.privacy_skipped >= 1, "{st:?}");
}

#[test]
fn tightening_the_policy_after_indexing_is_honoured_at_once_and_at_the_next_build() {
    for deny in [
        r#"["finance/**"]"#,
        r#"["finance/"]"#,
        r#"["/finance"]"#,
        r#"["finance"]"#,
        r#"["*.md"]"#,
        r#"["**/budget.md"]"#,
        r#"["finance/budget.md"]"#,
    ] {
        let repo = Repo::new();
        repo.write("finance/budget.md", "budgetlineword budgetsharedword\n");
        repo.write("docs/open.rs", "openlineword budgetsharedword\n");
        let _active = ActiveProject::set(&repo.root);
        let cs = repo.cs();
        assert_eq!(sync(&cs, &[]).total_files, 2);
        assert_eq!(find(&cs, "budgetlineword"), ["finance/budget.md"], "{deny}");

        // Tighten: no rebuild, no restart.
        repo.write(POLICY, &format!("deny = {deny}\n"));
        let res = search(&cs, "budgetlineword");
        assert!(
            res.matches.is_empty(),
            "{deny}: a denied file is still returned"
        );
        assert_eq!(res.privacy_skipped, 1, "{deny}");
        let res = search(&cs, "budgetsharedword");
        assert_eq!(names(&cs, &res), ["docs/open.rs"], "{deny}");
        assert_eq!(res.privacy_skipped, 1, "{deny}");
        assert_eq!(
            serde_json::to_value(&res).unwrap()["privacy_skipped"],
            json!(1)
        );
        for h in &res.matches {
            assert!(!h.snippet.contains("budgetlineword"));
        }

        // The next sync drops it from the index for good.
        let st = sync(&cs, &[]);
        assert_eq!((st.removed, st.total_files), (1, 1), "{deny}: {st:?}");
        assert!(st.privacy_skipped >= 1, "{deny}: {st:?}");
        let res = search(&cs, "budgetlineword");
        assert!(
            res.matches.is_empty() && res.privacy_skipped == 0,
            "{deny}: {res:?}"
        );

        // Loosen it again: the file comes back with the next sync, not before.
        fs::remove_file(repo.p(POLICY)).unwrap();
        assert!(find(&cs, "budgetlineword").is_empty());
        let st = sync(&cs, &[]);
        assert_eq!((st.indexed, st.total_files), (1, 2), "{deny}: {st:?}");
        assert_eq!(find(&cs, "budgetlineword"), ["finance/budget.md"], "{deny}");
    }
}

#[test]
fn refreshing_a_newly_denied_file_drops_it() {
    let repo = Repo::new();
    let secret = repo.write("finance/budget.md", "budgetlineword\n");
    repo.write("docs/open.rs", "openlineword\n");
    let _active = ActiveProject::set(&repo.root);
    let cs = repo.cs();
    sync(&cs, &[]);
    repo.write(POLICY, "deny = [\"finance/**\"]\n");
    cs.refresh_file(&secret);
    assert_eq!(cs.status().files, 1);
    assert!(find(&cs, "budgetlineword").is_empty());
    // And a denied file that is rewritten and refreshed does not sneak back in.
    repo.write("finance/budget.md", "budgetlineword and more text\n");
    cs.refresh_file(&secret);
    assert_eq!(cs.status().files, 1);
    assert!(find(&cs, "budgetlineword").is_empty());
}

#[test]
fn denied_files_do_not_use_up_result_slots() {
    let repo = Repo::new();
    for i in 0..60 {
        repo.write(
            &format!("finance/f{i:02}.md"),
            &format!("budgetline budgetline budgetline budgetline {i}\n"),
        );
    }
    for i in 0..3 {
        repo.write(&format!("open/o{i}.md"), "budgetline\n");
    }
    let _active = ActiveProject::set(&repo.root);
    let cs = repo.cs();
    let before = search_k(&cs, "budgetline", 3);
    assert!(
        names(&cs, &before)
            .iter()
            .all(|f| f.starts_with("finance/")),
        "the strong matches come first"
    );
    repo.write(POLICY, "deny = [\"finance/**\"]\n");
    for k in [1, 3, 10, 50] {
        let res = search_k(&cs, "budgetline", k);
        assert_eq!(
            res.matches.len(),
            k.min(3),
            "top_k {k}: {:?}",
            names(&cs, &res)
        );
        assert!(
            names(&cs, &res).iter().all(|f| f.starts_with("open/")),
            "{:?}",
            names(&cs, &res)
        );
        assert!(res.privacy_skipped >= 1, "top_k {k}");
    }
}

#[test]
fn an_invalid_policy_file_fails_closed() {
    let repo = Repo::new();
    repo.write(POLICY, "deny = [\"finance/**\"]\n");
    repo.write("finance/budget.md", "budgetlineword\n");
    repo.write("docs/open.rs", "openlineword\n");
    let _active = ActiveProject::set(&repo.root);
    let cs = repo.cs();
    assert_eq!(sync(&cs, &[]).total_files, 1);
    // Someone breaks the policy file. The last good list stays in force.
    repo.write(POLICY, "deny = [\n this is not toml");
    let rebuilt = repo.cs();
    let st = sync(&rebuilt, &[]);
    assert_eq!(st.total_files, 1, "{st:?}");
    assert!(find(&rebuilt, "budgetlineword").is_empty());
    assert!(find(&cs, "budgetlineword").is_empty());
    assert_eq!(find(&cs, "openlineword"), ["docs/open.rs"]);
    // Fixing it loosens things again.
    repo.write(POLICY, "deny = []\n");
    let st = sync(&cs, &[]);
    assert_eq!(st.total_files, 2, "{st:?}");
    assert_eq!(find(&cs, "budgetlineword"), ["finance/budget.md"]);
}

#[test]
fn the_policy_file_and_ide_state_are_never_searchable() {
    let repo = Repo::new();
    repo.write(POLICY, "# policyfilecommentword\ndeny = []\n");
    repo.write(".phazeai/design-system.md", "designsystemword\n");
    repo.write(
        ".phazeai/designs/proto.html",
        "<p>designprototypeword</p>\n",
    );
    repo.write("src/a.rs", "fn visiblecodeword() {}\n");
    let _active = ActiveProject::set(&repo.root);
    let cs = repo.cs();
    sync(&cs, &[]);
    for w in [
        "policyfilecommentword",
        "designsystemword",
        "designprototypeword",
    ] {
        assert!(find(&cs, w).is_empty(), "{w}");
    }
    assert_eq!(find(&cs, "visiblecodeword"), ["src/a.rs"]);
}

// ═════════════ 10. seeded differential tests against fresh and oracle ═══════

const VOCAB: &[&str] = &[
    "parse",
    "json",
    "config",
    "server",
    "client",
    "parseJsonConfig",
    "load_settings",
    "HTTPServer",
    "render",
    "buffer",
    "cache",
    "handle_request",
    "utf8",
    "x86_64",
    "MAX_RETRY",
    "fooBar_baz",
    "naïve",
    "日本語",
    "token",
    "stream",
    "queue",
    "lock",
    "Index",
    "Matrix",
    "widget",
    "get",
    "set",
    "run",
];

/// Paths the random operations create. The last group can never be indexed.
const POOL: &[&str] = &[
    "src/a.rs",
    "src/b.rs",
    "src/deep/c.rs",
    "src/deep/d.py",
    "lib/e.js",
    "lib/f.ts",
    "docs/g.md",
    "docs/h.txt",
    "i.go",
    "j.json",
    "tests/k.rs",
    "tests/sub/l.rs",
    "m.c",
    "n.yaml",
    "node_modules/pkg/o.js",
    "target/p.rs",
    "src/build/q.rs",
    "vendor/r.go",
    ".hidden.rs",
    "icon.png",
    "Cargo.lock",
    "src/.dot.md",
];

const DIRS: &[&str] = &[
    "src",
    "src/deep",
    "docs",
    "lib",
    "tests",
    "tests/sub",
    "node_modules",
    "target",
];

/// The .gitignore variants the random operations switch between.
const VARIANTS: &[&str] = &[
    "",
    "docs/\n",
    "*.rs\n!src/a.rs\n",
    "/lib\n",
    "*.md\n*.txt\n",
];

/// What the old sidecar skipped and indexed (the contract the engine inherits).
const ORACLE_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    "dist",
    "build",
    ".next",
    ".venv",
    "venv",
    "vendor",
];
const ORACLE_EXTS: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "cpp", "cc", "cxx", "h", "hpp", "md",
    "toml", "json", "yaml", "yml", "txt",
];

const DIFF_LIMITS: SearchLimits = SearchLimits {
    max_files: 50_000,
    max_file_bytes: 3000,
    max_total_bytes: 256 << 20,
};

/// Every regular file below `root` as (relative path, absolute path), sorted.
fn walk_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap() {
            let e = e.unwrap();
            let ft = e.file_type().unwrap();
            let path = e.path();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
    }
    out.sort();
    out
}

/// Whether a file of this path could ever be indexed, whatever it contains.
fn oracle_name_ok(rel: &str) -> bool {
    let comps: Vec<&str> = rel.split('/').collect();
    let (name, dirs) = comps.split_last().unwrap();
    if dirs.iter().any(|d| ORACLE_SKIP_DIRS.contains(d)) || name.starts_with('.') {
        return false;
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty() && ORACLE_EXTS.contains(&ext.to_ascii_lowercase().as_str())
        }
        None => false,
    }
}

/// What the `.gitignore` of the given variant ignores (a pattern matching a
/// directory ignores everything below it).
fn oracle_ignored(variant: usize, rel: &str) -> bool {
    let comps: Vec<&str> = rel.split('/').collect();
    match variant {
        0 => false,
        1 => comps[..comps.len() - 1].contains(&"docs"),
        2 => comps
            .iter()
            .enumerate()
            .any(|(k, c)| c.ends_with(".rs") && !(k == 1 && comps[0] == "src" && *c == "a.rs")),
        3 => comps.len() > 1 && comps[0] == "lib",
        4 => comps
            .iter()
            .any(|c| c.ends_with(".md") || c.ends_with(".txt")),
        _ => unreachable!(),
    }
}

/// The files that must be in the index right now, and the subset of them that
/// has no text at all (an engine may or may not keep those).
fn oracle(root: &Path, variant: usize, max_bytes: u64) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut expected, mut blank_files) = (BTreeSet::new(), BTreeSet::new());
    for (rel, path) in walk_files(root) {
        if !oracle_name_ok(&rel) || oracle_ignored(variant, &rel) {
            continue;
        }
        let bytes = fs::read(&path).unwrap();
        if bytes.len() as u64 > max_bytes || bytes[..bytes.len().min(8192)].contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if text.lines().all(blank) {
            blank_files.insert(rel.clone());
        }
        expected.insert(rel);
    }
    (expected, blank_files)
}

/// Stamp -> relative path for every stamp that is on disk right now.
fn live_stamps(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (rel, path) in walk_files(root) {
        for stamp in stamps_in(&fs::read(path).unwrap()) {
            out.insert(stamp, rel.clone());
        }
    }
    out
}

fn gen_text(rng: &mut Rng, stamp: &str) -> Vec<String> {
    let n = 1 + rng.below(50);
    let mut lines: Vec<String> = (0..n)
        .map(|_| {
            if rng.chance(12) {
                String::new()
            } else {
                let k = 1 + rng.below(4);
                (0..k)
                    .map(|_| *rng.pick(VOCAB))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        })
        .collect();
    let at = rng.below(n);
    lines[at] = format!("// {stamp} {}", lines[at]);
    lines
}

struct World {
    repo: Repo,
    cs: CodeSearch,
    rng: Rng,
    stamp_no: usize,
    tick: u64,
    all_stamps: Vec<String>,
    variant: usize,
    ctx: String,
}

impl World {
    fn new(seed: u64) -> Self {
        let repo = Repo::new();
        let cs = repo.cs_with(DIFF_LIMITS);
        World {
            repo,
            cs,
            rng: Rng::new(seed),
            stamp_no: 0,
            tick: 0,
            all_stamps: Vec::new(),
            variant: 0,
            ctx: format!("seed {seed}"),
        }
    }

    fn stamp(&mut self) -> String {
        self.stamp_no += 1;
        let s = format!("stamp{}", letters(self.stamp_no));
        self.all_stamps.push(s.clone());
        s
    }

    /// Random file content: mostly text (sometimes CRLF or with a BOM), and
    /// sometimes empty, blank, binary, not UTF-8 or over the size limit.
    fn content(&mut self) -> Vec<u8> {
        let stamp = self.stamp();
        let limit = DIFF_LIMITS.max_file_bytes as usize;
        match self.rng.below(100) {
            0..=3 => Vec::new(),
            4..=6 => b" \n\n\t\n".to_vec(),
            7..=10 => {
                let mut v = vec![0u8, 1, 2];
                v.extend(format!("{stamp} binary\n").bytes());
                v
            }
            11..=13 => {
                let mut v = format!("{stamp} caf").into_bytes();
                v.extend([0xE9]);
                v.extend(b" latin1\n");
                v
            }
            14..=17 => {
                let target = limit + 1 + self.rng.below(300);
                let mut t = format!("// {stamp}\n");
                while t.len() < target {
                    t.push_str("let somewhere = nothing_here;\n");
                }
                t.into_bytes()
            }
            _ => {
                let lines = gen_text(&mut self.rng, &stamp);
                let eol = if self.rng.chance(25) { "\r\n" } else { "\n" };
                let mut t = lines.join(eol);
                if self.rng.chance(80) {
                    t.push_str(eol);
                }
                if self.rng.chance(5) {
                    t.insert(0, '\u{FEFF}');
                }
                t.into_bytes()
            }
        }
    }

    /// Write random content at `rel`; every write gets its own timestamp.
    fn put(&mut self, rel: &str) {
        if self.repo.p(rel).is_dir() {
            return;
        }
        let bytes = self.content();
        let p = self.repo.write_bytes(rel, &bytes);
        self.tick += 1;
        set_mtime(&p, self.tick);
    }

    fn pool_path(&mut self) -> String {
        self.rng.pick(POOL).to_string()
    }

    fn existing_file(&mut self) -> Option<String> {
        // The .gitignore is managed by the variant switch, not by random file operations.
        let files: Vec<_> = walk_files(&self.repo.root)
            .into_iter()
            .filter(|(r, _)| r != ".gitignore")
            .collect();
        (!files.is_empty()).then(|| files[self.rng.below(files.len())].0.clone())
    }

    fn check_hits_are_indexable_names(&self, res: &SearchResults) {
        let root = self.cs.root();
        for h in &res.matches {
            let r = rel(&root, &h.file);
            assert!(
                oracle_name_ok(&r),
                "{}: {r} can never be indexed but was returned",
                self.ctx
            );
        }
    }

    fn step(&mut self) {
        let root = self.repo.root.clone();
        let r = self.rng.below(100);
        match r {
            0..=23 => {
                let rel = self.pool_path();
                self.ctx.push_str(&format!(" put {rel}"));
                self.put(&rel);
            }
            24..=31 => {
                let rel = if self.rng.chance(80) {
                    self.pool_path()
                } else {
                    self.rng.pick(DIRS).to_string()
                };
                self.ctx.push_str(&format!(" delete {rel}"));
                let p = root.join(&rel);
                if p.is_dir() {
                    fs::remove_dir_all(p).unwrap();
                } else if p.exists() {
                    fs::remove_file(p).unwrap();
                }
            }
            32..=37 => {
                let to = self.pool_path();
                if let Some(from) = self.existing_file() {
                    let (f, t) = (root.join(&from), root.join(&to));
                    self.ctx.push_str(&format!(" rename {from} -> {to}"));
                    if f.is_file() && !t.exists() {
                        fs::create_dir_all(t.parent().unwrap()).unwrap();
                        fs::rename(f, t).unwrap();
                    }
                }
            }
            38..=41 => {
                self.variant = self.rng.below(VARIANTS.len());
                self.ctx
                    .push_str(&format!(" gitignore variant {}", self.variant));
                if VARIANTS[self.variant].is_empty() {
                    let _ = fs::remove_file(root.join(".gitignore"));
                } else {
                    fs::write(root.join(".gitignore"), VARIANTS[self.variant]).unwrap();
                }
            }
            42..=44 => {
                let rel = self.pool_path();
                self.ctx.push_str(&format!(" dirify {rel}"));
                let p = root.join(&rel);
                if p.is_file() {
                    fs::remove_file(&p).unwrap();
                    fs::create_dir(&p).unwrap();
                    self.put(&format!("{rel}/inner.rs"));
                }
            }
            45 => {
                let rel = self.pool_path();
                self.ctx.push_str(&format!(" undirify {rel}"));
                let p = root.join(&rel);
                if p.is_dir() {
                    fs::remove_dir_all(p).unwrap();
                }
            }
            46 => {
                self.ctx.push_str(" move src/deep");
                let (a, b) = (root.join("src/deep"), root.join("src/deep_moved"));
                if a.is_dir() && !b.exists() {
                    fs::rename(a, b).unwrap();
                } else if b.is_dir() && !a.exists() {
                    fs::rename(b, a).unwrap();
                }
            }
            47..=49 => {
                if let Some(rel) = self.existing_file() {
                    self.ctx.push_str(&format!(" touch {rel}"));
                    self.tick += 1;
                    set_mtime(&root.join(rel), self.tick);
                }
            }
            50..=60 => {
                let rel = if self.rng.chance(70) {
                    self.pool_path()
                } else {
                    self.existing_file().unwrap_or_else(|| "src/a.rs".into())
                };
                self.ctx.push_str(&format!(" refresh {rel}"));
                let p = root.join(&rel);
                let stamps = if p.is_file() {
                    stamps_in(&fs::read(&p).unwrap())
                } else {
                    Vec::new()
                };
                self.cs.refresh_file(&p);
                let (expected, blank_files) =
                    oracle(&root, self.variant, DIFF_LIMITS.max_file_bytes);
                for s in stamps {
                    let got = find(&self.cs, &s);
                    if expected.contains(&rel) && !blank_files.contains(&rel) {
                        assert_eq!(
                            got,
                            std::slice::from_ref(&rel),
                            "{}: refreshed file is not searchable",
                            self.ctx
                        );
                    } else {
                        assert!(
                            got.is_empty(),
                            "{}: {rel} must not be indexed after a refresh, found {got:?}",
                            self.ctx
                        );
                    }
                }
            }
            61..=65 => {
                let rel = if self.rng.chance(70) {
                    self.pool_path()
                } else {
                    self.rng.pick(DIRS).to_string()
                };
                self.ctx.push_str(&format!(" remove_file {rel}"));
                let below = format!("{rel}/");
                let affected: Vec<String> = walk_files(&root)
                    .into_iter()
                    .filter(|(r, _)| *r == rel || r.starts_with(&below))
                    .flat_map(|(_, p)| stamps_in(&fs::read(p).unwrap()))
                    .collect();
                self.cs.remove_file(&root.join(&rel));
                for s in affected {
                    assert!(
                        find(&self.cs, &s).is_empty(),
                        "{}: {s} is still served after remove_file({rel})",
                        self.ctx
                    );
                }
            }
            66..=75 => {
                self.ctx.push_str(" build");
                sync(&self.cs, &[]);
            }
            76..=80 => {
                let n = 1 + self.rng.below(3);
                let paths: Vec<PathBuf> = (0..n)
                    .map(|_| {
                        let rel = if self.rng.chance(50) {
                            self.pool_path()
                        } else {
                            self.rng.pick(DIRS).to_string()
                        };
                        root.join(rel)
                    })
                    .collect();
                self.ctx.push_str(
                    &format!(" partial build {paths:?}").replace(root.to_str().unwrap(), ""),
                );
                sync(&self.cs, &paths);
            }
            _ => {
                let k = 1 + self.rng.below(3);
                let q: Vec<&str> = (0..k).map(|_| *self.rng.pick(VOCAB)).collect();
                let q = q.join(" ");
                self.ctx.push_str(&format!(" query {q:?}"));
                let res = search_k(&self.cs, &q, 10);
                self.check_hits_are_indexable_names(&res);
            }
        }
        self.verify_light();
    }

    /// Cheap checks after every step: what is gone is never served, and every
    /// hit is a real, current, indexable file.
    fn verify_light(&mut self) {
        let root = self.repo.root.clone();
        let live = live_stamps(&root);
        let dead: Vec<String> = self
            .all_stamps
            .iter()
            .filter(|s| !live.contains_key(*s))
            .cloned()
            .collect();
        for _ in 0..4.min(dead.len()) {
            let s = self.rng.pick(&dead).clone();
            assert!(
                find(&self.cs, &s).is_empty(),
                "{}: the replaced or deleted text {s} is still served",
                self.ctx
            );
        }
        let live_list: Vec<(&String, &String)> = live.iter().collect();
        for _ in 0..3.min(live_list.len()) {
            let (s, r) = *self.rng.pick(&live_list);
            let got = find(&self.cs, s);
            assert!(
                got.iter().all(|g| g == r),
                "{}: {s} lives in {r} but was found in {got:?}",
                self.ctx
            );
        }
    }

    /// The expensive check: after a full build the index is exactly what the
    /// oracle says, and identical to a freshly built one.
    fn verify_full(&mut self) {
        let root = self.repo.root.clone();
        let ctx = self.ctx.clone();
        let st = sync(&self.cs, &[]);
        let (expected, blank_files) = oracle(&root, self.variant, DIFF_LIMITS.max_file_bytes);
        let lowest = expected.len() - blank_files.len();
        assert!(
            (lowest..=expected.len()).contains(&st.total_files),
            "{ctx}: {} files indexed, the oracle expects {}..={}: {st:?}\nexpected: {expected:?}",
            st.total_files,
            lowest,
            expected.len()
        );
        let live = live_stamps(&root);
        for (s, r) in &live {
            let res = search(&self.cs, s);
            let got = names(&self.cs, &res);
            if expected.contains(r) && !blank_files.contains(r) {
                assert_eq!(got, std::slice::from_ref(r), "{ctx}: {s}");
                let text = String::from_utf8_lossy(&fs::read(root.join(r)).unwrap()).into_owned();
                assert!(
                    covers(&res.matches[0], line_of(&text, s)),
                    "{ctx}: {s} in {r}: {:?}",
                    res.matches[0]
                );
            } else {
                assert!(
                    got.is_empty(),
                    "{ctx}: {r} must not be indexed but {s} was found in {got:?}"
                );
            }
        }
        for s in self.all_stamps.iter().filter(|s| !live.contains_key(*s)) {
            assert!(find(&self.cs, s).is_empty(), "{ctx}: stale {s}");
        }
        // Identical to a fresh index.
        let fresh = CodeSearch::with_limits(root.clone(), DIFF_LIMITS);
        let fst = fresh.build_blocking(&[]).unwrap();
        assert_eq!(
            (fst.total_files, fst.chunks),
            (st.total_files, st.chunks),
            "{ctx}: fresh {fst:?} vs {st:?}"
        );
        let mut queries: Vec<String> = live.keys().cloned().collect();
        for _ in 0..10 {
            let k = 1 + self.rng.below(3);
            queries.push(
                (0..k)
                    .map(|_| *self.rng.pick(VOCAB))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        queries.extend([
            "parse json config".into(),
            "Matrix widget".into(),
            "日本語 naïve".into(),
        ]);
        for q in &queries {
            let a = serde_json::to_value(search(&self.cs, q)).unwrap();
            let b = serde_json::to_value(search(&fresh, q)).unwrap();
            assert_eq!(
                a, b,
                "{ctx}: query {q:?} differs between the long-lived and a fresh index"
            );
        }
        let again = sync(&self.cs, &[]);
        assert_eq!(
            (again.indexed, again.updated, again.removed),
            (0, 0, 0),
            "{ctx}: second build changed something: {again:?}"
        );
    }
}

fn run_seed(seed: u64, steps: usize) {
    let mut w = World::new(seed);
    for _ in 0..8 {
        let rel = w.pool_path();
        w.put(&rel);
    }
    w.verify_full();
    for i in 0..steps {
        w.ctx = format!("seed {seed} step {i}:");
        w.step();
        if i % 20 == 19 {
            w.verify_full();
        }
    }
    w.ctx = format!("seed {seed} final:");
    w.verify_full();
}

#[test]
fn differential_seeds_00_to_09() {
    for seed in 0..10 {
        run_seed(seed, 90);
    }
}

#[test]
fn differential_seeds_10_to_19() {
    for seed in 10..20 {
        run_seed(seed, 90);
    }
}

#[test]
fn differential_seeds_20_to_29() {
    for seed in 20..30 {
        run_seed(seed, 90);
    }
}

#[test]
fn differential_seeds_30_to_39() {
    for seed in 30..40 {
        run_seed(seed, 90);
    }
}

#[test]
fn differential_one_long_run() {
    run_seed(777, 600);
}

// ═══════════════════════ 11. more adversarial cases ══════════════════════════

#[test]
fn identifier_splitting_covers_the_common_naming_styles() {
    let repo = Repo::new();
    let cases = [
        ("HTTPQuixServer", "http quix server"),
        ("getZorbURL", "get zorb url"),
        ("parseXMLZingDocument", "parse xml zing document"),
        ("sha256Zap", "sha 256 zap"),
        ("kebab-quark-name", "kebab quark name"),
        ("__private_flux_helper__", "private flux helper"),
        ("iPhoneGlint", "phone glint"),
        ("snake_CASE_Mixed_blorp", "case mixed blorp"),
        ("foo::bar::baz_wibble", "bar baz wibble"),
        ("ClassNameFrob.methodNameGrok", "frob grok"),
        ("MAX_RETRY_COUNT_PLINK", "max retry count plink"),
        ("ünïcödeWörterbuch", "wörterbuch"),
    ];
    for (i, (ident, _)) in cases.iter().enumerate() {
        repo.write(
            &format!("ids/f{i:02}.txt"),
            &format!("some text with {ident} inside\n"),
        );
    }
    let cs = repo.cs();
    for (i, (ident, query)) in cases.iter().enumerate() {
        let file = format!("ids/f{i:02}.txt");
        assert_eq!(
            top1(&cs, query).as_deref(),
            Some(file.as_str()),
            "words {query:?}"
        );
        assert_eq!(
            top1(&cs, ident).as_deref(),
            Some(file.as_str()),
            "identifier {ident:?}"
        );
    }
}

// The snippet of a hit inside one very long line (minified JS, a data file) is a window
// of about 600 characters around the match, not the head of the line, which can leave
// out the matching text completely and tells the reader nothing about the hit.
#[test]
fn the_snippet_of_a_very_long_line_shows_the_match() {
    let repo = Repo::new();
    let mut line = String::from("var a=1;");
    while line.len() < 3000 {
        line.push_str("b.c(d,e);");
    }
    line.push_str("function snippetmarkerzzq(){}");
    while line.len() < 6000 {
        line.push_str("b.c(d,e);");
    }
    repo.write("static/m.js", &(line + "\n"));
    let cs = repo.cs();
    let res = search(&cs, "snippetmarkerzzq");
    assert_eq!(res.matches.len(), 1);
    assert!(
        res.matches[0].snippet.contains("snippetmarkerzzq"),
        "the snippet does not contain the match: {:?}",
        &res.matches[0].snippet[..60]
    );
}

#[test]
fn the_policy_and_gitignore_of_an_enclosing_project_apply_to_a_search_root_inside_it() {
    let repo = Repo::new();
    repo.write(POLICY, "deny = [\"sub/secret/**\"]\n");
    repo.write(".gitignore", "*.generated.rs\n");
    repo.write("sub/secret/key.rs", "fn subsecretword() {}\n");
    repo.write("sub/open.rs", "fn subopenword() {}\n");
    repo.write("sub/x.generated.rs", "fn subgeneratedword() {}\n");
    repo.write("outside.rs", "fn outsiderootword() {}\n");
    let _active = ActiveProject::set(&repo.root);
    let cs = CodeSearch::new(repo.p("sub"));
    let st = sync(&cs, &[]);
    assert_eq!(st.indexed, 1, "{st:?}");
    assert_eq!(find(&cs, "subopenword"), ["open.rs"]);
    for w in ["subsecretword", "subgeneratedword", "outsiderootword"] {
        assert!(find(&cs, w).is_empty(), "{w}");
    }
}

#[test]
fn an_empty_or_odd_root_is_an_error_not_a_panic() {
    for root in ["", ".", "./", "no-such-dir-for-code-search-spec"] {
        let cs = CodeSearch::new(root);
        assert!(
            cs.root().is_absolute() || root.is_empty(),
            "{:?}",
            cs.root()
        );
        if root.is_empty() || root.starts_with("no-such") {
            assert!(
                matches!(cs.build_blocking(&[]), Err(SearchError::Io(_))),
                "{root:?}"
            );
            assert!(cs.search_blocking("anything", 3).is_err(), "{root:?}");
        }
        let _ = cs.status();
        cs.refresh_file(Path::new(""));
        cs.remove_file(Path::new(""));
    }
    // A root that is a file, not a directory: nothing to index, nothing to crash on.
    let repo = Repo::new();
    let file = repo.write("lonely.rs", "fn lonelyfileword() {}\n");
    let cs = CodeSearch::new(file);
    if let Ok(st) = cs.build_blocking(&[]) {
        assert_eq!(st.total_files, 0, "{st:?}");
    }
    let _ = cs.search_blocking("lonelyfileword", 3);
}

#[test]
fn refresh_file_respects_max_files() {
    let repo = Repo::new();
    for n in ["aa", "bb"] {
        repo.write(
            &format!("{n}.rs"),
            &format!("fn {n}refreshcapword() {{}}\n"),
        );
    }
    let cs = repo.cs_with(SearchLimits {
        max_files: 2,
        ..SearchLimits::default()
    });
    sync(&cs, &[]);
    let extra = repo.write("cc.rs", "fn ccrefreshcapword() {}\n");
    cs.refresh_file(&extra);
    assert_eq!(
        cs.status().files,
        2,
        "refresh_file must not grow the index past max_files"
    );
    assert!(find(&cs, "ccrefreshcapword").is_empty());
    assert!(cs.status().truncated);
    // Files already in the index can still be refreshed.
    let aa = repo.write("aa.rs", "fn aarefreshcapword2() {}\nfn more() {}\n");
    cs.refresh_file(&aa);
    assert_eq!(find(&cs, "aarefreshcapword2"), ["aa.rs"]);
    assert_eq!(cs.status().files, 2);
}

#[test]
fn reset_root_races_with_every_other_operation() {
    let a = Repo::new();
    let b = Repo::new();
    let (na, nb) = (300, 60);
    for i in 0..na {
        a.write(
            &format!("d{}/a{i}.rs", i % 12),
            &format!("fn fromprojecta{}() {{}}\n", letters(i)),
        );
    }
    for i in 0..nb {
        b.write(
            &format!("b{i}.rs"),
            &format!("fn fromprojectb{}() {{}}\n", letters(i)),
        );
    }
    let cs = a.cs();
    cs.build_blocking(&[]).unwrap();
    let (ra, rb) = (a.root.clone(), b.root.clone());
    within(180, "reset_root race", {
        let (cs, ra, rb) = (cs.clone(), ra.clone(), rb.clone());
        move || {
            let handles: Vec<_> = (0..6u64)
                .map(|t| {
                    let (cs, ra, rb) = (cs.clone(), ra.clone(), rb.clone());
                    std::thread::spawn(move || {
                        let mut rng = Rng::new(40 + t);
                        for _ in 0..150 {
                            match rng.below(7) {
                                0 => cs.reset_root(if rng.chance(50) {
                                    ra.clone()
                                } else {
                                    rb.clone()
                                }),
                                1 | 2 => {
                                    let (c, n) = if rng.chance(50) { ('a', na) } else { ('b', nb) };
                                    let q = format!("fromproject{c}{}", letters(rng.below(n)));
                                    match cs.search_blocking(&q, 3) {
                                        Ok(res) => {
                                            for h in &res.matches {
                                                let p = Path::new(&h.file);
                                                assert!(
                                                    p.starts_with(&ra) || p.starts_with(&rb),
                                                    "{}",
                                                    h.file
                                                );
                                            }
                                        }
                                        Err(SearchError::RootChanged) => {}
                                        Err(e) => panic!("search failed: {e}"),
                                    }
                                }
                                3 => match cs.build_blocking(&[]) {
                                    Ok(_) | Err(SearchError::RootChanged) => {}
                                    Err(e) => panic!("build failed: {e}"),
                                },
                                4 => {
                                    let i = rng.below(nb);
                                    cs.refresh_file(&rb.join(format!("b{i}.rs")));
                                    let i = rng.below(na);
                                    cs.refresh_file(&ra.join(format!("d{}/a{i}.rs", i % 12)));
                                }
                                5 => cs.remove_file(&if rng.chance(50) {
                                    ra.join("d3")
                                } else {
                                    rb.join("b7.rs")
                                }),
                                _ => {
                                    let _ = cs.status();
                                }
                            }
                        }
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap();
            }
        }
    });
    cs.reset_root(b.root.clone());
    let st = sync(&cs, &[]);
    assert_eq!(
        st.total_files, nb,
        "files of the other root leaked into the index: {st:?}"
    );
    for i in [0, 5, 150, 299] {
        assert!(find(&cs, &format!("fromprojecta{}", letters(i))).is_empty());
    }
    for i in [0, 7, 59] {
        assert_eq!(find(&cs, &format!("fromprojectb{}", letters(i))).len(), 1);
    }
    let fresh = b.cs();
    fresh.build_blocking(&[]).unwrap();
    assert_eq!(
        (cs.status().files, cs.status().chunks),
        (fresh.status().files, fresh.status().chunks)
    );
}

#[test]
fn heavy_churn_compacts_the_index_without_changing_any_answer() {
    // Rewriting every file many times leaves thousands of dead chunks behind, which
    // the index eventually renumbers; nothing a search returns may change because of it.
    let repo = Repo::new();
    let mut rng = Rng::new(4242);
    let n = 120;
    let cs = repo.cs();
    let mut stamps = Vec::new();
    let mut version = 0usize;
    let path = |i: usize| format!("c/g{}/f{i:03}.rs", i % 6);
    let write_all = |repo: &Repo,
                     rng: &mut Rng,
                     version: &mut usize,
                     stamps: &mut Vec<String>,
                     idx: &[usize]| {
        for &i in idx {
            *version += 1;
            let stamp = format!("stamp{}", letters(*version));
            stamps.push(stamp.clone());
            // 90 lines without blank lines: two chunks per file.
            let mut text = format!("// {stamp}\n");
            for k in 0..89 {
                text.push_str(&format!("let {} = {};\n", rng.pick(VOCAB), k));
            }
            let p = repo.write(&path(i), &text);
            set_mtime(&p, 1_000 + *version as u64);
        }
    };
    let all: Vec<usize> = (0..n).collect();
    write_all(&repo, &mut rng, &mut version, &mut stamps, &all);
    sync(&cs, &[]);
    for cycle in 0..60 {
        if cycle % 2 == 0 {
            // Everything changed at once: one big batch.
            write_all(&repo, &mut rng, &mut version, &mut stamps, &all);
            let st = sync(&cs, &[]);
            assert_eq!(st.updated, n, "cycle {cycle}: {st:?}");
        } else {
            // Single-file saves, a few at a time, some files deleted or added.
            let some: Vec<usize> = (0..6).map(|_| rng.below(n)).collect();
            write_all(&repo, &mut rng, &mut version, &mut stamps, &some);
            for &i in &some {
                cs.refresh_file(&repo.p(&path(i)));
            }
            let gone = rng.below(n);
            let _ = fs::remove_file(repo.p(&path(gone)));
            cs.remove_file(&repo.p(&path(gone)));
            write_all(&repo, &mut rng, &mut version, &mut stamps, &[gone]);
            sync(&cs, &[]);
        }
        if cycle % 4 == 3 {
            let fresh = repo.cs();
            let fst = fresh.build_blocking(&[]).unwrap();
            let st = cs.status();
            assert_eq!(
                (st.files, st.chunks),
                (fst.total_files, fst.chunks),
                "cycle {cycle}"
            );
            let mut queries: Vec<String> = (0..8)
                .map(|_| format!("{} {}", rng.pick(VOCAB), rng.pick(VOCAB)))
                .collect();
            queries.push(stamps[stamps.len() - 1].clone());
            queries.push(stamps[stamps.len() / 2].clone());
            for q in &queries {
                assert_eq!(
                    serde_json::to_value(search(&cs, q)).unwrap(),
                    serde_json::to_value(search(&fresh, q)).unwrap(),
                    "cycle {cycle}: {q:?}"
                );
            }
        }
    }
    // Every file's newest stamp is findable and every replaced stamp is gone.
    let live = live_stamps(&repo.root);
    assert_eq!(live.len(), n);
    for (s, r) in &live {
        assert_eq!(find(&cs, s), std::slice::from_ref(r));
    }
    for s in stamps.iter().filter(|s| !live.contains_key(*s)).step_by(37) {
        assert!(find(&cs, s).is_empty(), "{s}");
    }
}

#[test]
fn the_size_caps_hold_under_random_churn() {
    for seed in 0..8u64 {
        let repo = Repo::new();
        let mut rng = Rng::new(seed + 5000);
        let max_files = 4 + rng.below(8);
        let max_bytes = 600 + rng.below(2000) as u64;
        let limits = SearchLimits {
            max_files,
            max_file_bytes: 1 << 20,
            max_total_bytes: max_bytes,
        };
        let cs = repo.cs_with(limits);
        let mut counter = 0usize;
        for step in 0..60 {
            let ctx =
                format!("seed {seed} step {step} (max_files {max_files}, max_bytes {max_bytes})");
            match rng.below(10) {
                0..=5 => {
                    counter += 1;
                    let rel = format!("d{}/f{}.rs", rng.below(3), rng.below(14));
                    let words = 5 + rng.below(40);
                    let mut text = format!("// stamp{}\n", letters(counter));
                    for _ in 0..words {
                        text.push_str(&format!("{} ", rng.pick(VOCAB)));
                    }
                    text.push('\n');
                    let p = repo.write(&rel, &text);
                    set_mtime(&p, counter as u64);
                }
                6..=7 => {
                    if let Some((rel, _)) = {
                        let files = walk_files(&repo.root);
                        (!files.is_empty()).then(|| files[rng.below(files.len())].clone())
                    } {
                        fs::remove_file(repo.p(&rel)).unwrap();
                    }
                }
                _ => {}
            }
            if rng.chance(50) {
                let files = walk_files(&repo.root);
                if !files.is_empty() {
                    cs.refresh_file(&files[rng.below(files.len())].1);
                }
            }
            if step % 3 == 2 {
                let st = sync(&cs, &[]);
                let files = walk_files(&repo.root);
                let eligible_bytes: u64 = files
                    .iter()
                    .map(|(_, p)| fs::metadata(p).unwrap().len())
                    .sum();
                assert_eq!(
                    st.truncated,
                    files.len() > max_files || eligible_bytes > max_bytes,
                    "{ctx}: {} files / {eligible_bytes} bytes on disk: {st:?}",
                    files.len()
                );
                assert_eq!(cs.status().truncated, st.truncated, "{ctx}");
                assert!(st.total_files <= max_files, "{ctx}: {st:?}");
                let live = live_stamps(&repo.root);
                let mut indexed_bytes = 0;
                let mut indexed_files = 0;
                for (s, r) in &live {
                    if !find(&cs, s).is_empty() {
                        indexed_files += 1;
                        indexed_bytes += fs::metadata(repo.p(r)).unwrap().len();
                    }
                }
                assert_eq!(indexed_files, st.total_files, "{ctx}");
                assert!(
                    indexed_bytes <= max_bytes,
                    "{ctx}: {indexed_bytes} bytes indexed"
                );
                if !st.truncated {
                    assert_eq!(
                        st.total_files,
                        files.len(),
                        "{ctx}: nothing is over a cap but files are missing"
                    );
                }
            }
        }
    }
}

/// A string of tricky characters: case-less scripts, combining marks, characters
/// whose lowercase is longer than themselves, emoji, zero-width and line
/// separators, digits of other scripts.
fn tricky_text(rng: &mut Rng, len: usize) -> String {
    const ALPHABET: &[&str] = &[
        "a",
        "b",
        "Z",
        "_",
        "_",
        " ",
        " ",
        "\n",
        "\n",
        "\r\n",
        "\t",
        "0",
        "7",
        "42",
        "-",
        ".",
        "(",
        ")",
        "İ",
        "ß",
        "ǅ",
        "ΐ",
        "é",
        "e\u{301}",
        "日",
        "本",
        "語",
        "ｆｕｌｌ",
        "²",
        "Ⅷ",
        "〇",
        "😀",
        "\u{200b}",
        "\u{feff}",
        "\u{2028}",
        "ע",
        "ب",
        "ᄀ",
        "ᅡ",
        "ﬁ",
        "Σ",
        "ς",
        "ñ",
        "Ñ",
        "ǆ",
        "Ǆ",
        "ꙮ",
        "\u{0301}",
        "fooBar",
        "HTTPServer",
        "x86_64",
        "0xdeadbeef",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ];
    (0..len).map(|_| *rng.pick(ALPHABET)).collect()
}

#[test]
fn arbitrary_unicode_text_and_names_never_break_indexing_or_snippets() {
    for seed in 0..6u64 {
        let repo = Repo::new();
        let mut rng = Rng::new(seed + 9000);
        let mut names = Vec::new();
        for i in 0..80 {
            let n = 1 + rng.below(6);
            let stem = tricky_text(&mut rng, n).replace(['\n', '\r', '\u{2028}'], "_");
            let rel = format!("fz{}/{i}{stem}.rs", i % 4);
            let n = rng.below(300);
            let body = tricky_text(&mut rng, n);
            // Some files get a pure-ASCII needle too, so queries have something to find.
            let body = format!("{body}\nneedle{}\n", letters(i % 9));
            repo.write(&rel, &body);
            names.push(rel);
        }
        let cs = repo.cs();
        let st = sync(&cs, &[]);
        assert_eq!(st.errors, 0, "seed {seed}: {st:?}");
        for k in 0..9 {
            let res = search(&cs, &format!("needle{}", letters(k)));
            assert!(
                !res.matches.is_empty(),
                "seed {seed}: needle{} not found",
                letters(k)
            );
        }
        for _ in 0..60 {
            let n = 1 + rng.below(8);
            let q = tricky_text(&mut rng, n);
            // Any outcome but a panic or a broken hit is fine.
            let res = cs
                .search_blocking(&q, 5)
                .unwrap_or_else(|e| panic!("seed {seed}: query {q:?}: {e}"));
            check_results(&cs, &res, 5);
        }
        // Edits keep working on such files.
        for rel in names.iter().take(10) {
            let p = repo.write(rel, &tricky_text(&mut rng, 100));
            cs.refresh_file(&p);
        }
        sync(&cs, &[]);
    }
}

#[test]
fn debug_output_is_available_while_a_build_runs() {
    let repo = Repo::new();
    for i in 0..1500 {
        repo.write(
            &format!("p{}/f{i}.rs", i % 15),
            &format!("fn debugprobe{}() {{}}\n", letters(i)),
        );
    }
    let cs = repo.cs();
    let builder = {
        let cs = cs.clone();
        std::thread::spawn(move || cs.build_blocking(&[]).unwrap())
    };
    let mut seen = 0;
    while !builder.is_finished() && seen < 200 {
        let text = format!("{cs:?} {:?}", cs.status());
        assert!(text.contains("CodeSearch"));
        seen += 1;
    }
    builder.join().unwrap();
}

#[test]
fn build_stats_describe_each_kind_of_change() {
    let repo = Repo::new();
    for i in 0..10 {
        repo.write(
            &format!("f{i}.rs"),
            &format!("fn statsoriginal{}() {{}}\n", letters(i)),
        );
    }
    let cs = repo.cs();
    let first = sync(&cs, &[]);
    assert_eq!(
        (first.indexed, first.unchanged, first.total_files),
        (10, 0, 10)
    );
    // two new, three edited, two deleted, one newly ignored, four untouched
    repo.write("new0.rs", "fn statsnewzero() {}\n");
    repo.write("new1.rs", "fn statsnewone() {}\n");
    for i in 0..3 {
        repo.write(
            &format!("f{i}.rs"),
            &format!("fn statsedited{}() {{}}\n// longer now\n", letters(i)),
        );
    }
    repo.remove("f3.rs");
    repo.remove("f4.rs");
    repo.write(".gitignore", "f5.rs\n");
    let st = sync(&cs, &[]);
    assert_eq!(
        (
            st.indexed,
            st.updated,
            st.removed,
            st.unchanged,
            st.total_files,
            st.skipped,
            st.errors
        ),
        (2, 3, 3, 4, 9, 0, 0),
        "{st:?}"
    );
}

#[test]
fn partial_builds_only_touch_their_scope_and_quietly_drop_vanished_paths() {
    let repo = Repo::new();
    repo.write("a/x.rs", "fn scopeax() {}\n");
    repo.write("a/y.rs", "fn scopeay() {}\n");
    repo.write("b/x.rs", "fn scopebx() {}\n");
    repo.write("b/y.rs", "fn scopeby() {}\n");
    let cs = repo.cs();
    sync(&cs, &[]);
    repo.remove("a/x.rs");
    repo.remove("b/x.rs");
    repo.write("a/z.rs", "fn scopeaz() {}\n");
    repo.write("b/z.rs", "fn scopebz() {}\n");
    let st = sync(&cs, &[repo.p("a")]);
    assert_eq!(
        (st.indexed, st.removed, st.unchanged, st.errors),
        (1, 1, 1, 0),
        "{st:?}"
    );
    assert_eq!(cs.status().files, 4, "b/ was not part of the build");
    // A path that vanished from disk but is still in the index is dropped without complaint.
    let st = sync(&cs, &[repo.p("b/x.rs")]);
    assert_eq!((st.removed, st.errors, st.indexed), (1, 0, 0), "{st:?}");
    assert!(st.warnings.is_empty(), "{st:?}");
    assert_eq!(cs.status().files, 3);
    let st = sync(&cs, &[repo.p("b")]);
    assert_eq!((st.indexed, st.unchanged), (1, 1), "{st:?}");
    assert_eq!(cs.status().files, 4);
    // (Searches only come now: a search looks at the whole disk, so one in
    // between would have done the partial builds' work for them.)
    assert_eq!(find(&cs, "scopeaz"), ["a/z.rs"]);
    assert_eq!(find(&cs, "scopebz"), ["b/z.rs"]);
    assert_eq!(
        find(&cs, "scopebx"),
        Vec::<String>::new(),
        "a vanished file is never served"
    );
}

#[test]
fn unknown_words_do_not_hide_matches_and_focused_text_beats_a_long_file() {
    let (_repo, cs) = mini_repo();
    for q in [
        "json config xyzzyunknownword",
        "xyzzyunknownword json config",
        "json qqqqzzzz config plughxy",
    ] {
        assert_eq!(top1(&cs, q).as_deref(), Some("src/config/loader.rs"), "{q}");
    }
    assert!(find(&cs, "xyzzyunknownword plughxy").is_empty());

    let repo = Repo::new();
    repo.write(
        "short/note.rs",
        "// the frobnicator lives here\nfn small() {}\n",
    );
    let mut long = String::new();
    for i in 0..30 {
        long.push_str(&format!("let value_{i} = compute({i});\n"));
    }
    long.push_str("// one mention of the frobnicator\n");
    for i in 30..60 {
        long.push_str(&format!("let value_{i} = compute({i});\n"));
    }
    repo.write("long/many.rs", &long);
    let cs = repo.cs();
    assert_eq!(find(&cs, "frobnicator"), ["short/note.rs", "long/many.rs"]);
}

#[test]
fn equal_chunks_of_one_file_report_the_first() {
    let repo = Repo::new();
    let block: String = (0..45)
        .map(|i| {
            if i == 20 {
                "let echoed = repeatedmarkerword();\n".to_string()
            } else {
                format!("let line{i} = {i};\n")
            }
        })
        .collect();
    repo.write("rep.rs", &format!("{block}\n{block}\n{block}"));
    let cs = repo.cs();
    let res = search(&cs, "repeatedmarkerword");
    assert_eq!(res.matches.len(), 1);
    assert!(covers(&res.matches[0], 21), "{:?}", res.matches[0]);
}

// A partial `build_blocking(paths)` does not count as the root being built: when the
// model's first call is `build_search_index {"paths": ["web"]}` (the tool description
// offers "to index a specific path") the first `code_search` still auto-builds the root,
// instead of ranking that folder alone with nothing telling the model that most of the
// workspace is missing.
#[test]
fn a_partial_build_before_the_first_search_does_not_stop_the_search_from_indexing_the_rest() {
    let (repo, cs) = mini_repo();
    let st = cs.build_blocking(&[repo.p("web")]).unwrap();
    assert_eq!(st.total_files, 3, "{st:?}");
    assert_eq!(
        top1(&cs, "tcp listener incoming connection").as_deref(),
        Some("src/network/server.rs"),
        "the search only sees the part of the workspace that was built by hand"
    );
}

#[test]
fn git_info_exclude_and_a_nested_repository_are_honoured() {
    let repo = Repo::new();
    repo.write(".git/HEAD", "ref: refs/heads/main\n");
    repo.write(".git/info/exclude", "excluded_locally.rs\n");
    repo.write(".git/objects/aa/bb.txt", "gitinternalsword\n");
    repo.write("excluded_locally.rs", "fn excludedlocallyword() {}\n");
    repo.write("kept.rs", "fn keptinrepoword() {}\n");
    // A nested repository (a submodule checkout) is just a directory with its own files.
    repo.write("vendor_free/sub/.git", "gitdir: ../../.git/modules/sub\n");
    repo.write("vendor_free/sub/lib.rs", "fn submoduleword() {}\n");
    let cs = repo.cs();
    sync(&cs, &[]);
    assert!(find(&cs, "excludedlocallyword").is_empty());
    assert!(find(&cs, "gitinternalsword").is_empty());
    assert_eq!(find(&cs, "keptinrepoword"), ["kept.rs"]);
    assert_eq!(find(&cs, "submoduleword"), ["vendor_free/sub/lib.rs"]);
}

// Linux only: macOS file systems refuse names that are not valid UTF-8.
#[cfg(target_os = "linux")]
#[test]
fn file_names_that_are_not_utf8_do_not_break_the_index() {
    use std::os::unix::ffi::OsStrExt;
    let repo = Repo::new();
    repo.write("ok.rs", "fn goodnameword() {}\n");
    let odd = repo
        .root
        .join(std::ffi::OsStr::from_bytes(b"latin1_caf\xe9.rs"));
    fs::write(&odd, "fn latinnameword() {}\n").unwrap();
    let cs = repo.cs();
    let st = cs.build_blocking(&[]).unwrap();
    assert_eq!(st.errors, 0, "{st:?}");
    assert_eq!(find(&cs, "goodnameword"), ["ok.rs"]);
    // The odd file may or may not be indexed; if it is, the hit must still be a valid result.
    let res = cs.search_blocking("latinnameword", 5).unwrap();
    assert!(res.matches.len() <= 1);
    cs.refresh_file(&odd);
    cs.remove_file(&odd);
    let again = cs.build_blocking(&[]).unwrap();
    assert_eq!(again.errors, 0, "{again:?}");
}

#[cfg(unix)]
#[test]
fn a_relative_root_is_resolved_against_the_working_directory() {
    let repo = Repo::new();
    repo.write("src/a.rs", "fn relativerootword() {}\n");
    let cwd = std::env::current_dir().unwrap();
    if cwd.canonicalize().ok().as_deref() != Some(cwd.as_path()) {
        eprintln!("the working directory is reached through a symlink; skipping");
        return;
    }
    let up = "../".repeat(cwd.components().count() - 1);
    let rel_root = PathBuf::from(up).join(repo.root.strip_prefix("/").unwrap());
    assert!(rel_root.is_relative());
    let cs = CodeSearch::new(rel_root);
    assert_eq!(cs.root(), repo.root);
    let res = search(&cs, "relativerootword");
    assert_eq!(res.matches.len(), 1);
    assert_eq!(
        Path::new(&res.matches[0].file),
        repo.p("src/a.rs").as_path()
    );
    cs.refresh_file(&repo.p("src/a.rs"));
    assert_eq!(cs.status().files, 1);
}

/// Opt-in soak run: `CODE_SEARCH_SPEC_EXTRA_SEEDS=500 cargo test -p phazeai-core
/// --test code_search_spec differential_soak` replays that many more seeds.
#[test]
fn differential_soak() {
    let Some(n) = std::env::var("CODE_SEARCH_SPEC_EXTRA_SEEDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return;
    };
    for seed in 1000..1000 + n {
        run_seed(seed, 120);
    }
}

#[test]
fn a_denied_file_is_judged_by_its_name_before_anything_else_about_it_is_looked_at() {
    let _active = ActiveProject::none();
    let repo = Repo::new();
    // Over the size limit *and* denied: it must count as a privacy skip, which
    // shows the policy was applied before the size check (let alone a read).
    repo.write(
        "credentials.json",
        &format!("{{\"k\": \"{}\"}}\n", "x".repeat(2 << 20)),
    );
    repo.write(
        "big_but_fine.json",
        &format!("{{\"k\": \"{}\"}}\n", "x".repeat(2 << 20)),
    );
    repo.write("ok.rs", "fn okfileword() {}\n");
    let cs = repo.cs();
    let st = sync(&cs, &[]);
    assert_eq!(
        (st.privacy_skipped, st.skipped, st.indexed),
        (1, 1, 1),
        "{st:?}"
    );
}
