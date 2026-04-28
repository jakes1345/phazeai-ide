//! Workspace-root sandboxing for filesystem-touching tools.
//!
//! When the host (IDE/CLI) sets a workspace root via [`set_workspace_root`], every
//! file-modifying tool (`bash`, `edit_file`, `write_file`, `download`, `move_path`,
//! `copy_path`, `create_directory`, `delete_path`) calls [`resolve_within_workspace`]
//! to canonicalize the input path and refuse anything outside the root, including
//! symlink escapes.
//!
//! When no workspace root is set the resolver permits any path (backwards-compatible
//! with raw library use), but every tool still consults [`is_protected_system_path`]
//! to refuse OS-critical locations regardless of workspace state.

use crate::error::PhazeError;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// OS paths that no tool may ever touch, even with no workspace configured.
/// Extends the original `delete_path` list with /srv /mnt /media /root and
/// the modern `/usr/bin`-style symlink targets used on Arch/Fedora/RHEL where
/// `/bin`, `/sbin`, `/lib`, `/lib64` are symlinks into /usr.
pub const PROTECTED_SYSTEM_PATHS: &[&str] = &[
    "/",
    "/home",
    "/usr",
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/lib64",
    "/usr/local",
    "/bin",
    "/sbin",
    "/etc",
    "/var",
    "/tmp",
    "/boot",
    "/dev",
    "/proc",
    "/sys",
    "/lib",
    "/lib64",
    "/opt",
    "/srv",
    "/mnt",
    "/media",
    "/root",
];

static WORKSPACE_ROOT: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Set the workspace root used by all sandboxed tool calls. Call once at app
/// startup with the canonical path of the project root. Subsequent calls
/// overwrite. Pass `None` to disable sandboxing (matches default).
pub fn set_workspace_root(root: Option<PathBuf>) {
    let canonical = root.and_then(|p| p.canonicalize().ok());
    if let Ok(mut guard) = WORKSPACE_ROOT.write() {
        *guard = canonical;
    }
}

/// Current workspace root, if configured.
pub fn workspace_root() -> Option<PathBuf> {
    WORKSPACE_ROOT.read().ok().and_then(|g| g.clone())
}

/// Returns true if `canonical` matches any path in [`PROTECTED_SYSTEM_PATHS`]
/// or is the user's home directory itself.
pub fn is_protected_system_path(canonical: &Path) -> bool {
    let s = canonical.to_string_lossy();
    if PROTECTED_SYSTEM_PATHS.iter().any(|p| s.as_ref() == *p) {
        return true;
    }
    if let Some(home) = dirs::home_dir() {
        if canonical == home {
            return true;
        }
    }
    false
}

/// Resolve `input` (absolute or relative) into a canonical [`PathBuf`] and
/// verify it lies inside the configured workspace root, if any.
///
/// - If the path exists, the OS canonicalises it (resolves symlinks).
/// - If the path does not exist, the **parent** is canonicalised and joined
///   with the original final component, so creation tools (`write_file`,
///   `download`, `create_directory`) can target new files without bypassing
///   the symlink-escape check.
/// - When no workspace root is configured the path is returned as-is (after
///   the protected-system-paths check) so library users without an IDE host
///   keep their current behaviour.
///
/// On rejection returns a `PhazeError::tool(tool_name, ...)` so the caller can
/// propagate the error verbatim.
pub fn resolve_within_workspace(tool_name: &str, input: &str) -> Result<PathBuf, PhazeError> {
    if input.is_empty() {
        return Err(PhazeError::tool(tool_name, "path is empty"));
    }

    let raw = Path::new(input);

    // Defense in depth: refuse the textual form before canonicalisation, so a
    // symlink like /bin -> /usr/bin doesn't bypass the protected list when /bin
    // canonicalises away. We also check after canonicalisation below.
    if is_protected_system_path(raw) {
        return Err(PhazeError::tool(
            tool_name,
            format!("REFUSED: '{}' is a protected system path", input),
        ));
    }

    // Resolve to a canonical absolute path. For non-existent paths we
    // canonicalise the closest existing ancestor and re-append the rest,
    // which still defeats `..` and symlink escapes from the parent.
    let canonical = canonicalize_or_parent(raw).map_err(|e| {
        PhazeError::tool(tool_name, format!("cannot resolve path '{}': {}", input, e))
    })?;

    if is_protected_system_path(&canonical) {
        return Err(PhazeError::tool(
            tool_name,
            format!(
                "REFUSED: '{}' resolves to a protected system path",
                canonical.display()
            ),
        ));
    }

    if let Some(root) = workspace_root() {
        if !canonical.starts_with(&root) {
            return Err(PhazeError::tool(
                tool_name,
                format!(
                    "REFUSED: '{}' is outside the workspace root '{}'",
                    canonical.display(),
                    root.display()
                ),
            ));
        }
    }

    Ok(canonical)
}

