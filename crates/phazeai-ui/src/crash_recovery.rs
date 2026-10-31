use std::path::{Path, PathBuf};

const HEADER_PREFIX: &str = "// PHAZEAI-RECOVERY: ";

pub fn recovery_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(dir) = test_support::dir_override() {
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("phazeai")
        .join("crash-recovery");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn recovery_path_for(original: &Path) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    original.hash(&mut h);
    let hash = h.finish();
    let ext = original
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("txt");
    recovery_dir().join(format!("{:016x}.{}", hash, ext))
}

/// Write content to the crash-recovery shadow file for `original`.
/// Safe to call from a background thread.
pub fn write_recovery(original: &Path, content: &str) {
    let rp = recovery_path_for(original);
    let data = format!("{}{}\n{}", HEADER_PREFIX, original.display(), content);
    let _ = std::fs::write(&rp, data);
}

/// Remove the recovery shadow file for `original` (call after clean save).
pub fn clear_recovery(original: &Path) {
    let rp = recovery_path_for(original);
    let _ = std::fs::remove_file(rp);
}

/// Return the original file paths for any pending crash-recovery files.
/// A file is considered pending if the recovery content differs from disk.
pub fn list_pending() -> Vec<PathBuf> {
    let dir = recovery_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut pending = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(rec_content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Some(first_line) = rec_content.lines().next() else {
            continue;
        };
        let Some(original_str) = first_line.strip_prefix(HEADER_PREFIX) else {
            continue;
        };
        let original = PathBuf::from(original_str);
        let recovery_body: String = rec_content.lines().skip(1).collect::<Vec<_>>().join("\n");
        // Only report if original file differs (or doesn't exist)
        let on_disk = std::fs::read_to_string(&original).unwrap_or_default();
        if on_disk != recovery_body {
            pending.push(original);
        } else {
            // Already matches — stale recovery file, clean it up
            let _ = std::fs::remove_file(entry.path());
        }
    }
    pending
}

/// Lets a test keep recovery files in its own directory instead of the real config dir.
#[cfg(test)]
pub(crate) mod test_support {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    thread_local! {
        static DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    pub(super) fn dir_override() -> Option<PathBuf> {
        DIR.with(|d| d.borrow().clone())
    }

    /// Restores the real directory for this thread when dropped.
    pub(crate) struct DirGuard;

    impl Drop for DirGuard {
        fn drop(&mut self) {
            DIR.with(|d| *d.borrow_mut() = None);
        }
    }

    /// For the rest of this test (on this thread), recovery files go in `dir`.
    pub(crate) fn use_dir(dir: &Path) -> DirGuard {
        DIR.with(|d| *d.borrow_mut() = Some(dir.to_path_buf()));
        DirGuard
    }

    /// Does the recovery shadow of `original` exist?
    pub(crate) fn shadow_exists(original: &Path) -> bool {
        super::recovery_path_for(original).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_is_written_listed_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = test_support::use_dir(&dir.path().join("recovery"));
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "on disk\n").unwrap();

        assert!(!test_support::shadow_exists(&file));
        write_recovery(&file, "unsaved edit");
        assert!(test_support::shadow_exists(&file));
        assert_eq!(list_pending(), vec![file.clone()]);

        clear_recovery(&file);
        assert!(!test_support::shadow_exists(&file));
        assert!(list_pending().is_empty());
    }
}
