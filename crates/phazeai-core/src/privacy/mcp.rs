//! Applying the privacy policy to MCP tool calls.
//!
//! The bridge in [`crate::tools::mcp_bridge`] hands every call here before it
//! reaches the server ([`tool_check`]) and every text result back for
//! filtering ([`filter_text`], [`filter_fs_listing`], [`filter_fs_error`]).
//! An MCP server is a separate process that reads the local filesystem, so the
//! only hooks the policy has are the arguments the model sends and the text
//! that comes back.
//!
//! What this does:
//! - **Arguments.** Every string that can name a file is resolved the way the
//!   server would resolve it — against the project root, the server's working
//!   directory and the directories on its command line, `~`, `file://` URIs,
//!   and the Unicode-equivalent directory entries the official filesystem
//!   server will open for a name that is not on disk verbatim — and each
//!   resolved path is run through the same [`PrivacyPolicy::check`] the
//!   built-in tools use. A directory whose contents are denied counts as
//!   denied; a `source`-like argument is checked as a whole tree, like
//!   built-in `move_path`; and a tool the server does not mark
//!   `readOnlyHint: true` may not name anything under `.phazeai/`.
//! - **Results.** The official filesystem server's `list_directory`,
//!   `list_directory_with_sizes` and `directory_tree` lose their denied
//!   entries (a listing not in the expected format is refused, not passed
//!   through, and the refusal logged; an error result is the server's message,
//!   passed on as an error with any denied path it names hidden), and
//!   `search_files` / `list_allowed_directories` lose denied paths; its read
//!   tools return allowed files' contents unchanged (their path arguments were
//!   already checked). For any other server, `git diff` sections and
//!   whole-line paths naming denied files are removed from each text block.
//!
//! What this cannot do: see a file the server opens on its own initiative (an
//! indexer, a packer, git history), a file reached by another name (a hard
//! link, a URL to a local web server), or anything the server sends over the
//! network. Those are documented limits, not bugs.

use super::{Denial, PrivacyPolicy};
use serde_json::Value;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The project settings directory, relative to the project root. A mutating
/// MCP tool may not name anything inside it (see [`super::SETTINGS_REASON`]).
pub const SETTINGS_DIR: &str = ".phazeai";

/// Strings longer than this (bytes) are not treated as a path to check.
const MAX_PATH_STRING: usize = 4096;
/// A call carrying more strings than this is refused rather than checked in
/// part, so a path can't be hidden behind padding.
const MAX_STRINGS: usize = 10_000;
/// A directory with more entries than this fails the Unicode-equivalent and
/// listing checks closed ("can't check") rather than being walked unbounded.
const MAX_DIR_ENTRIES: usize = 10_000;
/// More Unicode-equivalent spellings than this for one component fails closed.
const MAX_EQUIV: usize = 64;
/// A made-up child name used to ask the policy "is everything inside this
/// directory denied?" (e.g. `secrets/**`, `data/`).
const CONTENTS_PROBE: &str = "phazeai-privacy-probe";

/// How one server resolves the paths named in its arguments.
///
/// `bases` are the directories a relative path may resolve against (the
/// server's working directory and the existing directories on its command
/// line); the project root is added at check time. `home` is what the server
/// expands `~` to.
#[derive(Debug, Clone, Default)]
pub struct McpServerPaths {
    pub bases: Vec<PathBuf>,
    pub home: Option<PathBuf>,
    /// When true, every directory argument is checked as a whole tree (for
    /// packers/indexers). Off by default; `source`-like arguments are always
    /// checked as trees regardless.
    pub tree_dirs: bool,
}

/// What role an argument plays, which decides how its string is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// A path (or array of paths): checked relative or absolute.
    Plain,
    /// A move/copy source: also checked as a whole tree, and (for a mutating
    /// tool) refused if it contains the policy file.
    Source,
    /// A move/copy destination: paired with sources for the policy-file check.
    Destination,
    /// Free text (file contents, a message, a query): a path only when the
    /// whole string is absolute, `~`/`~/…`, or a `file:` URI.
    FreeText,
    /// A shell command line: checked word by word, like `bash`.
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathArg {
    pub key: String,
    pub raw: String,
    pub role: Role,
}

/// One reason a call is refused.
#[derive(Debug, Clone)]
pub struct McpDenial {
    /// The text shown to the model (the raw argument, with a note for tree /
    /// directory / Unicode / protected cases).
    pub shown: String,
    /// The path that tripped the check, for the audit log.
    pub hit: PathBuf,
    pub denial: Denial,
}

