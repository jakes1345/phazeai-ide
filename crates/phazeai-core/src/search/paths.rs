//! Path helpers shared by the search engine.
//!
//! Index keys and results use plain absolute paths: no `canonicalize` (it
//! resolves symlinks, which would change the paths users see, and returns
//! `\\?\` verbatim paths on Windows) and no cwd-relative strings.

use std::path::{Component, Path, PathBuf};

/// `path` made absolute (relative paths resolve against the process cwd) with
/// `.` and `..` resolved lexically and any Windows verbatim prefix removed.
pub(crate) fn absolutize(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    strip_verbatim(normalize(&abs))
}

/// Resolve `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `\\?\C:\x` -> `C:\x` and `\\?\UNC\srv\share` -> `\\srv\share`; `None` when
/// `s` has no verbatim prefix that can be dropped without changing its meaning.
pub(crate) fn strip_verbatim_prefix(s: &str) -> Option<String> {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    let rest = s.strip_prefix(r"\\?\")?;
    let b = rest.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':').then(|| rest.to_string())
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    if cfg!(windows) {
        if let Some(plain) = path.to_str().and_then(strip_verbatim_prefix) {
            return PathBuf::from(plain);
        }
    }
    path
}

/// Map key for `path`: Windows file systems are case-insensitive, so two
/// spellings of one file must not become two index entries.
pub(crate) fn path_key(path: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    } else {
        path.to_path_buf()
    }
}

/// Component-wise `path.starts_with(base)` with the same case rules as [`path_key`].
pub(crate) fn path_starts_with(path: &Path, base: &Path) -> bool {
    if cfg!(windows) {
        path_key(path).starts_with(path_key(base))
    } else {
        path.starts_with(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_are_resolved_lexically() {
        let base = std::env::temp_dir();
        let messy = base.join("a").join(".").join("b").join("..").join("c.rs");
        assert_eq!(absolutize(&messy), base.join("a").join("c.rs"));
        assert!(absolutize(Path::new("rel/x.rs")).is_absolute());
    }

    #[test]
    fn parent_of_root_stays_at_root() {
        let root = absolutize(Path::new("/"));
        assert_eq!(absolutize(&root.join("..").join("..")), root);
    }

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(
            strip_verbatim_prefix(r"\\?\C:\Users\me\proj").as_deref(),
            Some(r"C:\Users\me\proj")
        );
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\server\share\proj").as_deref(),
            Some(r"\\server\share\proj")
        );
        assert_eq!(strip_verbatim_prefix(r"C:\plain"), None);
        assert_eq!(strip_verbatim_prefix(r"\\?\Volume{1234}\x"), None);
        assert_eq!(strip_verbatim_prefix("/usr/bin"), None);
    }

    #[test]
    fn starts_with_is_component_wise() {
        assert!(path_starts_with(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(!path_starts_with(Path::new("/a/bc"), Path::new("/a/b")));
    }
}
