use std::path::{Path, PathBuf};

const HEADER_PREFIX: &str = "// PHAZEAI-RECOVERY: ";

pub fn recovery_dir() -> PathBuf {
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
