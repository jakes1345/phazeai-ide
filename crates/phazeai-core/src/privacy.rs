//! Per-project AI privacy controls.
//!
//! A project can limit what may be sent to an AI provider with
//! `<project>/.phazeai/privacy.toml`:
//!
//! ```toml
//! # Never send these to any AI (gitignore-style globs, relative to the project root)
//! deny = ["secrets/**", "*.sqlite"]
//! # Use only on-device models (Ollama, LM Studio, any localhost/127.0.0.1/::1 endpoint)
//! local_only = false
//! ```
//!
//! # Deny patterns
//!
//! Project patterns follow `.gitignore` rules:
//! - a pattern without `/` (e.g. `*.sqlite`, `secrets`) matches a file or
//!   directory name at any depth;
//! - a pattern containing `/` (e.g. `secrets/**`, `/config/prod.toml`) is
//!   anchored to the project root;
//! - a trailing `/` (e.g. `data/`) matches a directory and everything in it;
//! - a matching directory denies everything inside it;
//! - `*` never crosses `/`, `**` does;
//! - negation (`!pattern`) is **not** supported and is reported as an error
//!   rather than silently ignored.
//!
//! [`BUILTIN_DENY`] always applies on top of the project list (it can't be
//! switched off from the file). Built-in patterns match at any depth and are
//! case-insensitive. For paths outside the project root only the built-in list
//! applies.
//!
//! # Invalid policy files
//!
//! A missing file means "built-in list only, cloud allowed". A file that
//! can't be read or parsed (unknown keys, bad globs, wrong types) **fails
//! closed**: the last good project deny list for that root (if any) is kept
//! *and* `local_only` is forced on, so nothing leaves the machine until the
//! file is fixed. The error is available from [`PrivacyPolicy::load_error`]
//! and is included in every refusal message.
//!
//! # What is enforced
//!
//! - Agent tools: `read_file`, `write_file`, `edit_file`, `diagnostics` refuse
//!   denied paths; `copy_path` / `move_path` refuse denied sources (including
//!   directories containing a denied file, symlinks followed) and
//!   destinations; `grep`, `glob`, `list_files`, `find_path` skip denied
//!   entries (grep reports only a count).
//! - The policy file itself is **protected**: the model may read it, but
//!   `write_file`, `edit_file`, `delete_path`, `move_path`, `copy_path`,
//!   `create_directory`, `download` and `bash` refuse to change, replace or
//!   remove `.phazeai/privacy.toml` (or the `.phazeai/` directory holding it),
//!   so the agent can't lift its own restrictions. See
//!   [`PrivacyPolicy::is_policy_file`] and [`tool_check_protected`].
//! - IDE / CLI features that attach file contents leave denied files out:
//!   @mentions, editor AI actions, inline edit, inline completion, the Design
//!   panel's style sources and design system, the commit-message diff, project
//!   instruction files, the repo map, skills, semantic-search results, `/add`.
//! - Every LLM client built through [`crate::llm::ProviderRegistry`] is a
//!   [`GuardedClient`]: it checks `local_only` before *each* request and
//!   records the request in [`audit`].
//!
//! **Best-effort only:** the `bash` tool check inspects the words of the
//! command line (see [`command_denial`]); a command can still read a denied
//! file indirectly (`grep -r`, scripts, variables). MCP servers run outside
//! this process and are not restricted. Approve `bash` and MCP calls with that
//! in mind.

pub mod audit;
mod guard;

pub use guard::GuardedClient;

use crate::llm::ProviderId;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

/// Location of the policy file, relative to the project root.
pub const POLICY_FILE: &str = ".phazeai/privacy.toml";

/// Why a mutating tool refuses to touch the policy file (see [`Denial::reason`]).
pub const PROTECTED_REASON: &str =
    "the project's privacy policy, which can only be edited by you, in the IDE (Settings › Privacy) or an editor";

/// `git` options that stop it from quoting non-ASCII path names in `diff` and
/// `status` output (`"caf\303\251.txt"`), so the paths can be compared with
/// the deny list. Pass before the sub-command: `git -c core.quotePath=false diff`.
/// Quotes, backslashes and control characters are still escaped;
/// [`filter_unified_diff`] and [`filter_status_porcelain`] decode those.
pub const GIT_NO_QUOTE_PATH: &[&str] = &["-c", "core.quotePath=false"];

/// Files that are never sent to any AI, whatever the project file says.
///
/// Matched at any depth (inside or outside the project), case-insensitively.
/// `.env.*` excludes the conventional templates in [`BUILTIN_ALLOW`].
pub const BUILTIN_DENY: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.keystore",
    "*.jks",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    ".ssh/**",
    ".aws/**",
    ".gnupg/**",
    ".npmrc",
    ".pypirc",
    ".netrc",
    ".git-credentials",
    "credentials*.json",
    "*service-account*.json",
    "secrets.*",
    ".docker/config.json",
    "terraform.tfstate*",
];

/// Template env files that hold no secrets; exempt from the built-in `.env.*`.
pub const BUILTIN_ALLOW: &[&str] = &[".env.example", ".env.sample", ".env.template"];

/// The commented template written by [`ensure_policy_file`].
pub const TEMPLATE: &str = r#"# PhazeAI privacy policy for this project.
#
# Files matching `deny` are never sent to any AI provider: the agent's tools
# refuse to read, write or search them, and @mentions, editor AI actions,
# inline edit and inline completion leave them out.
#
# Patterns use .gitignore syntax, relative to the project root:
#   "*.sqlite"     a name without "/" matches at any depth
#   "secrets/**"   a pattern with "/" is anchored to the project root
#   "data/"        a trailing "/" matches a directory and everything in it
# Negated patterns ("!foo") are not supported.
#
# A built-in list of common secret files (.env, *.pem, *.key, id_rsa*, .ssh/,
# .aws/, credentials*.json, ...) always applies on top of this list.
deny = []

