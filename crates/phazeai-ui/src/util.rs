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
