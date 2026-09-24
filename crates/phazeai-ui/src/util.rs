use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate};

/// Safely read a reactive signal that may have been disposed (e.g. from a
/// removed `dyn_stack` item).  Returns `default` if the signal's scope is gone.
///
/// Floem's `SignalGet::get()` panics when the signal's scope has been disposed,
/// which happens when a `dyn_stack` item is removed but its style/label closures
/// fire one last time during the same event cycle.  This wraps the read in
/// `catch_unwind` so those stale reads return a safe default instead of crashing.
pub fn safe_get<T: Clone + 'static>(sig: RwSignal<T>, default: T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sig.get())).unwrap_or(default)
}

/// Same as [`safe_get`] but for derived memos (`Memo<T>`).
pub fn safe_get_memo<T: Clone + 'static>(memo: Memo<T>, default: T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| memo.get())).unwrap_or(default)
}

/// Best-effort open a URL or file in the system browser / file handler.
pub fn open_external_url(url: &str) -> Result<(), String> {
    use std::process::Command;
    if cfg!(target_os = "linux") {
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if cfg!(target_os = "macos") {
        Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        Err("unsupported platform".to_string())
    }
}

/// Append a line to the shared debug console (UI thread).
pub fn append_debug_console(log: RwSignal<String>, msg: impl AsRef<str>) {
    let piece = msg.as_ref();
    log.update(|buf| {
        if !buf.is_empty() && !buf.ends_with('\n') {
            buf.push('\n');
        }
        buf.push_str(piece);
        if !buf.ends_with('\n') {
            buf.push('\n');
        }
    });
}

/// POSIX single-quote for `sh -c` fragments (`'...'` with `'` escaped as `'\''`).
pub fn shell_quote_single(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// Join shell words with minimal quoting (single-quote if spaces).
pub fn snapshot_listening_ports() -> Result<Vec<String>, String> {
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let out = Command::new("ss")
            .args(["-ltnH"])
            .output()
            .map_err(|e| format!("ss: {}", e))?;
        if !out.status.success() {
            return Err("ss exited with non-zero status".into());
        }
        let txt = String::from_utf8_lossy(&out.stdout);
        let mut lines = Vec::new();
        for line in txt.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            lines.push(t.to_string());
        }
        if lines.is_empty() {
            return Ok(vec!["(no listening TCP sockets reported)".into()]);
        }
        Ok(lines)
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let out = Command::new("lsof")
            .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
            .output()
            .map_err(|e| format!("lsof: {}", e))?;
        if !out.status.success() {
            return Err("lsof failed".into());
        }
        let txt = String::from_utf8_lossy(&out.stdout);
        let parsed: Vec<String> = txt
            .lines()
            .skip(1)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .take(128)
            .collect();
        if parsed.is_empty() {
            Ok(vec!["(no listening TCP sockets reported)".into()])
        } else {
            Ok(parsed)
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err("Port listing supports Linux (ss) and macOS (lsof) today.".into())
    }
}

pub fn shell_join_args(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| {
            if p.is_empty()
                || p.chars()
                    .any(|c| c.is_whitespace() || matches!(c, '|' | '&' | ';' | '<' | '>' | '$'))
            {
                shell_quote_single(p)
            } else {
                p.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Files larger than this are refused by the editor rather than loaded into
/// a text buffer (and later auto-saved over).
pub const MAX_EDITOR_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// Load a file for editing. A missing file yields an empty buffer (new file).
/// Anything we cannot faithfully round-trip — unreadable, binary, non-UTF-8,
/// or huge — is an error, so the caller never saves a lossy buffer back over
/// the user's data.
pub fn load_text_file(path: &std::path::Path) -> Result<String, String> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(format!("cannot read file: {e}")),
    };
    if meta.is_dir() {
        return Err("path is a directory".into());
    }
    if meta.len() > MAX_EDITOR_FILE_BYTES {
        return Err(format!(
            "file is too large to edit ({} MiB, limit {} MiB)",
            meta.len() / (1024 * 1024),
            MAX_EDITOR_FILE_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read file: {e}"))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err("file looks binary".into());
    }
    String::from_utf8(bytes).map_err(|_| "file is not valid UTF-8 text".into())
}

/// Write `content` to `path` atomically: write a sibling temp file, fsync it,
/// then rename over the target. A crash or full disk mid-save leaves the
/// original file intact. Symlinks are followed so the link itself survives,
/// and the original file's permissions are preserved.
pub fn write_file_atomic(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(content)?;
    tmp.as_file().sync_all()?;
    if let Ok(meta) = std::fs::metadata(&target) {
        let _ = std::fs::set_permissions(tmp.path(), meta.permissions());
    }
    tmp.persist(&target).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod file_io_tests {
    use super::*;

    #[test]
    fn load_rejects_binary_and_invalid_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("a.bin");
        std::fs::write(&bin, [0x7f, b'E', b'L', b'F', 0, 0, 1]).unwrap();
        assert!(load_text_file(&bin).is_err());
        let latin1 = dir.path().join("b.txt");
        std::fs::write(&latin1, [b'c', b'a', b'f', 0xe9]).unwrap();
        assert!(load_text_file(&latin1).is_err());
        assert_eq!(load_text_file(&dir.path().join("new.rs")).unwrap(), "");
    }

    #[test]
    fn atomic_write_replaces_content_and_keeps_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        std::fs::write(&real, "old").unwrap();
        write_file_atomic(&real, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        #[cfg(unix)]
        {
            let link = dir.path().join("link.txt");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            write_file_atomic(&link, b"via link").unwrap();
            assert!(std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(std::fs::read_to_string(&real).unwrap(), "via link");
        }
    }
}
