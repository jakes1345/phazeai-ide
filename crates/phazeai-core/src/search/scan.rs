//! Finding files, reading them and turning them into chunks.

use super::chunk;
use super::index::{Parsed, ParsedChunk, ParsedFile, Stamp};
use super::paths::path_starts_with;
use super::tokenize::Tokenizer;
use super::types::SearchLimits;
use crate::privacy::PrivacyPolicy;
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Directories never indexed (matched by name, at any depth). `.phazeai` is
/// the IDE's own state (generated design prototypes, the privacy policy).
const SKIP_DIRS: &[&str] = &[
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
];

/// Hidden files that are configuration worth searching; other dotfiles are skipped.
const WELL_KNOWN_HIDDEN: &[&str] = &[
    ".gitlab-ci.yml",
    ".travis.yml",
    ".eslintrc.json",
    ".eslintrc.yml",
    ".prettierrc.json",
    ".prettierrc.yml",
    ".rustfmt.toml",
    ".clippy.toml",
    ".golangci.yml",
    ".golangci.yaml",
    ".pre-commit-config.yaml",
];

/// Source-ish files without an extension (compared case-insensitively).
const EXTENSIONLESS: &[&str] = &["makefile", "dockerfile"];

/// Extra term frequency a path word (file stem or directory name) gets in
/// every chunk of its file, so `lsp manager` finds `lsp/manager.rs`.
const PATH_TF: u32 = 3;

/// A file read this long is binary if a NUL shows up in its first bytes.
const BINARY_PROBE: usize = 8192;

/// Files per commit batch of the build pipeline.
pub(crate) const COMMIT_GROUP: usize = 64;

fn is_source_ext(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "go" | "java" | "c" | "cpp" | "cc" | "cxx"
            | "h" | "hpp" | "md" | "toml" | "json" | "yaml" | "yml" | "txt"
            // Beyond the original Python list.
            | "kt" | "kts" | "swift" | "cs" | "rb" | "php" | "lua" | "sh" | "bash" | "zsh"
            | "ps1" | "sql" | "html" | "css" | "scss" | "gd" | "gdshader" | "shader" | "wgsl"
            | "glsl" | "hlsl" | "cu" | "m" | "mm" | "scala" | "clj" | "ex" | "exs" | "erl"
            | "hs" | "ml" | "r" | "jl" | "dart" | "vue" | "svelte" | "xml" | "gradle"
            | "cmake" | "proto" | "graphql"
    )
}

/// Whether a file with this name is worth indexing (before looking inside it).
pub(crate) fn is_indexable_name(name: &str) -> bool {
    if name.starts_with('.')
        && !WELL_KNOWN_HIDDEN
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
    {
        return false;
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => is_source_ext(&ext.to_ascii_lowercase()),
        _ => EXTENSIONLESS.iter().any(|n| n.eq_ignore_ascii_case(name)),
    }
}

fn is_skipped_dir(name: &str) -> bool {
    SKIP_DIRS.iter().any(|d| {
        if cfg!(windows) {
            d.eq_ignore_ascii_case(name)
        } else {
            *d == name
        }
    })
}

pub(crate) struct Discovery {
    /// Absolute paths of indexable regular files inside the scope.
    pub files: Vec<PathBuf>,
    /// Directories the privacy policy denies (files are checked later, in [`plan`]).
    pub denied_dirs: usize,
    pub errors: usize,
}

