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

/// OS directories whose *contents* are off-limits too (unlike the list above,
/// which only refuses the exact path so e.g. `/home/me/project` stays usable).
const PROTECTED_SYSTEM_PREFIXES: &[&str] = &[
    "/etc",
    "/boot",
    "/dev",
    "/proc",
    "/sys",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/lib64",
    "/var/lib",
];

/// Credential stores under the user's home directory. No tool may read, list,
/// search or write these: whatever a tool returns is sent to the model provider.
const SENSITIVE_HOME_PATHS: &[&str] = &[
    ".ssh",
    ".aws",
    ".azure",
    ".gnupg",
    ".kube",
    ".docker",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".pgpass",
    ".git-credentials",
    ".config/gcloud",
    ".config/gh",
    ".password-store",
    ".local/share/keyrings",
];

/// File names that hold secrets wherever they appear.
const SENSITIVE_FILE_NAMES: &[&str] = &[
    ".env",
    "id_rsa",
    "id_ecdsa",
    "id_ed25519",
    "id_dsa",
    ".git-credentials",
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

/// True for credential stores and secret files (`~/.ssh`, `~/.aws`, `.env`,
/// private keys, ...). Tools must neither read nor write these.
pub fn is_sensitive_path(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if SENSITIVE_FILE_NAMES.contains(&name) || name.starts_with(".env.") {
            return true;
        }
    }
    if let Some(home) = dirs::home_dir() {
        if SENSITIVE_HOME_PATHS
            .iter()
            .any(|sub| path.starts_with(home.join(sub)))
        {
            return true;
        }
    }
    false
}

/// Returns true if `canonical` matches any path in [`PROTECTED_SYSTEM_PATHS`],
/// lies under a [`PROTECTED_SYSTEM_PREFIXES`] directory, is a credential
/// store ([`is_sensitive_path`]), or is the user's home directory itself.
pub fn is_protected_system_path(canonical: &Path) -> bool {
    let s = canonical.to_string_lossy();
    if PROTECTED_SYSTEM_PATHS.iter().any(|p| s.as_ref() == *p) {
        return true;
    }
    if PROTECTED_SYSTEM_PREFIXES
        .iter()
        .any(|p| canonical.starts_with(p))
    {
        return true;
    }
    if is_sensitive_path(canonical) {
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

    // Relative paths are relative to the workspace, not to wherever the
    // process happened to be launched from (e.g. $HOME from a desktop icon).
    let joined;
    let raw = match workspace_root() {
        Some(root) if Path::new(input).is_relative() => {
            joined = root.join(input);
            joined.as_path()
        }
        _ => Path::new(input),
    };

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

/// Directory walker for search/list tools: honours .gitignore, includes other
/// dotfiles, never follows symlinks out of the tree, and never descends into
/// `.git` or credential stores (see [`is_sensitive_path`]).
pub fn search_walker(root: &Path) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git" && !is_sensitive_path(e.path()));
    builder
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

    #[test]
    fn refuses_contents_of_system_dirs_and_credentials() {
        with_workspace(None, || {
            let err = resolve_within_workspace("test", "/etc/passwd").unwrap_err();
            assert!(format!("{err}").contains("protected"));
            if let Some(home) = dirs::home_dir() {
                for sub in [".ssh/id_rsa", ".aws/credentials", ".gnupg"] {
                    let p = home.join(sub);
                    assert!(
                        resolve_within_workspace("test", p.to_str().unwrap()).is_err(),
                        "{} must be refused",
                        p.display()
                    );
                }
            }
        });
    }

    #[test]
    fn secret_files_are_refused_inside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "KEY=secret").unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        with_workspace(Some(dir.path().to_path_buf()), || {
            assert!(resolve_within_workspace("test", ".env").is_err());
            assert!(resolve_within_workspace("test", "main.rs").is_ok());
        });
        let names: Vec<_> = search_walker(dir.path())
            .build()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        assert!(names.contains(&"main.rs".to_string()));
        assert!(!names.contains(&".env".to_string()));
    }

    #[test]
    fn relative_paths_resolve_against_workspace_not_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        with_workspace(Some(canonical.clone()), || {
            assert_eq!(resolve_within_workspace("test", ".").unwrap(), canonical);
            assert_eq!(
                resolve_within_workspace("test", "src").unwrap(),
                canonical.join("src")
            );
        });
    }
}