/// Why a call was refused.
#[derive(Debug, Clone)]
pub enum McpRefusal {
    /// One or more arguments named a denied path.
    Denied(Vec<McpDenial>),
    /// The call could not be checked (too many strings, a directory too large
    /// to walk), so it is refused rather than sent unchecked.
    Unchecked(String),
}

/// Files a successful call named, to note in the audit scope afterwards.
#[derive(Debug, Default, Clone)]
pub struct McpChecked {
    pub files: Vec<PathBuf>,
}

// ── Argument check (recommendation A) ───────────────────────────────────────

/// Check `args` against `policy`, as a tool named `tool` with the given
/// read-only hint, for a server that resolves paths as `server` describes.
/// Pure apart from reading the filesystem (to resolve `..`, symlinks, Unicode
/// spellings and directory contents).
pub fn check_call(
    policy: &PrivacyPolicy,
    tool: &str,
    read_only: bool,
    args: &Value,
    server: &McpServerPaths,
) -> Result<McpChecked, McpRefusal> {
    let pargs = path_args(args).map_err(|n| {
        McpRefusal::Unchecked(format!(
            "the call has {n} string arguments, too many to check"
        ))
    })?;

    let bases = bases_for(policy, server);
    let mut denials: Vec<McpDenial> = Vec::new();
    let mut checked = McpChecked::default();
    let mut sources: Vec<PathBuf> = Vec::new();
    let mut dests: Vec<(String, PathBuf)> = Vec::new();

    for pa in &pargs {
        if pa.role == Role::Command {
            for base in &bases {
                if let Some((word, denial)) =
                    super::command_denial_in(policy, &pa.raw, base, server.home.as_deref())
                {
                    denials.push(McpDenial {
                        shown: word.clone(),
                        hit: PathBuf::from(word),
                        denial,
                    });
                    break;
                }
            }
            continue;
        }

        if skippable(&pa.raw) {
            continue;
        }

        for c in candidates(&pa.raw, pa.role, policy, server) {
            match check_candidate(policy, tool, read_only, pa, &c, server) {
                CandOutcome::Denied(d) => {
                    denials.push(d);
                    break; // first hit refuses this string
                }
                CandOutcome::Unchecked(why) => return Err(McpRefusal::Unchecked(why)),
                CandOutcome::Ok => {
                    if matches!(pa.role, Role::Plain | Role::Source | Role::Destination)
                        && c.is_file()
                        && !checked.files.contains(&c)
                    {
                        checked.files.push(c.clone());
                    }
                }
            }
            if pa.role == Role::Source {
                sources.push(c);
            } else if pa.role == Role::Destination {
                dests.push((pa.raw.clone(), c));
            }
        }
    }

    // A move/copy whose result would create or replace the policy file.
    if !read_only {
        for (raw, dest) in &dests {
            for source in &sources {
                if policy.copy_would_replace_policy_file(source, dest) {
                    denials.push(McpDenial {
                        shown: format!("{raw} (would replace {})", super::POLICY_FILE),
                        hit: dest.clone(),
                        denial: Denial::protected(),
                    });
                    break;
                }
            }
        }
    }

    if denials.is_empty() {
        Ok(checked)
    } else {
        Err(McpRefusal::Denied(denials))
    }
}

enum CandOutcome {
    Ok,
    Denied(McpDenial),
    Unchecked(String),
}

