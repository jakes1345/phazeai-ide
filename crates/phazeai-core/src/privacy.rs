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
//!   directories containing a denied file) and destinations; `grep`, `glob`,
//!   `list_files`, `find_path` skip denied entries (grep reports only a count).
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
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

/// Location of the policy file, relative to the project root.
pub const POLICY_FILE: &str = ".phazeai/privacy.toml";

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
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(root) = &self.root {
            root.join(path)
        } else {
            std::env::current_dir()
                .map(|c| c.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        let lexical = normalize_lexically(&abs);
        if let Some(d) = self.check_absolute(&lexical) {
            return Some(d);
        }
        match canonicalize_lenient(&lexical) {
            Some(real) if real != lexical => self.check_absolute(&real),
            _ => None,
        }
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
    format!(
        "REFUSED: '{path}' is {reason}. This project's privacy policy keeps that file away from AI models; \
         don't try to access it another way."
    )
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
        std::fs::write(&path, TEMPLATE)
            .map_err(|e| format!("couldn't write {POLICY_FILE}: {e}"))?;
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
    std::fs::write(&path, updated).map_err(|e| format!("couldn't write {POLICY_FILE}: {e}"))?;
    if active_root().as_deref() == Some(canonical_root(root).as_path()) {
        set_active_project(root);
    }
    Ok(())
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
pub fn filter_unified_diff(diff: &str, policy: &PrivacyPolicy) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut omitted = Vec::new();
    let mut chunk = String::new();
    let flush = |chunk: &mut String, out: &mut String, omitted: &mut Vec<String>| {
        if chunk.is_empty() {
            return;
        }
        let paths = diff_chunk_paths(chunk);
        if let Some(p) = paths.iter().find(|p| policy.is_denied(Path::new(p))) {
            omitted.push(p.clone());
        } else {
            out.push_str(chunk);
        }
        chunk.clear();
    };
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            flush(&mut chunk, &mut out, &mut omitted);
        }
        chunk.push_str(line);
    }
    flush(&mut chunk, &mut out, &mut omitted);
    (out, omitted)
}

fn diff_chunk_paths(chunk: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in chunk.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            // "a/P b/P" — split in the middle when both sides are the same path.
            if rest.len() >= 3 {
                let mid = (rest.len() - 3) / 2;
                if rest.is_char_boundary(mid) && rest[mid..].starts_with(" b/") {
                    paths.push(rest[..mid].to_string());
                } else if let Some((a, b)) = rest.rsplit_once(" b/") {
                    paths.push(a.to_string());
                    paths.push(b.to_string());
                }
            }
        } else if let Some(p) = line
            .strip_prefix("--- a/")
            .or_else(|| line.strip_prefix("+++ b/"))
            .or_else(|| line.strip_prefix("rename from "))
            .or_else(|| line.strip_prefix("rename to "))
            .or_else(|| line.strip_prefix("copy from "))
            .or_else(|| line.strip_prefix("copy to "))
        {
            paths.push(p.to_string());
        }
    }
    paths
}

/// Best-effort check of a shell command line: returns the first word that
/// names a denied path (relative words resolve against `cwd`; `~/` expands to
/// the home directory; a word with `*`, `?` or `[` is expanded against the
/// entries of its directory, one level deep).
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
    for word in shell_words(command) {
        let expanded = if let Some(rest) = word.strip_prefix("~/") {
            match dirs::home_dir() {
                Some(h) => h.join(rest),
                None => PathBuf::from(&word),
            }
        } else {
            cwd.join(&word)
        };
        if let Some(d) = policy.check(&expanded) {
            return Some((word, d));
        }
        if word.contains(['*', '?', '[']) {
            if let Some(hit) = expand_glob_word(policy, &expanded) {
                return Some((format!("{word} → {}", hit.0), hit.1));
            }
        }
    }
    None
}

fn expand_glob_word(policy: &PrivacyPolicy, pattern_path: &Path) -> Option<(String, Denial)> {
    let parent = pattern_path.parent()?;
    let name_pat = pattern_path.file_name()?.to_string_lossy().to_string();
    let matcher = build_glob(&name_pat, false).ok()?.compile_matcher();
    let entries = std::fs::read_dir(parent).ok()?;
    for entry in entries.flatten().take(10_000) {
        let name = entry.file_name();
        if matcher.is_match(Path::new(&name)) {
            let p = entry.path();
            if let Some(d) = policy.check(&p) {
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