/// Same as [`resolve_within_workspace`] but for tools that accept directories
/// they will create. The directory itself need not exist; we canonicalise its
/// nearest existing ancestor.
pub fn resolve_target_path(tool_name: &str, input: &str) -> Result<PathBuf, PhazeError> {
    resolve_within_workspace(tool_name, input)
}

fn canonicalize_or_parent(p: &Path) -> std::io::Result<PathBuf> {
    if let Ok(c) = p.canonicalize() {
        return Ok(c);
    }
    // Walk up until we find an existing ancestor, then re-append the descendant.
    let mut existing: PathBuf = PathBuf::from(".");
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let abs: PathBuf = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    let mut cursor: PathBuf = abs.clone();
    loop {
        if cursor.exists() {
            existing = cursor.clone();
            break;
        }
        let name = cursor.file_name().map(|n| n.to_owned());
        match name {
            Some(name) => {
                tail.push(name);
                if !cursor.pop() {
                    break;
                }
            }
            None => break,
        }
    }
    let canonical_existing = existing.canonicalize()?;
    let mut out = canonical_existing;
    for seg in tail.into_iter().rev() {
        out.push(seg);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Sandbox tests mutate a process-global, so they must run serially even
    // though Cargo runs `#[test]` functions in parallel by default.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn with_workspace<F: FnOnce()>(root: Option<PathBuf>, f: F) {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let prev = workspace_root();
        set_workspace_root(root);
        f();
        set_workspace_root(prev);
    }

    #[test]
    fn rejects_protected_system_paths_with_no_workspace() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let prev = workspace_root();
        set_workspace_root(None);
        for p in PROTECTED_SYSTEM_PATHS {
            // /proc and others may not exist in some sandboxes; only run the assertion
            // when the path is real on this host.
            if !std::path::Path::new(p).exists() {
                continue;
            }
            let result = resolve_within_workspace("test", p);
            let err = match result {
                Ok(ok) => panic!("expected refusal for protected path '{p}', but got Ok({ok:?})"),
                Err(e) => e,
            };
            let msg = format!("{err}");
            assert!(
                msg.contains("protected") || msg.contains("REFUSED"),
                "expected refusal for {p}, got: {msg}"
            );
        }
        set_workspace_root(prev);
    }

    #[test]
    fn rejects_path_outside_workspace_root() {
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let workspace = tmp.join(format!("phazeai-sbox-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();

        with_workspace(Some(workspace.clone()), || {
            // Pick something that exists but is outside the workspace.
            let outside = std::env::current_dir().unwrap();
            // Only assert if `outside` doesn't happen to be under `workspace`.
            if !outside.starts_with(&workspace) {
                let err = resolve_within_workspace("test", outside.to_str().unwrap()).unwrap_err();
                let msg = format!("{err}");
                assert!(msg.contains("outside the workspace"), "got: {msg}");
            }
        });

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn allows_path_inside_workspace_root() {
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let workspace = tmp.join(format!("phazeai-sbox-ok-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let inside = workspace.join("foo.txt");

        with_workspace(Some(workspace.clone()), || {
            let resolved = resolve_within_workspace("test", inside.to_str().unwrap()).unwrap();
            assert!(resolved.starts_with(&workspace));
        });

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn defeats_dotdot_escape() {
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let workspace = tmp.join(format!("phazeai-sbox-dd-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();

        with_workspace(Some(workspace.clone()), || {
            // ../../../etc/passwd from inside the workspace must escape detection.
            let escape = workspace.join("..").join("..").join("..").join("etc");
            let s = escape.to_string_lossy().to_string();
            let result = resolve_within_workspace("test", &s);
            assert!(
                result.is_err(),
                "expected ../.. to be refused, got {:?}",
                result
            );
        });

        let _ = std::fs::remove_dir_all(&workspace);
    }
}