# true = only on-device models may be used for this project: Ollama, LM Studio,
# or any endpoint on localhost / 127.0.0.1 / ::1. Cloud providers are refused.
local_only = false
"#;

/// Which list a denial came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenySource {
    BuiltIn,
    Project,
    /// Not a deny-list hit: the path is the policy file itself (or the
    /// directory holding it), which the model may read but never change.
    Protected,
}

/// Why a path is denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    pub source: DenySource,
    /// The pattern as written (built-in const or project file).
    pub pattern: String,
}

impl Denial {
    /// Human-readable reason, e.g. "blocked by the built-in secrets list (`*.pem`)".
    pub fn reason(&self) -> String {
        match self.source {
            DenySource::BuiltIn => {
                format!("blocked by the built-in secrets list (`{}`)", self.pattern)
            }
            DenySource::Project => format!("blocked by {POLICY_FILE} (`{}`)", self.pattern),
            DenySource::Protected => PROTECTED_REASON.to_string(),
        }
    }

    fn protected() -> Self {
        Self {
            source: DenySource::Protected,
            pattern: POLICY_FILE.to_string(),
        }
    }
}

/// A compiled glob list that remembers which source pattern each glob came from.
#[derive(Debug, Clone)]
struct PatternSet {
    set: GlobSet,
    /// glob index → index into `patterns`
    owner: Vec<usize>,
    patterns: Vec<String>,
}

impl PatternSet {
    fn empty() -> Self {
        Self {
            set: GlobSet::empty(),
            owner: Vec::new(),
            patterns: Vec::new(),
        }
    }

    /// Indices (into `patterns`) of every pattern matching `path`.
    fn matching(&self, path: &Path) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .set
            .matches(path)
            .into_iter()
            .map(|i| self.owner[i])
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

fn build_glob(pattern: &str, case_insensitive: bool) -> Result<globset::Glob, String> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .case_insensitive(case_insensitive)
        .backslash_escape(true)
        .build()
        .map_err(|e| e.to_string())
}

/// Expand one gitignore-style pattern into the globs that implement it.
fn expand_pattern(raw: &str, any_depth: bool) -> Result<Vec<String>, String> {
    let p = raw.trim();
    if p.is_empty() {
        return Err("empty pattern".to_string());
    }
    if p.starts_with('!') {
        return Err(format!(
            "`{raw}`: negated patterns (\"!…\") are not supported; remove the pattern instead"
        ));
    }
    let dir_only = p.ends_with('/');
    let body = p.trim_end_matches('/');
    if body.is_empty() || body == "/" {
        return Err(format!("`{raw}` would match the whole project"));
    }
    let anchored = !any_depth && body.contains('/');
    let base = if anchored {
        body.trim_start_matches('/').to_string()
    } else if body.starts_with("**/") {
        body.to_string()
    } else {
        format!("**/{}", body.trim_start_matches('/'))
    };
    let mut globs = Vec::new();
    if base.ends_with("/**") {
        globs.push(base);
    } else {
        globs.push(format!("{base}/**"));
        if !dir_only {
            globs.push(base);
        }
    }
    Ok(globs)
}

fn compile(patterns: &[String], any_depth: bool, ci: bool) -> Result<PatternSet, String> {
    let mut builder = GlobSetBuilder::new();
    let mut owner = Vec::new();
    for (idx, pat) in patterns.iter().enumerate() {
        for g in expand_pattern(pat, any_depth)? {
            let glob = build_glob(&g, ci).map_err(|e| format!("`{pat}`: {e}"))?;
            builder.add(glob);
            owner.push(idx);
        }
    }
    let set = builder.build().map_err(|e| e.to_string())?;
    Ok(PatternSet {
        set,
        owner,
        patterns: patterns.to_vec(),
    })
}

static BUILTIN: LazyLock<PatternSet> = LazyLock::new(|| {
    let pats: Vec<String> = BUILTIN_DENY.iter().map(|s| s.to_string()).collect();
    compile(&pats, true, true).expect("built-in privacy patterns are valid globs")
});

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    local_only: bool,
}

/// The effective privacy policy for one project (or for no project).
#[derive(Debug, Clone)]
pub struct PrivacyPolicy {
    root: Option<PathBuf>,
    project: PatternSet,
    local_only: bool,
    file_exists: bool,
    error: Option<String>,
}

impl PrivacyPolicy {
    /// Built-in list only, no project root.
    pub fn defaults() -> Self {
        Self {
            root: None,
            project: PatternSet::empty(),
            local_only: false,
            file_exists: false,
            error: None,
        }
    }

    fn defaults_for(root: PathBuf) -> Self {
        Self {
            root: Some(root),
            ..Self::defaults()
        }
    }