/// Walk `root` down to `scope` (a directory or a file inside `root`) and list
/// the files the index would hold. One walker serves whole-tree builds and
/// single-file refreshes, so both apply exactly the same rules: `.gitignore` /
/// `.ignore` (also without a `.git` directory), [`SKIP_DIRS`], hidden files,
/// the extension allowlist, no symlinks, and the privacy policy for
/// directories. Directories outside the path from `root` to `scope` are not
/// entered.
pub(crate) fn discover(root: &Path, scope: &Path, policy: &Arc<PrivacyPolicy>) -> Discovery {
    let denied = Arc::new(AtomicUsize::new(0));
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .require_git(false);
    {
        let policy = Arc::clone(policy);
        let denied = Arc::clone(&denied);
        let scope = scope.to_path_buf();
        builder.filter_entry(move |e| {
            if e.depth() == 0 {
                return true;
            }
            let path = e.path();
            let inside = path_starts_with(path, &scope);
            if !inside && !path_starts_with(&scope, path) {
                return false;
            }
            let Some(ft) = e.file_type() else {
                return false;
            };
            if ft.is_dir() {
                if e.file_name().to_str().is_some_and(is_skipped_dir) {
                    return false;
                }
                if policy.is_denied(path) {
                    denied.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                return true;
            }
            inside && ft.is_file() && e.file_name().to_str().is_some_and(is_indexable_name)
        });
    }
    let mut files = Vec::new();
    let mut errors = 0;
    for entry in builder.build() {
        match entry {
            Ok(e) if e.depth() > 0 && e.file_type().is_some_and(|t| t.is_file()) => {
                files.push(e.into_path());
            }
            Ok(_) => {}
            // Malformed ignore-file lines are reported as errors too; only
            // real I/O trouble (unreadable directories) counts.
            Err(err) => {
                if err.io_error().is_some() {
                    errors += 1;
                    tracing::debug!("code search walk: {err}");
                }
            }
        }
    }
    Discovery {
        files,
        denied_dirs: denied.load(Ordering::Relaxed),
        errors,
    }
}

/// A file that passed every check that does not need its content.
pub(crate) struct Planned {
    pub path: PathBuf,
    pub stamp: Stamp,
}

pub(crate) struct Plan {
    pub files: Vec<Planned>,
    pub privacy_skipped: usize,
    pub skipped: usize,
    pub errors: usize,
}

/// Apply the privacy policy and the size limit to discovered files and read
/// their (mtime, size). Denied files are never opened.
pub(crate) fn plan(files: &[PathBuf], policy: &PrivacyPolicy, limits: &SearchLimits) -> Plan {
    enum Outcome {
        Keep(Planned),
        Denied,
        TooLarge,
        Error,
        Gone,
    }
    let outcomes = parallel_map(files, |path| {
        if policy.is_denied(path) {
            return Outcome::Denied;
        }
        match std::fs::metadata(path) {
            Ok(m) if m.is_file() => {
                if m.len() > limits.max_file_bytes {
                    Outcome::TooLarge
                } else {
                    Outcome::Keep(Planned {
                        path: path.clone(),
                        stamp: Stamp::of(&m),
                    })
                }
            }
            Ok(_) => Outcome::Gone,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Outcome::Gone,
            Err(_) => Outcome::Error,
        }
    });
    let mut plan = Plan {
        files: Vec::with_capacity(files.len()),
        privacy_skipped: 0,
        skipped: 0,
        errors: 0,
    };
    for o in outcomes {
        match o {
            Outcome::Keep(p) => plan.files.push(p),
            Outcome::Denied => plan.privacy_skipped += 1,
            Outcome::TooLarge => plan.skipped += 1,
            Outcome::Error => plan.errors += 1,
            Outcome::Gone => {}
        }
    }
    plan
}

/// Worker threads for parallel stages: all cores but one (the IDE's UI thread
/// or the caller keeps running), at most 8.
pub(crate) fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .saturating_sub(1)
        .clamp(1, 8)
}

/// `items.iter().map(f).collect()` on up to [`worker_count`] threads, in input order.
pub(crate) fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = worker_count().min(items.len());
    if workers <= 1 || items.len() < 64 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, R)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        out.push((i, f(item)));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });
    done.sort_unstable_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, r)| r).collect()
}

/// Words of a file's location relative to `root`: directory names and the
/// file name without its extension, split like identifiers, each once.
pub(crate) fn path_terms(path: &Path, root: &Path) -> Vec<String> {
    let name_only;
    let rel = match path.strip_prefix(root) {
        Ok(r) if !r.as_os_str().is_empty() => r,
        _ => {
            name_only = PathBuf::from(path.file_name().unwrap_or_default());
            &name_only
        }
    };
    let names: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut tk = Tokenizer::new();
    for (i, name) in names.iter().enumerate() {
        let piece = if i + 1 == names.len() {
            match name.rfind('.') {
                Some(dot) if dot > 0 => &name[..dot],
                _ => name.as_str(),
            }
        } else {
            name.as_str()
        };
        tk.run(piece, |t| {
            if !out.iter().any(|x| x == t) {
                out.push(t.to_owned());
            }
        });
    }
    out
}