fn check_candidate(
    policy: &PrivacyPolicy,
    tool: &str,
    read_only: bool,
    pa: &PathArg,
    c: &Path,
    server: &McpServerPaths,
) -> CandOutcome {
    // 1. The deny lists, both the lexical and the symlink-resolved form.
    if let Some(denial) = policy.check(c) {
        return CandOutcome::Denied(McpDenial {
            shown: pa.raw.clone(),
            hit: c.to_path_buf(),
            denial,
        });
    }

    // The paths the server would open for this candidate: `c` itself when it
    // exists, otherwise the Unicode-equivalent directory entries the server
    // resolves it to (a no-break space for a space, NFC vs NFD, …), plus the
    // non-existent terminal path for a new file. `c` is already lexically
    // normalized by `candidates`, the way the server's `path.resolve` resolves
    // `..` before it opens anything. Each target is flagged *definite* when the
    // fold is exact (so the server really opens it) or only *possible* when two
    // non-ASCII names might be equal (used only to widen the deny check — see
    // [`unicode_equivalents`]).
    let targets = match resolved_targets(c) {
        Ok(t) => t,
        Err(why) => return CandOutcome::Unchecked(why),
    };

    // 2. A Unicode-equivalent spelling the server might open that is denied
    //    (both definite and possible matches — over-refusing a path is safe).
    for (e, _definite) in &targets {
        if e.as_path() == c {
            continue; // the verbatim path is covered by step 1
        }
        if let Some(denial) = policy.check(e) {
            return CandOutcome::Denied(McpDenial {
                shown: format!(
                    "{} (the server would open {})",
                    pa.raw.escape_debug(),
                    policy.display_path(e)
                ),
                hit: e.clone(),
                denial,
            });
        }
    }

    // 3. A directory whose contents are all denied (e.g. listing `secrets`),
    //    under whatever spelling named it. Only a *definite* resolution is a
    //    directory the server would really open, so a fuzzy non-ASCII match is
    //    not treated as a real directory to probe.
    for (e, definite) in &targets {
        if *definite && e.is_dir() {
            if let Some(denial) = policy.check(&e.join(CONTENTS_PROBE)) {
                return CandOutcome::Denied(McpDenial {
                    shown: format!("{} (everything inside it)", pa.raw),
                    hit: e.clone(),
                    denial,
                });
            }
        }
    }

    // 4. A source directory (or any directory, with tree_dirs) holding a
    //    denied file, like built-in copy_path / move_path.
    if pa.role == Role::Source || server.tree_dirs {
        for (e, definite) in &targets {
            if *definite && e.is_dir() {
                if let Some((hit, denial)) = super::first_denied_under(policy, e) {
                    return CandOutcome::Denied(McpDenial {
                        shown: format!("{} (contains {})", pa.raw, policy.display_path(&hit)),
                        hit,
                        denial,
                    });
                }
            }
        }
    }

    // 5. A tool that may write must not touch the policy or the MCP server
    //    list (anything under .phazeai/), under whatever spelling — including a
    //    Unicode respelling of a parent directory the server maps back onto the
    //    real path. Read-only tools may read them.
    if !read_only {
        let moves = pa.role == Role::Source || tool_moves_or_deletes(tool);
        let definite_targets = targets
            .iter()
            .filter(|(_, definite)| *definite)
            .map(|(p, _)| p.as_path());
        for e in std::iter::once(c).chain(definite_targets) {
            if policy.is_under_settings_dir(e) {
                return CandOutcome::Denied(McpDenial {
                    shown: pa.raw.clone(),
                    hit: e.to_path_buf(),
                    denial: Denial::settings(),
                });
            }
            if moves && policy.contains_policy_file(e) {
                return CandOutcome::Denied(McpDenial {
                    shown: format!("{} (contains {})", pa.raw, super::POLICY_FILE),
                    hit: e.to_path_buf(),
                    denial: Denial::protected(),
                });
            }
        }
    }

    CandOutcome::Ok
}

/// The paths on disk the server would operate on for the (already lexically
/// normalized) candidate `c`: `c` itself (definite) when it exists, otherwise
/// the Unicode-equivalent entries the official filesystem server resolves it to
/// (which may include a non-existent terminal path for a new file). Each is
/// flagged definite (an exact fold match or a verbatim/terminal path, which the
/// server really opens) or possible (a fuzzy non-ASCII match, for the deny
/// check only).
fn resolved_targets(c: &Path) -> Result<Vec<(PathBuf, bool)>, String> {
    if c.exists() {
        Ok(vec![(c.to_path_buf(), true)])
    } else {
        unicode_equivalents(c)
    }
}

/// Whether the tool's name suggests it moves, renames or deletes, so naming a
/// directory that contains the policy file is a protected operation. The name
/// is split into whole word tokens first, so a destructive verb is matched as
/// a word (`delete_file`, `moveFile`, `mv`) and not as a chance substring
/// (`format_files`, `terraform_validate`, `transform` all *contain* "rm").
fn tool_moves_or_deletes(tool: &str) -> bool {
    const WORDS: &[&str] = &[
        "move", "rename", "mv", "delete", "remove", "rm", "unlink", "trash",
    ];
    name_tokens(tool)
        .iter()
        .any(|t| WORDS.contains(&t.as_str()))
}