    /// Load `<root>/.phazeai/privacy.toml`. A missing file yields the
    /// defaults; an unreadable or invalid file is an error.
    pub fn load(root: &Path) -> Result<Self, String> {
        let root = canonical_root(root);
        let path = root.join(POLICY_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&root, &text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::defaults_for(root)),
            Err(e) => Err(format!("couldn't read {POLICY_FILE}: {e}")),
        }
    }

    /// Parse policy text for `root` (marks the file as existing).
    pub fn parse(root: &Path, text: &str) -> Result<Self, String> {
        let file: PolicyFile =
            toml::from_str(text).map_err(|e| format!("{POLICY_FILE}: {}", e.to_string().trim()))?;
        let project =
            compile(&file.deny, false, false).map_err(|e| format!("{POLICY_FILE}: deny {e}"))?;
        Ok(Self {
            root: Some(canonical_root(root)),
            project,
            local_only: file.local_only,
            file_exists: true,
            error: None,
        })
    }

    /// The fail-closed policy used when the file is invalid: keep the last
    /// good project list (if any) and force `local_only`.
    fn fail_closed(root: PathBuf, last_good: Option<&PrivacyPolicy>, error: String) -> Self {
        Self {
            root: Some(root),
            project: last_good
                .map(|p| p.project.clone())
                .unwrap_or_else(PatternSet::empty),
            local_only: true,
            file_exists: true,
            error: Some(error),
        }
    }

    /// The project root this policy applies to.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Path of the policy file, when there is a root.
    pub fn policy_path(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|r| r.join(POLICY_FILE))
    }

    /// Whether `.phazeai/privacy.toml` exists.
    pub fn file_exists(&self) -> bool {
        self.file_exists
    }

    /// The error from loading the file, when it is invalid (policy is then fail-closed).
    pub fn load_error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Only on-device models may be used (also true when the file is invalid).
    pub fn local_only(&self) -> bool {
        self.local_only
    }

    /// The project's own deny patterns, as written.
    pub fn project_patterns(&self) -> &[String] {
        &self.project.patterns
    }

    /// Whether `path` (absolute, or relative to the project root) is denied.
    pub fn is_denied(&self, path: &Path) -> bool {
        self.check(path).is_some()
    }

    /// Why `path` (absolute, or relative to the project root) is denied, if it is.
    ///
    /// Both the lexical path and its symlink-resolved form are checked, so a
    /// link named `notes.txt` pointing at `.env` is denied too.
    pub fn check(&self, path: &Path) -> Option<Denial> {
        let lexical = self.normalized(path);
        if let Some(d) = self.check_absolute(&lexical) {
            return Some(d);
        }
        match canonicalize_lenient(&lexical) {
            Some(real) if real != lexical => self.check_absolute(&real),
            _ => None,
        }
    }

    /// `path` made absolute (relative to the project root, else the cwd) with
    /// `.` and `..` resolved lexically.
    fn normalized(&self, path: &Path) -> PathBuf {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(root) = &self.root {
            root.join(path)
        } else {
            std::env::current_dir()
                .map(|c| c.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        normalize_lexically(&abs)
    }

    /// The lexical and the symlink-resolved forms of `path` (one entry when
    /// they agree).
    fn forms(&self, path: &Path) -> Vec<PathBuf> {
        let lexical = self.normalized(path);
        let mut out = vec![lexical.clone()];
        if let Some(real) = canonicalize_lenient(&lexical) {
            if real != lexical {
                out.push(real);
            }
        }
        out
    }

    /// Whether `path` is this project's policy file (`.phazeai/privacy.toml`),
    /// compared by normalised path and after resolving symlinks, whether or
    /// not the file exists yet. Mutating tools refuse such a path (the model
    /// could otherwise rewrite the rules that bind it); reading is fine.
    pub fn is_policy_file(&self, path: &Path) -> bool {
        let Some(policy) = self.policy_path() else {
            return false;
        };
        self.forms(path).contains(&policy)
    }

    /// Whether `path` is the policy file or the `.phazeai/` directory that
    /// holds it — what a shell command must not name (`rm -rf .phazeai`).
    pub fn is_policy_file_or_dir(&self, path: &Path) -> bool {
        let Some(policy) = self.policy_path() else {
            return false;
        };
        let dir = policy.parent();
        self.forms(path)
            .iter()
            .any(|p| *p == policy || Some(p.as_path()) == dir)
    }

    /// Whether `path` is the policy file or any directory containing it (up
    /// to and including the project root), so deleting or moving `path` would
    /// remove the policy.
    pub fn contains_policy_file(&self, path: &Path) -> bool {
        let Some(policy) = self.policy_path() else {
            return false;
        };
        self.forms(path).iter().any(|p| policy.starts_with(p))
    }

    /// Whether copying or moving `source` to `dest` would create or replace
    /// the policy file: `dest` is the file itself, or `dest` is a directory
    /// the policy lives under and `source` holds the matching relative path
    /// (e.g. copying `tmp/` → `.phazeai/` when `tmp/privacy.toml` exists, or
    /// `evil/` → the project root when `evil/.phazeai/privacy.toml` exists).
    pub fn copy_would_replace_policy_file(&self, source: &Path, dest: &Path) -> bool {
        let Some(policy) = self.policy_path() else {
            return false;
        };
        if self.is_policy_file(dest) {
            return true;
        }
        let source = self.normalized(source);
        self.forms(dest).iter().any(|d| {
            policy
                .strip_prefix(d)
                .ok()
                .filter(|rel| !rel.as_os_str().is_empty())
                .is_some_and(|rel| source.join(rel).symlink_metadata().is_ok())
        })
    }

    fn check_absolute(&self, abs: &Path) -> Option<Denial> {
        let rel = self.root.as_ref().and_then(|r| abs.strip_prefix(r).ok());
        match rel {
            Some(rel) => {
                if rel.as_os_str().is_empty() {
                    return None; // the root itself
                }
                if let Some(d) = builtin_denial(rel) {
                    return Some(d);
                }
                self.project.matching(rel).first().map(|&i| Denial {
                    source: DenySource::Project,
                    pattern: self.project.patterns[i].clone(),
                })
            }
            None => builtin_denial(&strip_root(abs)),
        }
    }

    /// `path` shown relative to the project root when inside it.
    pub fn display_path(&self, path: &Path) -> String {
        let abs = if path.is_absolute() {
            normalize_lexically(path)
        } else {
            return path.to_string_lossy().replace('\\', "/");
        };
        if let Some(root) = &self.root {
            if let Ok(rel) = abs.strip_prefix(root) {
                return rel.to_string_lossy().replace('\\', "/");
            }
            if let Some(real) = canonicalize_lenient(&abs) {
                if let Ok(rel) = real.strip_prefix(root) {
                    return rel.to_string_lossy().replace('\\', "/");
                }
            }
        }
        abs.to_string_lossy().to_string()
    }
}

fn builtin_denial(target: &Path) -> Option<Denial> {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let allowed_template = BUILTIN_ALLOW.contains(&name.as_str());
    BUILTIN
        .matching(target)
        .into_iter()
        .map(|i| BUILTIN.patterns[i].clone())
        // `.env.example` etc. are exempt from `.env.*` only — not from, say, `.ssh/**`.
        .find(|p| !(allowed_template && p == ".env.*"))
        .map(|pattern| Denial {
            source: DenySource::BuiltIn,
            pattern,
        })
}

fn canonical_root(root: &Path) -> PathBuf {
    root.canonicalize()
        .unwrap_or_else(|_| normalize_lexically(root))
}

/// Resolve `.` and `..` without touching the filesystem.
fn normalize_lexically(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalise `p`, or (when it doesn't exist) its nearest existing ancestor
/// with the missing tail re-appended.
fn canonicalize_lenient(p: &Path) -> Option<PathBuf> {
    if let Ok(c) = p.canonicalize() {
        return Some(c);
    }
    let mut tail = Vec::new();
    let mut cur = p.to_path_buf();
    loop {
        let name = cur.file_name()?.to_owned();
        tail.push(name);
        if !cur.pop() {
            return None;
        }
        if let Ok(c) = cur.canonicalize() {
            let mut out = c;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return Some(out);
        }
    }
}

/// Drop the root / drive prefix so a glob like `**/.ssh/**` can match.
fn strip_root(p: &Path) -> PathBuf {
    p.components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .collect()
}

// ── Process-wide active policy ────────────────────────────────────────────────

/// Modification stamp of the policy file (None = missing).
type Stamp = Option<(SystemTime, u64)>;

struct ActiveState {
    root: PathBuf,
    policy: Arc<PrivacyPolicy>,
    stamp: Stamp,
    last_good: Option<Arc<PrivacyPolicy>>,
}

static ACTIVE: Mutex<Option<ActiveState>> = Mutex::new(None);
static DEFAULTS: LazyLock<Arc<PrivacyPolicy>> =
    LazyLock::new(|| Arc::new(PrivacyPolicy::defaults()));

fn stamp_of(root: &Path) -> Stamp {
    let meta = std::fs::metadata(root.join(POLICY_FILE)).ok()?;
    Some((
        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        meta.len(),
    ))
}

fn load_state(root: PathBuf, last_good: Option<Arc<PrivacyPolicy>>) -> ActiveState {
    let stamp = stamp_of(&root);
    match PrivacyPolicy::load(&root) {
        Ok(p) => {
            let p = Arc::new(p);
            ActiveState {
                root,
                policy: p.clone(),
                stamp,
                last_good: Some(p),
            }
        }
        Err(e) => {
            tracing::warn!("privacy policy invalid, failing closed: {e}");
            let p = PrivacyPolicy::fail_closed(root.clone(), last_good.as_deref(), e);
            ActiveState {
                root,
                policy: Arc::new(p),
                stamp,
                last_good,
            }
        }
    }
}

/// Make `root` the active project. Its policy file is (re)loaded now and
/// re-checked for changes on every [`active`] call.
pub fn set_active_project(root: impl AsRef<Path>) {
    let root = canonical_root(root.as_ref());
    let mut guard = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    let last_good = guard
        .as_ref()
        .filter(|s| s.root == root)
        .and_then(|s| s.last_good.clone());
    *guard = Some(load_state(root, last_good));
}

/// Forget the active project (only the built-in list applies afterwards).
pub fn clear_active_project() {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The active project root, if one was set.
pub fn active_root() -> Option<PathBuf> {
    ACTIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|s| s.root.clone())
}

/// The active policy. Cheap: one `stat` of the policy file; reloads when the
/// file was created, changed or deleted since the last call.
pub fn active() -> Arc<PrivacyPolicy> {
    let mut guard = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_mut() else {
        return DEFAULTS.clone();
    };
    if stamp_of(&state.root) != state.stamp {
        let root = state.root.clone();
        let last_good = state.last_good.clone();
        *state = load_state(root, last_good);
    }
    state.policy.clone()
}

/// The policy for `root`: the active one when `root` is the active project,
/// otherwise loaded fresh (fail-closed on error).
pub fn policy_for(root: &Path) -> Arc<PrivacyPolicy> {
    let root = canonical_root(root);
    if active_root().as_deref() == Some(root.as_path()) {
        return active();
    }
    Arc::new(
        PrivacyPolicy::load(&root)
            .unwrap_or_else(|e| PrivacyPolicy::fail_closed(root.clone(), None, e)),
    )
}

/// Check `path` against the active policy. On denial, records a blocked entry
/// in the audit log (tagged with `via`, e.g. "read_file" or "@mention") and
/// returns the reason.
pub fn check_and_record(path: &Path, via: &str) -> Result<(), String> {
    let policy = active();
    match policy.check(path) {
        None => Ok(()),
        Some(d) => {
            let reason = d.reason();
            audit::record_blocked_file(&policy.display_path(path), &reason, via);
            Err(reason)
        }
    }
}

/// Standard refusal text for tools.
pub fn refusal(path: &str, reason: &str) -> String {
    if reason == PROTECTED_REASON {
        return format!(
            "REFUSED: '{path}' is {reason}. The AI may read the policy but must not change, replace, \
             move or delete it; don't try another way."
        );
    }
    format!(
        "REFUSED: '{path}' is {reason}. This project's privacy policy keeps that file away from AI models; \
         don't try to access it another way."
    )
}

fn protected_refusal(tool: &str, shown: &str) -> crate::error::PhazeError {
    let d = Denial::protected();
    audit::record_blocked_file(shown, &d.reason(), tool);
    crate::error::PhazeError::tool(tool, refusal(shown, &d.reason()))
}

/// Refuse to write, create or replace `path` when it is the active project's
/// policy file (`write_file`, `edit_file`, `create_directory`, `download`).
pub fn tool_check_protected(
    tool: &str,
    path: &Path,
    shown: &str,
) -> Result<(), crate::error::PhazeError> {
    if active().is_policy_file(&absolutize(path)) {
        return Err(protected_refusal(tool, shown));
    }
    Ok(())
}

/// Refuse to delete or move `path` when it is the policy file or a directory
/// containing it (`delete_path`, `move_path` source).
pub fn tool_check_protected_tree(
    tool: &str,
    path: &Path,
    shown: &str,
) -> Result<(), crate::error::PhazeError> {
    let policy = active();
    let abs = absolutize(path);
    if policy.contains_policy_file(&abs) {
        let what = if policy.is_policy_file(&abs) {
            shown.to_string()
        } else {
            format!("{shown} (contains {POLICY_FILE})")
        };
        return Err(protected_refusal(tool, &what));
    }
    Ok(())
}

/// Refuse a `copy_path` / `move_path` whose result would create or replace
/// the policy file (see [`PrivacyPolicy::copy_would_replace_policy_file`]).
pub fn tool_check_protected_dest(
    tool: &str,
    source: &Path,
    dest: &Path,
    shown_dest: &str,
) -> Result<(), crate::error::PhazeError> {
    if active().copy_would_replace_policy_file(&absolutize(source), &absolutize(dest)) {
        return Err(protected_refusal(
            tool,
            &format!("{shown_dest} (would replace {POLICY_FILE})"),
        ));
    }
    Ok(())
}

/// Make a tool path absolute (relative paths resolve against the process cwd,
/// which is what the filesystem calls in the tools do).
pub fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Tool-side check: refuse `path` (shown to the model as `shown`) when the
/// active policy denies it, recording the attempt.
pub fn tool_check(tool: &str, path: &Path, shown: &str) -> Result<(), crate::error::PhazeError> {
    check_and_record(&absolutize(path), tool)
        .map_err(|reason| crate::error::PhazeError::tool(tool, refusal(shown, &reason)))
}

/// Like [`tool_check`], but a directory is refused when anything inside it is denied.
pub fn tool_check_tree(
    tool: &str,
    path: &Path,
    shown: &str,
) -> Result<(), crate::error::PhazeError> {
    let policy = active();
    match first_denied_under(&policy, path) {
        None => Ok(()),
        Some((hit, d)) => {
            let reason = d.reason();
            let hit_shown = policy.display_path(&hit);
            audit::record_blocked_file(&hit_shown, &reason, tool);
            let what = if absolutize(path) == hit {
                shown.to_string()
            } else {
                format!("{shown} (contains {hit_shown})")
            };
            Err(crate::error::PhazeError::tool(
                tool,
                refusal(&what, &reason),
            ))
        }
    }
}

/// First denied file at or under `path` (for copy/move of directories).
///
/// Symlinks are followed: a directory holding a link to a denied directory
/// (or file) is denied, since copying it would copy the target's contents.
/// Every entry is checked by its lexical path *and* its resolved target
/// ([`PrivacyPolicy::check`]). Link loops are reported by the walker as
/// errors and skipped, so the walk always terminates.
pub fn first_denied_under(policy: &PrivacyPolicy, path: &Path) -> Option<(PathBuf, Denial)> {
    let path = absolutize(path);
    if let Some(d) = policy.check(&path) {
        return Some((path, d));
    }
    if !path.is_dir() {
        return None;
    }
    for entry in ignore::WalkBuilder::new(&path)
        .standard_filters(false)
        .follow_links(true)
        .build()
        .flatten()
    {
        if let Some(d) = policy.check(entry.path()) {
            return Some((entry.path().to_path_buf(), d));
        }
    }
    None
}

// ── local_only ────────────────────────────────────────────────────────────────

/// Host part of a base URL (`http://` is assumed when no scheme is given).
pub fn url_host(base_url: &str) -> Option<String> {
    let trimmed = base_url.trim();
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    reqwest::Url::parse(&with_scheme)
        .ok()?
        .host_str()
        .map(|h| h.to_string())
}

/// `localhost`, `127.0.0.0/8`, `::1` (and IPv4-mapped loopback).
pub fn is_loopback_host(host: &str) -> bool {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.');
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => a.is_loopback(),
        Ok(std::net::IpAddr::V6(a)) => {
            a.is_loopback() || a.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        Err(_) => false,
    }
}

/// Whether requests to this provider stay on this machine.
///
/// The base URL decides: its host must be loopback. Only when no base URL is
/// configured does the provider kind decide (Ollama / LM Studio default to
/// localhost). An Ollama or LM Studio server on another machine is therefore
/// **not** local — the data would leave this computer.
pub fn is_local_endpoint(provider: &ProviderId, base_url: &str) -> bool {
    if base_url.trim().is_empty() {
        return provider.is_local();
    }
    url_host(base_url).is_some_and(|h| is_loopback_host(&h))
}

/// Refuse a non-local provider when `policy` is local-only.
pub fn ensure_provider_allowed_by(
    policy: &PrivacyPolicy,
    provider: &ProviderId,
    base_url: &str,
) -> Result<(), String> {
    if !policy.local_only() || is_local_endpoint(provider, base_url) {
        return Ok(());
    }
    let host = url_host(base_url).unwrap_or_else(|| base_url.to_string());
    let target = if host.is_empty() {
        provider.name().to_string()
    } else {
        format!("{} ({host})", provider.name())
    };
    Err(match policy.load_error() {
        Some(err) => format!(
            "Blocked: {POLICY_FILE} couldn't be loaded ({err}), so cloud AI is disabled for this \
             project until it's fixed. {target} was not contacted. Fix the file (Settings › Privacy › \
             Edit privacy.toml) or use a local model (Ollama, LM Studio)."
        ),
        None => format!(
            "Blocked: this project is set to local-only AI (local_only = true in {POLICY_FILE}), so \
             {target} was not contacted. Switch to a local model (Ollama, LM Studio, or any localhost \
             endpoint), or turn off \"Local only\" in Settings › Privacy (CLI: edit {POLICY_FILE})."
        ),
    })
}

/// [`ensure_provider_allowed_by`] with the active policy.
pub fn ensure_provider_allowed(provider: &ProviderId, base_url: &str) -> Result<(), String> {
    ensure_provider_allowed_by(&active(), provider, base_url)
}

/// [`ensure_provider_allowed`], recording a refusal in the audit log.
pub fn check_provider(
    provider: &ProviderId,
    base_url: &str,
    model: &str,
) -> Result<(), crate::error::PhazeError> {
    ensure_provider_allowed(provider, base_url).map_err(|reason| {
        audit::record_blocked_provider(provider.name(), model, &reason);
        crate::error::PhazeError::Other(reason)
    })
}

// ── Editing the file ──────────────────────────────────────────────────────────

/// Create `<root>/.phazeai/privacy.toml` from [`TEMPLATE`] if it doesn't exist.
pub fn ensure_policy_file(root: &Path) -> Result<PathBuf, String> {
    let path = canonical_root(root).join(POLICY_FILE);
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
        }
        write_atomic(&path, TEMPLATE).map_err(|e| format!("couldn't write {POLICY_FILE}: {e}"))?;
    }
    Ok(path)
}

/// Set `local_only` in the policy file, keeping comments and other lines
/// intact (only the `local_only = …` line changes, or one is appended).
/// Creates the file from the template when missing. Refuses to touch a file
/// that doesn't parse, so a broken file is never made worse.
pub fn write_local_only(root: &Path, value: bool) -> Result<(), String> {
    let path = ensure_policy_file(root)?;
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("couldn't read {POLICY_FILE}: {e}"))?;
    PrivacyPolicy::parse(root, &text).map_err(|e| format!("fix the file first — {e}"))?;
    let updated = set_local_only_in_text(&text, value);
    PrivacyPolicy::parse(root, &updated)?;
    write_atomic(&path, &updated).map_err(|e| format!("couldn't write {POLICY_FILE}: {e}"))?;
    if active_root().as_deref() == Some(canonical_root(root).as_path()) {
        set_active_project(root);
    }
    Ok(())
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Name of the temporary file [`write_atomic`] uses for `path`, in the same
/// directory (so the final rename never crosses filesystems).
fn atomic_tmp_path(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

/// Replace `path` with `contents` atomically.
///
/// The text goes to a fresh temporary file in the same directory, is flushed
/// to disk, and the temporary file is renamed over `path`. A concurrent reader
/// — the active-policy reload in [`active`], another PhazeAI process, an
/// editor — therefore sees either the old file or the new one, never an
/// empty or partial file (an empty policy parses as "no rules", a partial one
/// fails closed; both would make requests mid-write behave differently). The
/// rename is atomic on POSIX and replaces an existing file on Windows; if it
/// fails anyway, the contents are copied into place as a last resort.
/// Permissions of an existing file are kept.
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = atomic_tmp_path(path);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = file.set_permissions(meta.permissions());
        }
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(rename_err) => std::fs::copy(&tmp, path)
                .map(|_| ())
                .map_err(|_| rename_err),
        }
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