/// Chunk `text` and count the terms of every chunk. Path words are added to
/// each chunk with [`PATH_TF`] extra occurrences and one extra token of length.
pub(crate) fn parse_text(text: &str, path_terms: &[String]) -> Vec<ParsedChunk> {
    let lines: Vec<&str> = text.lines().collect();
    let mut tk = Tokenizer::new();
    chunk::split(&lines)
        .into_iter()
        .map(|span| {
            let mut counts: HashMap<String, u32> = HashMap::new();
            let mut len = 0u32;
            for line in &lines[span.start - 1..span.end] {
                tk.run(line, |t| {
                    len += 1;
                    match counts.get_mut(t) {
                        Some(c) => *c += 1,
                        None => {
                            counts.insert(t.to_owned(), 1);
                        }
                    }
                });
            }
            for t in path_terms {
                *counts.entry(t.clone()).or_insert(0) += PATH_TF;
            }
            len += path_terms.len() as u32;
            ParsedChunk {
                start: span.start as u32,
                end: span.end as u32,
                len,
                terms: counts.into_iter().collect(),
            }
        })
        .collect()
}

/// Read and chunk one file. The stamp is taken from the open handle *before*
/// reading, so a write that races with the read leaves the index stamp older
/// than the file and the next sync re-reads it.
pub(crate) fn parse_file(path: &Path, root: &Path, limits: &SearchLimits) -> Parsed {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Parsed::Gone(path.to_path_buf())
        }
        Err(_) => return Parsed::Failed(path.to_path_buf()),
    };
    let Ok(meta) = file.metadata() else {
        return Parsed::Failed(path.to_path_buf());
    };
    if !meta.is_file() {
        return Parsed::Gone(path.to_path_buf());
    }
    if meta.len() > limits.max_file_bytes {
        return Parsed::Skipped(path.to_path_buf());
    }
    let stamp = Stamp::of(&meta);
    // One byte past the limit shows that the file grew since the stat.
    let mut reader = file.by_ref().take(limits.max_file_bytes.saturating_add(1));
    let mut bytes = Vec::with_capacity(meta.len() as usize + 1);
    // A binary file is recognised by its first bytes, before the rest is read.
    if reader
        .by_ref()
        .take(BINARY_PROBE as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Parsed::Failed(path.to_path_buf());
    }
    if bytes.contains(&0) {
        return Parsed::Skipped(path.to_path_buf());
    }
    if reader.read_to_end(&mut bytes).is_err() {
        return Parsed::Failed(path.to_path_buf());
    }
    if bytes.len() as u64 > limits.max_file_bytes {
        return Parsed::Skipped(path.to_path_buf());
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Parsed::Skipped(path.to_path_buf());
    };
    Parsed::File(ParsedFile {
        path: path.to_path_buf(),
        stamp,
        chunks: parse_text(&text, &path_terms(path, root)),
    })
}