/// Split a tool name into lowercase word tokens, breaking on any non-ASCII
/// -alphanumeric character and on camelCase boundaries (`moveFile` → `move`,
/// `file`).
fn name_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut prev_lower_or_digit = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && prev_lower_or_digit && !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
            cur.push(c.to_ascii_lowercase());
            prev_lower_or_digit = c.is_ascii_lowercase() || c.is_ascii_digit();
        } else {
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
            prev_lower_or_digit = false;
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// Record a call or listing refused because it could not be checked against
/// the policy (fail closed), like any other refusal. `shown` names what was
/// refused: the listed directory, or `the call` as a whole.
pub(crate) fn record_unchecked(shown: &str, why: &str, tool: &str) {
    super::audit::record_blocked_file(
        shown,
        &format!("could not be checked against the privacy policy ({why})"),
        tool,
    );
}

/// [`check_call`] against the active policy, recording one audit entry per
/// denial (or one for a call that could not be checked) and formatting the
/// refusal the model sees.
pub fn tool_check(
    tool: &str,
    read_only: bool,
    args: &Value,
    server: &McpServerPaths,
) -> Result<McpChecked, crate::error::PhazeError> {
    let policy = super::active();
    match check_call(&policy, tool, read_only, args, server) {
        Ok(checked) => Ok(checked),
        Err(McpRefusal::Denied(denials)) => {
            let mut msg = String::new();
            for d in &denials {
                let reason = d.denial.reason();
                super::audit::record_blocked_file(&policy.display_path(&d.hit), &reason, tool);
                if !msg.is_empty() {
                    msg.push('\n');
                }
                msg.push_str(&super::refusal(&d.shown, &reason));
            }
            Err(crate::error::PhazeError::tool(tool, msg))
        }
        Err(McpRefusal::Unchecked(why)) => {
            record_unchecked("the call", &why, tool);
            Err(crate::error::PhazeError::tool(
                tool,
                format!("REFUSED: PhazeAI could not check this call against the project's privacy policy: {why}."),
            ))
        }
    }
}

// ── Argument extraction ─────────────────────────────────────────────────────

/// Every string in `args` to check, with its role (§ the role table). `Err(n)`
/// when the call carries more than [`MAX_STRINGS`] strings.
pub(crate) fn path_args(args: &Value) -> Result<Vec<PathArg>, usize> {
    let mut out = Vec::new();
    let mut count = 0usize;
    walk_args("", args, &mut out, &mut count)?;
    Ok(out)
}

fn walk_args(
    key: &str,
    value: &Value,
    out: &mut Vec<PathArg>,
    count: &mut usize,
) -> Result<(), usize> {
    match value {
        Value::String(s) => {
            *count += 1;
            if *count > MAX_STRINGS {
                return Err(*count);
            }
            out.push(PathArg {
                key: key.to_string(),
                raw: s.clone(),
                role: role_for(key),
            });
        }
        Value::Array(items) => {
            for item in items {
                walk_args(key, item, out, count)?; // array elements inherit the key
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                walk_args(k, v, out, count)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn role_for(key: &str) -> Role {
    let k: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    const SOURCE: &[&str] = &["source", "sources", "src", "from", "origin", "oldpath"];
    const DEST: &[&str] = &["destination", "dest", "target", "to", "newpath"];
    const COMMAND: &[&str] = &[
        "command",
        "cmd",
        "script",
        "shell",
        "commandline",
        "args",
        "argv",
        "arguments",
    ];
    const FREE_TEXT: &[&str] = &[
        "content",
        "contents",
        "text",
        "body",
        "message",
        "description",
        "title",
        "comment",
        "summary",
        "prompt",
        "instructions",
        "oldtext",
        "newtext",
        "oldstring",
        "newstring",
        "oldstr",
        "newstr",
        "query",
        "sql",
        "pattern",
        "patterns",
        "regex",
        "excludepatterns",
    ];
    if SOURCE.contains(&k.as_str()) {
        Role::Source
    } else if DEST.contains(&k.as_str()) {
        Role::Destination
    } else if COMMAND.contains(&k.as_str()) {
        Role::Command
    } else if FREE_TEXT.contains(&k.as_str()) {
        Role::FreeText
    } else {
        Role::Plain
    }
}

/// Strings never treated as a path: empty, oversized, with a newline or NUL,
/// or a URL whose scheme is not `file`.
fn skippable(s: &str) -> bool {
    s.is_empty()
        || s.len() > MAX_PATH_STRING
        || s.bytes().any(|b| b == b'\n' || b == b'\r' || b == 0)
        || non_file_url(s)
}

fn non_file_url(s: &str) -> bool {
    match s.find("://") {
        Some(i) => {
            let scheme = &s[..i];
            is_url_scheme(scheme) && !scheme.eq_ignore_ascii_case("file")
        }
        None => false,
    }
}

fn is_url_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

// ── Resolving one argument to candidate paths ───────────────────────────────

/// The project root (first) and the server's bases.
fn bases_for(policy: &PrivacyPolicy, server: &McpServerPaths) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(root) = policy.root() {
        out.push(root.to_path_buf());
    }
    for b in &server.bases {
        if !out.contains(b) {
            out.push(b.clone());
        }
    }
    out
}

/// Absolute paths one argument string could name, the way the server resolves
/// it, each **lexically normalized** (`..`/`.` resolved without touching the
/// disk) exactly as the server's `path.resolve` does before it opens anything.
/// A relative free-text string names nothing (only a whole path does).
fn candidates(
    raw: &str,
    role: Role,
    policy: &PrivacyPolicy,
    server: &McpServerPaths,
) -> Vec<PathBuf> {
    let mut raw_paths: Vec<PathBuf> = Vec::new();
    // A `file:` URI, for a server that parses it as one.
    if let Some(p) = file_uri_path(raw) {
        raw_paths.push(p);
    }
    // The same string as a plain path, the way a server that does *not* parse
    // `file://` (the official filesystem server among them) resolves it — `~`,
    // absolute, or relative joined to every base. Checking both interpretations
    // is a safe over-approximation.
    if raw == "~" {
        if let Some(h) = &server.home {
            raw_paths.push(h.clone());
        }
    } else if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(h) = &server.home {
            raw_paths.push(h.join(rest));
        }
    } else if Path::new(raw).is_absolute() {
        raw_paths.push(PathBuf::from(raw));
    } else if role != Role::FreeText {
        // Free text counts only as a whole path, never a relative one.
        for b in bases_for(policy, server) {
            raw_paths.push(b.join(raw));
        }
    }

    let mut out: Vec<PathBuf> = Vec::new();
    for p in raw_paths {
        let p = super::normalize_lexically(&p);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// The filesystem path of a `file:` URI, or `None` for any other string. Uses
/// a boundary-safe prefix test so a multi-byte first character can't panic.
fn file_uri_path(s: &str) -> Option<PathBuf> {
    if !s.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")) {
        return None;
    }
    reqwest::Url::parse(s).ok()?.to_file_path().ok()
}

// ── Unicode-equivalent spellings (no-dependency fallback) ───────────────────

/// The existing paths the official filesystem server would open for `c` when
/// `c` is not on disk verbatim: it walks down from the nearest existing
/// ancestor, matching each component against directory entries whose
/// Unicode-folding ([`fold_key`]) is equal, and appends any missing tail.
///
/// Each returned path carries a *definite* flag: `true` when every component
/// was an exact fold match (a space separator or one of the three ASCII-NFC
/// characters) or a verbatim/terminal path — a path the server really opens —
/// and `false` when some component matched only because both names stay
/// non-ASCII after folding ([`both_non_ascii`]), which without full NFC can't
/// be decided. A possible (`false`) match is sound only for widening the deny
/// check; it must not be treated as a real directory to list or prune.
///
/// `Err` when a directory is too large to walk or one component has too many
/// equivalent spellings (fail closed).
fn unicode_equivalents(c: &Path) -> Result<Vec<(PathBuf, bool)>, String> {
    // Peel components off the end until we reach something that exists.
    let mut existing = c.to_path_buf();
    let mut tail: Vec<OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(n) => tail.push(n.to_owned()),
            None => return Ok(Vec::new()),
        }
        if !existing.pop() {
            return Ok(Vec::new());
        }
    }
    tail.reverse();

    let mut terminals: Vec<(PathBuf, bool)> = Vec::new();
    let mut frontier: Vec<(PathBuf, bool)> = vec![(existing, true)];
    for (depth, comp) in tail.iter().enumerate() {
        let want = comp.to_string_lossy();
        let mut next: Vec<(PathBuf, bool)> = Vec::new();
        for (dir, dir_definite) in &frontier {
            // Each matching entry with whether its fold is exact (definite).
            let mut matches: Vec<(OsString, bool)> = Vec::new();
            let mut exact: Option<OsString> = None;
            let rd = match std::fs::read_dir(dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            let mut seen = 0usize;
            for entry in rd.flatten() {
                seen += 1;
                if seen > MAX_DIR_ENTRIES {
                    return Err("a directory on the path is too large to check".to_string());
                }
                let name = entry.file_name();
                let ns = name.to_string_lossy();
                if ns == want {
                    exact = Some(name);
                    break;
                }
                if fold_key(&ns) == fold_key(&want) {
                    matches.push((name, true));
                } else if both_non_ascii(&ns, &want) {
                    matches.push((name, false));
                }
            }
            match exact {
                Some(name) => next.push((dir.join(name), *dir_definite)),
                None if matches.is_empty() => {
                    // The server appends the rest and gets a new (non-existent)
                    // path here; record it so a write under a denied directory
                    // is still caught. It is the path the server opens, so it is
                    // as definite as the ancestor it hangs off.
                    let mut p = dir.clone();
                    for seg in &tail[depth..] {
                        p.push(seg);
                    }
                    terminals.push((p, *dir_definite));
                }
                None => {
                    for (name, m_definite) in matches {
                        next.push((dir.join(name), *dir_definite && m_definite));
                    }
                }
            }
        }
        if next.len() > MAX_EQUIV {
            return Err("a path component has too many equivalent spellings".to_string());
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    terminals.extend(frontier);
    Ok(terminals)
}

/// Fold a path component the way the server compares two spellings, without a
/// Unicode library: map every space separator to U+0020 and the only three
/// non-ASCII characters whose NFC form is ASCII to it. Exact for any component
/// that is ASCII once folded; [`both_non_ascii`] handles the rest.
fn fold_key(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{037E}' => ';',
            '\u{1FEF}' => '`',
            '\u{212A}' => 'K',
            c if is_space_separator(c) => ' ',
            c => c,
        })
        .collect()
}

fn is_space_separator(c: char) -> bool {
    matches!(
        c as u32,
        0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000
    )
}

/// Two names that might still be NFC-equivalent after folding: both keep a
/// non-ASCII character, so the fallback can't rule the match out and
/// over-checks (the ASCII side of a pair never matches a non-ASCII side).
fn both_non_ascii(a: &str, b: &str) -> bool {
    !fold_key(a).is_ascii() && !fold_key(b).is_ascii()
}

// ── Result filtering (recommendation B) ─────────────────────────────────────

/// The directories a result's paths resolve against: the project root, the
/// server's bases, and any existing directory named by the call's arguments.
pub fn result_bases(policy: &PrivacyPolicy, server: &McpServerPaths, args: &Value) -> Vec<PathBuf> {
    let mut bases = bases_for(policy, server);
    if let Ok(pargs) = path_args(args) {
        for pa in &pargs {
            if pa.role == Role::Command || skippable(&pa.raw) {
                continue;
            }
            for c in candidates(&pa.raw, pa.role, policy, server) {
                if c.is_dir() && !bases.contains(&c) {
                    bases.push(c);
                }
            }
        }
    }
    bases
}

/// Drop `git diff` sections and whole-line paths naming denied files from
/// `text`. Returns the filtered text and how many items were removed. When
/// `placeholder` is `Some`, a removed whole-line path is replaced by that text
/// (keeping the line, so nothing silently vanishes from what may be file
/// content); when `None`, the line is deleted (for a tool whose output is a
/// list of paths, where a dropped entry is expected).
pub fn filter_text(
    policy: &PrivacyPolicy,
    bases: &[PathBuf],
    home: Option<&Path>,
    text: &str,
    placeholder: Option<&str>,
) -> (String, usize) {
    let mut hidden = 0;

    // Diff sections: everything before the first `diff --git`/`--cc` line is a
    // preamble kept verbatim; the rest goes through the diff parser. Never run
    // the parser on text with no such line (a stray `--- x` would be misread).
    let body = match diff_start(text) {
        Some(idx) => {
            let (pre, diff) = text.split_at(idx);
            let (filtered, omitted) = super::filter_unified_diff_by(diff, &|s| {
                path_str_denied(policy, bases, home, s, true)
            });
            hidden += omitted.len();
            format!("{pre}{filtered}")
        }
        None => text.to_string(),
    };

    // Whole-line paths: a line that is exactly one absolute / `~` / `file:`
    // path, and nothing else. Relative lines are too ambiguous to touch.
    let mut out = String::new();
    for line in body.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        if !content.is_empty()
            && content == content.trim()
            && is_whole_path(content)
            && path_str_denied(policy, bases, home, content, false)
        {
            hidden += 1;
            if let Some(ph) = placeholder {
                out.push_str(ph);
                out.push_str(&line[content.len()..]); // keep the line ending
            }
            continue;
        }
        out.push_str(line);
    }
    (out, hidden)
}

fn diff_start(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let l = line.trim_end_matches(['\n', '\r']);
        if l.starts_with("diff --git ")
            || l.starts_with("diff --cc ")
            || l.starts_with("diff --combined ")
        {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn is_whole_path(s: &str) -> bool {
    Path::new(s).is_absolute() || s == "~" || s.starts_with("~/") || file_uri_path(s).is_some()
}

/// Whether a path string from a result names a denied file. `allow_relative`
/// joins a relative string to every base (used for diff header paths);
/// otherwise only a whole absolute / `~` / `file:` path is resolved.
fn path_str_denied(
    policy: &PrivacyPolicy,
    bases: &[PathBuf],
    home: Option<&Path>,
    s: &str,
    allow_relative: bool,
) -> bool {
    if let Some(p) = file_uri_path(s) {
        return policy.check(&p).is_some();
    }
    if s == "~" {
        return home.is_some_and(|h| policy.check(h).is_some());
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return home.is_some_and(|h| policy.check(&h.join(rest)).is_some());
    }
    let p = Path::new(s);
    if p.is_absolute() {
        return policy.check(p).is_some();
    }
    allow_relative && bases.iter().any(|b| policy.check(&b.join(s)).is_some())
}

/// Filter a listing from the official filesystem server. `None` when `tool`
/// is not one of its listing tools; `Some(Err)` when the output is not in the
/// expected format (the caller then refuses it).
pub fn filter_fs_listing(
    policy: &PrivacyPolicy,
    server: &McpServerPaths,
    tool: &str,
    args: &Value,
    text: &str,
) -> Option<Result<(String, usize), String>> {
    match tool {
        "list_directory" => Some(filter_list_directory(policy, server, args, text, false)),
        "list_directory_with_sizes" => {
            Some(filter_list_directory(policy, server, args, text, true))
        }
        "directory_tree" => Some(filter_directory_tree(policy, server, args, text)),
        _ => None,
    }
}

/// Whether `tool` is one of the official filesystem server's listing tools,
/// whose successful output [`filter_fs_listing`] filters.
pub fn is_fs_listing_tool(tool: &str) -> bool {
    matches!(
        tool,
        "list_directory" | "list_directory_with_sizes" | "directory_tree"
    )
}

/// Filter an error result from one of the official filesystem server's
/// listing tools. The server's message (`ENOENT: no such file or directory,
/// scandir '/p/x'`) is not a listing, so it is never parsed as one, which
/// would pass a plain error off as a privacy refusal. It gets the
/// [`filter_text`] pass, and each path it quotes is checked as well:
/// `directory_tree` stops at the first descendant it can't open and quotes
/// that path, a name its listing would have hidden. A denied quoted path is
/// replaced with `placeholder`. Returns the text and how many paths were
/// hidden.
pub fn filter_fs_error(
    policy: &PrivacyPolicy,
    bases: &[PathBuf],
    home: Option<&Path>,
    text: &str,
    placeholder: &str,
) -> (String, usize) {
    let (text, mut hidden) = filter_text(policy, bases, home, text, Some(placeholder));
    let out = text
        .split_inclusive('\n')
        .map(|line| hide_quoted_path(policy, home, line, placeholder, &mut hidden))
        .collect();
    (out, hidden)
}

/// `line` with a quoted denied path hidden. A quoted path runs from a `'`
/// that starts a whole path to the next `'` or, for a name that itself holds
/// a `'`, to the last one on the line (Node quotes the path last). When either
/// span is denied, everything from there to the last `'` is replaced with
/// `placeholder`.
fn hide_quoted_path(
    policy: &PrivacyPolicy,
    home: Option<&Path>,
    line: &str,
    placeholder: &str,
    hidden: &mut usize,
) -> String {
    let Some(last) = line.rfind('\'') else {
        return line.to_string();
    };
    let denied = |s: &str| is_whole_path(s) && path_str_denied(policy, &[], home, s, false);
    let mut from = 0;
    while let Some(off) = line[from..last].find('\'') {
        let start = from + off + 1;
        let next = line[start..].find('\'').map_or(last, |o| start + o);
        if denied(&line[start..next]) || (next != last && denied(&line[start..last])) {
            *hidden += 1;
            return format!("{}{placeholder}{}", &line[..start], &line[last..]);
        }
        from = start;
    }
    line.to_string()
}

/// The existing directories `args.path` resolves to — lexically normalized and
/// then through the server's Unicode-equivalent resolution, so a `..` or a
/// no-break-space spelling resolves to the same real directory the server
/// listed (the listing is of one of them; checking against all is a safe
/// over-approximation). Empty when nothing resolves to a directory, which
/// makes the caller fail closed rather than filter against an empty set.
fn listing_dirs(policy: &PrivacyPolicy, server: &McpServerPaths, args: &Value) -> Vec<PathBuf> {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for c in candidates(path, Role::Plain, policy, server) {
        for (t, definite) in resolved_targets(&c).unwrap_or_default() {
            // Only a definite resolution is a directory the server really
            // listed; a fuzzy non-ASCII match is not treated as one.
            if definite && t.is_dir() && !dirs.contains(&t) {
                dirs.push(t);
            }
        }
    }
    dirs
}

fn name_denied(policy: &PrivacyPolicy, dirs: &[PathBuf], name: &str) -> bool {
    dirs.iter().any(|d| policy.check(&d.join(name)).is_some())
}

fn filter_list_directory(
    policy: &PrivacyPolicy,
    server: &McpServerPaths,
    args: &Value,
    text: &str,
    with_sizes: bool,
) -> Result<(String, usize), String> {
    let dirs = listing_dirs(policy, server, args);
    if dirs.is_empty() {
        // Nothing resolved to a directory, so there is nothing to check the
        // entries against — fail closed instead of passing the listing through.
        return Err("could not resolve the listed directory".to_string());
    }
    // Authoritative entry names for the size variant, whose columns can't be
    // split on whitespace (a name may contain spaces).
    let denied_names: Vec<String> = if with_sizes {
        let mut names = Vec::new();
        for d in &dirs {
            let rd = std::fs::read_dir(d).map_err(|e| format!("can't read the directory: {e}"))?;
            let mut seen = 0usize;
            for entry in rd.flatten() {
                seen += 1;
                if seen > MAX_DIR_ENTRIES {
                    return Err("the directory is too large to check".to_string());
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if policy.check(&d.join(&name)).is_some() && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    } else {
        Vec::new()
    };

    let mut out: Vec<&str> = Vec::new();
    let mut hidden = 0;
    for line in text.lines() {
        if with_sizes && (line.is_empty() || is_size_summary(line)) {
            out.push(line);
            continue;
        }
        let name = entry_name(line).ok_or_else(|| format!("unexpected listing line: {line:?}"))?;
        let denied = if with_sizes {
            denied_names
                .iter()
                .any(|n| name == *n || name.strip_prefix(n.as_str()).is_some_and(starts_space))
        } else {
            name_denied(policy, &dirs, name)
        };
        if denied {
            hidden += 1;
        } else {
            out.push(line);
        }
    }
    Ok((out.join("\n"), hidden))
}

/// The entry name in a `[FILE] name` / `[DIR] name` line (for the size
/// variant, the padded name field, trailing spaces kept). `None` for any
/// other shape.
fn entry_name(line: &str) -> Option<&str> {
    line.strip_prefix("[FILE] ")
        .or_else(|| line.strip_prefix("[DIR] "))
}

fn starts_space(rest: &str) -> bool {
    rest.is_empty() || rest.starts_with(' ')
}

fn is_size_summary(line: &str) -> bool {
    line.starts_with("Total: ") || line.starts_with("Combined size: ")
}

fn filter_directory_tree(
    policy: &PrivacyPolicy,
    server: &McpServerPaths,
    args: &Value,
    text: &str,
) -> Result<(String, usize), String> {
    let tree: Value = serde_json::from_str(text)
        .map_err(|e| format!("the tree is not the expected JSON: {e}"))?;
    let Value::Array(nodes) = tree else {
        return Err("the tree is not a JSON array".to_string());
    };
    let dirs = listing_dirs(policy, server, args);
    if dirs.is_empty() {
        // Nothing resolved to a directory, so the entries can't be checked —
        // fail closed instead of returning the tree unpruned.
        return Err("could not resolve the listed directory".to_string());
    }
    let mut hidden = 0;
    let kept = prune_tree(policy, &dirs, Path::new(""), &nodes, &mut hidden)?;
    let text = serde_json::to_string_pretty(&Value::Array(kept))
        .map_err(|e| format!("can't re-serialize the tree: {e}"))?;
    Ok((text, hidden))
}

fn prune_tree(
    policy: &PrivacyPolicy,
    dirs: &[PathBuf],
    rel: &Path,
    nodes: &[Value],
    hidden: &mut usize,
) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for node in nodes {
        let obj = node.as_object().ok_or("a tree node is not an object")?;
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .ok_or("a tree node has no name")?;
        let kind = obj
            .get("type")
            .and_then(Value::as_str)
            .ok_or("a tree node has no type")?;
        let child_rel = rel.join(name);
        let denied = dirs
            .iter()
            .any(|d| policy.check(&d.join(&child_rel)).is_some());
        if denied {
            *hidden += 1;
            continue;
        }
        let mut kept = serde_json::Map::new();
        kept.insert("name".to_string(), Value::String(name.to_string()));
        kept.insert("type".to_string(), Value::String(kind.to_string()));
        if let Some(children) = obj.get("children") {
            let children = children
                .as_array()
                .ok_or("a tree node's children is not an array")?;
            let kept_children = prune_tree(policy, dirs, &child_rel, children, hidden)?;
            kept.insert("children".to_string(), Value::Array(kept_children));
        }
        out.push(Value::Object(kept));
    }
    Ok(out)
}