fn set_local_only_in_text(text: &str, value: bool) -> String {
    let mut replaced = false;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let is_key = !replaced
            && trimmed
                .strip_prefix("local_only")
                .is_some_and(|rest| rest.trim_start().starts_with('='));
        if is_key {
            let indent = &line[..line.len() - trimmed.len()];
            let comment = trimmed.find('#').map(|i| &trimmed[i..]);
            out.push(match comment {
                Some(c) => format!("{indent}local_only = {value} {c}"),
                None => format!("{indent}local_only = {value}"),
            });
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !replaced {
        out.push(format!("local_only = {value}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

// ── Content helpers ───────────────────────────────────────────────────────────

/// Remove the per-file sections of a unified `git diff` whose old or new path
/// is denied. Returns the filtered diff and the omitted paths.
///
/// Paths git quotes C-style (`"caf\303\251.txt"`, `"a\"b"`, `"tab\there"`)
/// are decoded before the check. A section whose header can't be parsed is
/// dropped too — it may belong to a denied file — and reported in the omitted
/// list as `<unparsed diff header: …>`. Run git with [`GIT_NO_QUOTE_PATH`] so
/// the common non-ASCII case needs no decoding at all.
pub fn filter_unified_diff(diff: &str, policy: &PrivacyPolicy) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut omitted = Vec::new();
    let mut chunk = String::new();
    let flush = |chunk: &mut String, out: &mut String, omitted: &mut Vec<String>| {
        if chunk.is_empty() {
            return;
        }
        match diff_chunk_paths(chunk) {
            Ok(paths) => {
                if let Some(p) = paths.iter().find(|p| policy.is_denied(Path::new(p))) {
                    omitted.push(p.clone());
                } else {
                    out.push_str(chunk);
                }
            }
            Err(header) => omitted.push(format!(
                "<unparsed diff header: {}>",
                crate::text::truncate_bytes(&header, 200)
            )),
        }
        chunk.clear();
    };
    for line in diff.split_inclusive('\n') {
        if is_diff_section_start(line) {
            flush(&mut chunk, &mut out, &mut omitted);
        }
        chunk.push_str(line);
    }
    flush(&mut chunk, &mut out, &mut omitted);
    (out, omitted)
}

fn is_diff_section_start(line: &str) -> bool {
    line.starts_with("diff --git ")
        || line.starts_with("diff --cc ")
        || line.starts_with("diff --combined ")
}

/// Every path named by the headers of one per-file diff section.
///
/// `Ok(vec![])` means the text has no header lines at all (a preamble);
/// `Err(line)` names the first header line that couldn't be parsed.
fn diff_chunk_paths(chunk: &str) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for line in chunk.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("@@") {
            // Hunk bodies follow; a removed line starting with "-- " would
            // otherwise look like a "--- " header.
            break;
        }
        let fail = || Err(line.to_string());
        if let Some(rest) = line.strip_prefix("diff --git ") {
            match diff_git_header_paths(rest) {
                Some(mut p) => paths.append(&mut p),
                None => return fail(),
            }
        } else if let Some(rest) = line
            .strip_prefix("diff --cc ")
            .or_else(|| line.strip_prefix("diff --combined "))
        {
            match diff_path_token(rest) {
                Some((p, "")) => paths.push(p),
                _ => return fail(),
            }
        } else if let Some(rest) = line.strip_prefix("--- ") {
            match diff_path_token(rest) {
                Some((p, "")) => {
                    if p != "/dev/null" {
                        paths.push(strip_diff_prefix(&p, "a/"));
                    }
                }
                _ => return fail(),
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            match diff_path_token(rest) {
                Some((p, "")) => {
                    if p != "/dev/null" {
                        paths.push(strip_diff_prefix(&p, "b/"));
                    }
                }
                _ => return fail(),
            }
        } else if let Some(rest) = line
            .strip_prefix("rename from ")
            .or_else(|| line.strip_prefix("rename to "))
            .or_else(|| line.strip_prefix("copy from "))
            .or_else(|| line.strip_prefix("copy to "))
        {
            match diff_path_token(rest) {
                Some((p, "")) => paths.push(p),
                _ => return fail(),
            }
        }
    }
    Ok(paths)
}

fn strip_diff_prefix(path: &str, prefix: &str) -> String {
    path.strip_prefix(prefix).unwrap_or(path).to_string()
}

/// The two paths of a `diff --git A B` header (without their `a/` / `b/`).
fn diff_git_header_paths(rest: &str) -> Option<Vec<String>> {
    let strip = |a: &str, b: &str| vec![strip_diff_prefix(a, "a/"), strip_diff_prefix(b, "b/")];
    if rest.starts_with('"') {
        // Quoted first side: its closing quote marks the split.
        let (a, after) = diff_path_token(rest)?;
        let (b, tail) = diff_path_token(after.strip_prefix(' ')?)?;
        return tail.is_empty().then(|| strip(&a, &b));
    }
    if let Some(idx) = rest.find(" \"") {
        // Unquoted old side, quoted new side.
        let (b, tail) = diff_path_token(&rest[idx + 1..])?;
        return tail.is_empty().then(|| strip(&rest[..idx], &b));
    }
    // Both unquoted: "a/P b/P" — split in the middle when both sides are the
    // same path (so a space inside P is harmless), else at the last " b/".
    let mid = (rest.len().checked_sub(1)?) / 2;
    if rest.is_char_boundary(mid)
        && rest.as_bytes().get(mid) == Some(&b' ')
        && rest[..mid].trim_start_matches("a/") == rest[mid + 1..].trim_start_matches("b/")
    {
        return Some(strip(&rest[..mid], &rest[mid + 1..]));
    }
    let (a, b) = rest.rsplit_once(" b/")?;
    (!a.is_empty() && !b.is_empty()).then(|| strip(a, b))
}

/// One path from a diff header: a C-quoted string (decoded) or the rest of
/// the text up to its end. Returns the path and the text after it.
fn diff_path_token(s: &str) -> Option<(String, &str)> {
    match s.strip_prefix('"') {
        Some(quoted) => c_unquote_prefix(quoted),
        None => Some((s.to_string(), "")),
    }
}

/// Decode a git C-style quoted string. `s` starts right after the opening
/// quote; returns the decoded text and what follows the closing quote. `None`
/// when the quote is unterminated, an escape is unknown, or the bytes aren't
/// UTF-8 (such a path can't be compared with the policy — callers fail closed).
fn c_unquote_prefix(s: &str) -> Option<(String, &str)> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let text = String::from_utf8(out).ok()?;
                return Some((text, &s[i + 1..]));
            }
            b'\\' => {
                i += 1;
                match *bytes.get(i)? {
                    b'a' => out.push(0x07),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'v' => out.push(0x0b),
                    b'\\' => out.push(b'\\'),
                    b'"' => out.push(b'"'),
                    b'0'..=b'7' => {
                        let mut value: u32 = 0;
                        let mut digits = 0;
                        while digits < 3 && bytes.get(i).is_some_and(u8::is_ascii_digit) {
                            let d = (bytes[i] - b'0') as u32;
                            if d > 7 {
                                break;
                            }
                            value = value * 8 + d;
                            i += 1;
                            digits += 1;
                        }
                        out.push(u8::try_from(value).ok()?);
                        continue;
                    }
                    _ => return None,
                }
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    None
}

/// Keep only the lines of `git status --porcelain` output whose path(s) are
/// not denied. Paths are relative to `repo_root` (the repository top level);
/// quoted paths are decoded as in [`filter_unified_diff`]. A line whose path
/// can't be parsed is dropped (fail closed). Returns the kept lines and the
/// omitted paths.
pub fn filter_status_porcelain(
    status: &str,
    repo_root: &Path,
    policy: &PrivacyPolicy,
) -> (Vec<String>, Vec<String>) {
    let mut kept = Vec::new();
    let mut omitted = Vec::new();
    for line in status.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        match porcelain_paths(line) {
            Some(paths) => match paths.iter().find(|p| policy.is_denied(&repo_root.join(p))) {
                Some(p) => omitted.push(p.clone()),
                None => kept.push(line.to_string()),
            },
            None => omitted.push(format!(
                "<unparsed status line: {}>",
                crate::text::truncate_bytes(line, 200)
            )),
        }
    }
    (kept, omitted)
}

/// Paths in one `XY PATH` / `XY ORIG -> PATH` porcelain line.
fn porcelain_paths(line: &str) -> Option<Vec<String>> {
    let mut chars = line.chars();
    let x = chars.next()?;
    let y = chars.next()?;
    if chars.next()? != ' ' {
        return None;
    }
    let rest = &line[3..];
    if rest.is_empty() {
        return None;
    }
    let renamed = matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C');
    if let Some(quoted) = rest.strip_prefix('"') {
        let (first, tail) = c_unquote_prefix(quoted)?;
        if tail.is_empty() {
            return Some(vec![first]);
        }
        let (second, tail) = diff_path_token(tail.strip_prefix(" -> ")?)?;
        return tail.is_empty().then(|| vec![first, second]);
    }
    if renamed {
        if let Some((a, b)) = rest.split_once(" -> ") {
            let (b, tail) = diff_path_token(b)?;
            return tail.is_empty().then(|| vec![a.to_string(), b]);
        }
    }
    Some(vec![rest.to_string()])
}

/// Best-effort check of a shell command line: returns the first word that
/// names a denied path (relative words resolve against `cwd`; `~/` expands to
/// the home directory; a word with `*`, `?` or `[` is expanded against the
/// entries of its directory, one level deep). A word naming the policy file
/// or its `.phazeai/` directory is reported with [`DenySource::Protected`].
///
/// This does **not** see paths produced at run time (variables, command
/// substitution, recursive tools like `grep -r` or `find`, scripts that open
/// files themselves). It errs on the side of refusing: a denied name that is
/// only mentioned (e.g. `echo .env`) is refused too.
pub fn command_denial(
    policy: &PrivacyPolicy,
    command: &str,
    cwd: &Path,
) -> Option<(String, Denial)> {
    let word_denial = |p: &Path| {
        policy
            .check(p)
            .or_else(|| policy.is_policy_file_or_dir(p).then(Denial::protected))
    };
    for word in shell_words(command) {
        let expanded = if let Some(rest) = word.strip_prefix("~/") {
            match dirs::home_dir() {
                Some(h) => h.join(rest),
                None => PathBuf::from(&word),
            }
        } else {
            cwd.join(&word)
        };
        if let Some(d) = word_denial(&expanded) {
            return Some((word, d));
        }
        if word.contains(['*', '?', '[']) {
            if let Some(hit) = expand_glob_word(&word_denial, &expanded) {
                return Some((format!("{word} → {}", hit.0), hit.1));
            }
        }
    }
    None
}

fn expand_glob_word(
    denial: &dyn Fn(&Path) -> Option<Denial>,
    pattern_path: &Path,
) -> Option<(String, Denial)> {
    let parent = pattern_path.parent()?;
    let name_pat = pattern_path.file_name()?.to_string_lossy().to_string();
    let matcher = build_glob(&name_pat, false).ok()?.compile_matcher();
    let entries = std::fs::read_dir(parent).ok()?;
    for entry in entries.flatten().take(10_000) {
        let name = entry.file_name();
        if matcher.is_match(Path::new(&name)) {
            let p = entry.path();
            if let Some(d) = denial(&p) {
                return Some((name.to_string_lossy().to_string(), d));
            }
        }
    }
    None
}

/// Split a command line into words: whitespace and shell operators separate
/// words, quotes are removed, and `--opt=value` / `VAR=value` yield `value`.
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in command.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => cur.push(ch),
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                c if c.is_whitespace() || ";|&<>()`$={},".contains(c) => {
                    if !cur.is_empty() {
                        words.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            },
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
        .into_iter()
        .filter(|w| !w.starts_with('-') && w != "~")
        .collect()
}

#[cfg(test)]
pub(crate) mod tests;