/// The text of `path` if it still matches the stamp it was indexed with, read
/// consistently (the file did not change while it was read). `None` means the
/// file changed, vanished or is no longer readable text.
pub(crate) fn read_checked(path: &Path, expect: Stamp, max_bytes: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || Stamp::of(&meta) != expect {
        return None;
    }
    let mut text = String::with_capacity(meta.len() as usize);
    file.by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > max_bytes || Stamp::of(&file.metadata().ok()?) != expect {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_allowlist() {
        for ok in [
            "main.rs",
            "a.PY",
            "x.tsx",
            "build.gradle",
            "shader.wgsl",
            "Makefile",
            "dockerfile",
            "CMakeLists.txt",
            "App.vue",
            "q.graphql",
            "notes.MD",
            "a.b.c.ts",
            ".gitlab-ci.yml",
            ".eslintrc.json",
        ] {
            assert!(is_indexable_name(ok), "{ok}");
        }
        for no in [
            "image.png",
            "lib.so",
            "Cargo.lock",
            ".env",
            ".gitignore",
            ".hidden.rs",
            "noext",
            "archive.tar.gz",
            "x.",
            ".rs",
            "Makefile.bak",
        ] {
            assert!(!is_indexable_name(no), "{no}");
        }
    }

    #[test]
    fn path_terms_use_directories_and_the_file_stem() {
        let root = Path::new("/w");
        assert_eq!(
            path_terms(Path::new("/w/crates/phazeai-core/src/lsp/manager.rs"), root),
            ["crates", "phazeai", "core", "src", "lsp", "manager"]
        );
        assert_eq!(
            path_terms(Path::new("/w/ui/lspBridge.test.ts"), root),
            ["ui", "lspbridge", "lsp", "bridge", "test"]
        );
        assert_eq!(path_terms(Path::new("/w/Makefile"), root), ["makefile"]);
        // Outside the root only the file name counts; one-letter pieces are dropped.
        assert_eq!(path_terms(Path::new("/elsewhere/a_b.rs"), root), ["a_b"]);
    }

    #[test]
    fn parse_text_counts_terms_and_adds_path_words_to_every_chunk() {
        let mut text = String::new();
        for i in 0..100 {
            text.push_str(&format!("let value{i} = parseJsonConfig(path);\n"));
        }
        let chunks = parse_text(&text, &["manager".to_string()]);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            let get = |t: &str| c.terms.iter().find(|(x, _)| x == t).map(|(_, n)| *n);
            assert_eq!(get("manager"), Some(PATH_TF));
            let lines = c.end - c.start + 1;
            assert_eq!(get("parse"), Some(lines));
            assert_eq!(get("parsejsonconfig"), Some(lines));
            // Per line: let, value<i>, value (piece, once per distinct), parsejsonconfig,
            // parse, json, config, path; +1 for the path word.
            assert!(c.len > lines * 6);
        }
    }

    fn parse(dir: &Path, name: &str, bytes: &[u8], limits: &SearchLimits) -> Parsed {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        parse_file(&path, dir, limits)
    }

    #[test]
    fn parse_file_accepts_text_and_refuses_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let limits = SearchLimits {
            max_file_bytes: 20_000,
            ..SearchLimits::default()
        };
        let d = dir.path();
        match parse(d, "ok.rs", b"fn main() {}\n", &limits) {
            Parsed::File(f) => {
                assert_eq!(f.stamp.size, 13);
                assert_eq!(f.chunks.len(), 1);
            }
            _ => panic!("a text file is parsed"),
        }
        assert!(
            matches!(parse(d, "empty.rs", b"", &limits), Parsed::File(f) if f.chunks.is_empty())
        );
        assert!(matches!(
            parse(d, "nul.rs", b"fn a\0b", &limits),
            Parsed::Skipped(_)
        ));
        // A binary file is refused from its first bytes, wherever its size is.
        let mut blob = vec![0u8; 1];
        blob.extend(std::iter::repeat_n(b'x', 19_000));
        assert!(matches!(
            parse(d, "blob.rs", &blob, &limits),
            Parsed::Skipped(_)
        ));
        assert!(matches!(
            parse(d, "latin1.rs", b"caf\xe9", &limits),
            Parsed::Skipped(_)
        ));
        let big = vec![b'x'; 20_001];
        assert!(matches!(
            parse(d, "big.rs", &big, &limits),
            Parsed::Skipped(_)
        ));
        let exact = vec![b'x'; 20_000];
        assert!(matches!(
            parse(d, "exact.rs", &exact, &limits),
            Parsed::File(_)
        ));
        assert!(matches!(
            parse_file(&d.join("missing.rs"), d, &limits),
            Parsed::Gone(_)
        ));
        // The limit may be anything, including "no limit".
        let unlimited = SearchLimits {
            max_file_bytes: u64::MAX,
            ..SearchLimits::default()
        };
        assert!(matches!(
            parse(d, "any.rs", b"fn x() {}", &unlimited),
            Parsed::File(_)
        ));
        assert!(read_checked(
            &d.join("any.rs"),
            Stamp::of(&std::fs::metadata(d.join("any.rs")).unwrap()),
            u64::MAX
        )
        .is_some());
    }

    #[test]
    fn parallel_map_keeps_order() {
        let items: Vec<usize> = (0..1000).collect();
        let doubled = parallel_map(&items, |&i| i * 2);
        assert_eq!(doubled, (0..1000).map(|i| i * 2).collect::<Vec<_>>());
        assert_eq!(parallel_map(&items[..3], |&i| i + 1), [1, 2, 3]);
        assert!(parallel_map(&items[..0], |&i| i).is_empty());
    }
}
