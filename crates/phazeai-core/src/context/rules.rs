//! Rules files that coding tools keep in a project, read for the system prompt.
//!
//! Teams write their rules for the tool they use: `AGENTS.md`, `CLAUDE.md`,
//! `GEMINI.md`, `.cursorrules` and `.cursor/rules/*.mdc`, Copilot's
//! `.github/copilot-instructions.md` and `.github/instructions/`, Windsurf's
//! `.windsurfrules` and `.windsurf/rules/`, Cline's `.clinerules`. PhazeAI
//! reads all of them, so a project needs no copy written for it.
//!
//! Every rule is sent with every request, so only what applies to every task
//! is inlined: the project's main rules file (the first of [`ROOT_FILES`]
//! that exists, with `@path` imports expanded) and Claude Code's personal
//! `CLAUDE.local.md`. Everything else goes in a short menu, one line per file
//! with what it covers, and the model reads a file with `read_file` before
//! work it covers. Exact copies (a `CLAUDE.md` symlinked to `AGENTS.md`, or
//! one that only imports it) are dropped, so they are never paid for twice.
//! A rule another tool marks as always on is inlined only when the project
//! has no main rules file; when it has one, the menu says the rule is always
//! on, since projects with both usually keep the same rules in each.
//!
//! Files the project's privacy policy denies are neither read nor listed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::system_prompt::Instructions;
use crate::privacy::PrivacyPolicy;

/// Files at a project's root that hold its rules, best first, with the tool
/// each is written for. The first that exists is inlined; the rest are
/// listed in the menu (unless they are copies of it).
pub const ROOT_FILES: &[(&str, &str)] = &[
    // The order PhazeAI has always used, so a project keeps the main file it had.
    (".phazerules", "PhazeAI"),
    (".cursorrules", "Cursor"),
    ("CLAUDE.md", "Claude Code"),
    ("AGENTS.md", "Codex and other agents"),
    (".phazeai/instructions.md", "PhazeAI"),
    (".phazeai/config.md", "PhazeAI"),
    (".ai/instructions.md", "AI tools"),
    // Other tools' files.
    ("GEMINI.md", "Gemini CLI"),
    (".github/copilot-instructions.md", "GitHub Copilot"),
    (".windsurfrules", "Windsurf"),
    (".clinerules", "Cline"),
];

/// Personal additions that go with the main rules file wherever they exist.
const PERSONAL_FILES: &[&str] = &["CLAUDE.local.md"];

/// Rules files a subdirectory can hold for work under it.
const NESTED_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md", "GEMINI.md"];

/// Menu lines at most; a monorepo with hundreds of rules files gets a count.
const MAX_MENU_ENTRIES: usize = 30;
/// Directory entries the walk for nested rules looks at, at most, so a huge
/// repository costs the prompt a bounded amount of time.
const MAX_WALK_ENTRIES: usize = 20_000;
const MAX_WALK_DEPTH: usize = 8;
/// How deep `@path` imports are followed (Claude Code allows 5 hops).
const MAX_IMPORT_DEPTH: usize = 4;
/// A rules file larger than this is not a rules file.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// A menu line's description is cut to this many bytes.
const MAX_ABOUT_BYTES: usize = 160;

/// A project's rules: the blocks every request carries and the files the
/// model reads when they apply.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProjectRules {
    /// Inlined, in order.
    pub inline: Vec<Instructions>,
    /// Listed in the menu, in order.
    pub on_demand: Vec<RuleEntry>,
}

/// One line of the menu: a rules file and what it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleEntry {
    pub path: PathBuf,
    pub about: String,
}

impl ProjectRules {
    /// The menu of rules read on demand, or `None` when there are none.
    pub fn menu(&self) -> Option<String> {
        if self.on_demand.is_empty() {
            return None;
        }
        let mut out = String::from(
            "# More project rules\nNot loaded. Before work one covers, read it with read_file.\n",
        );
        for entry in self.on_demand.iter().take(MAX_MENU_ENTRIES) {
            out.push_str(&format!("- {}: {}\n", entry.path.display(), entry.about));
        }
        let more = self.on_demand.len().saturating_sub(MAX_MENU_ENTRIES);
        if more > 0 {
            out.push_str(&format!(
                "- {more} more rules files in subdirectories are not listed.\n"
            ));
        }
        Some(out)
    }
}

/// The rules in the project at `root`.
pub fn discover(root: &Path) -> ProjectRules {
    let policy = crate::privacy::policy_for(root);
    let mut found = Found {
        root,
        policy: &policy,
        seen_paths: HashSet::new(),
        seen_texts: HashSet::new(),
    };
    let mut rules = ProjectRules::default();

    // The main rules file, then the other tools' root files.
    let mut has_main = false;
    for &(rel, tool) in ROOT_FILES {
        let path = root.join(rel);
        let Some(text) = found.read_new(&path) else {
            continue;
        };
        if has_main {
            rules.on_demand.push(RuleEntry {
                path,
                about: format!("rules written for {tool}; may repeat the rules above"),
            });
        } else {
            has_main = true;
            rules.inline.push(text.block(&path, ""));
        }
    }
    for rel in PERSONAL_FILES {
        let path = root.join(rel);
        if let Some(text) = found.read_new(&path) {
            rules.inline.push(text.block(&path, &format!("# {rel}\n")));
        }
    }

    // Rules files with front matter that says when they apply.
    let mut always = Vec::new();
    for format in [
        Format::Cursor,
        Format::Windsurf,
        Format::Copilot,
        Format::Cline,
    ] {
        let dir = root.join(format.dir());
        for path in rule_files(&dir, format) {
            found.structured(&path, format, None, &mut always, &mut rules.on_demand);
        }
    }
    for (path, text, label) in always {
        if has_main {
            rules.on_demand.push(RuleEntry { path, about: label });
        } else {
            let heading = format!("# {}\n", relative(root, &path).display());
            rules.inline.push(text.block(&path, &heading));
        }
    }

    // Rules for work in a subdirectory.
    found.nested(&mut rules.on_demand);
    rules
}

/// The tools whose rules directories carry front matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// `.cursor/rules/**/*.mdc`: `description`, `globs`, `alwaysApply`.
    Cursor,
    /// `.windsurf/rules/*.md`: `trigger` (`always_on`, `model_decision`,
    /// `glob`, `manual`), `description`, `globs`.
    Windsurf,
    /// `.github/instructions/**/*.instructions.md`: `applyTo`, `description`.
    Copilot,
    /// `.clinerules/*.md`: always on, unless `paths` limits them.
    Cline,
}

impl Format {
    fn dir(self) -> &'static str {
        match self {
            Format::Cursor => ".cursor/rules",
            Format::Windsurf => ".windsurf/rules",
            Format::Copilot => ".github/instructions",
            Format::Cline => ".clinerules",
        }
    }

    fn tool(self) -> &'static str {
        match self {
            Format::Cursor => "Cursor",
            Format::Windsurf => "Windsurf",
            Format::Copilot => "GitHub Copilot",
            Format::Cline => "Cline",
        }
    }

    fn is_rule_file(self, name: &str) -> bool {
        match self {
            Format::Cursor => name.ends_with(".mdc") || name.ends_with(".md"),
            Format::Copilot => name.ends_with(".instructions.md"),
            Format::Windsurf | Format::Cline => name.ends_with(".md") || name.ends_with(".txt"),
        }
    }

    /// When a rule applies, from its front matter.
    fn when(self, meta: &FrontMatter) -> When {
        let description = meta.get("description");
        let globs = |key: &str| meta.list(key);
        match self {
            Format::Cursor => {
                if meta.get("alwaysApply").is_some_and(is_true) {
                    When::Always
                } else {
                    When::Some {
                        description,
                        globs: globs("globs"),
                    }
                }
            }
            Format::Windsurf => match meta.get("trigger").as_deref() {
                Some("always_on") | None => When::Always,
                Some("manual") => When::Never,
                _ => When::Some {
                    description,
                    globs: globs("globs"),
                },
            },
            Format::Copilot => {
                let apply_to = globs("applyTo");
                if apply_to.iter().any(|g| matches!(g.as_str(), "**" | "**/*")) {
                    When::Always
                } else {
                    When::Some {
                        description,
                        globs: apply_to,
                    }
                }
            }
            Format::Cline => {
                let paths = globs("paths");
                if paths.is_empty() {
                    When::Always
                } else {
                    When::Some {
                        description,
                        globs: paths,
                    }
                }
            }
        }
    }
}

enum When {
    Always,
    /// Applies when the description fits the task or a file matches a glob;
    /// with neither, it is attached by hand only.
    Some {
        description: Option<String>,
        globs: Vec<String>,
    },
    Never,
}

fn is_true(value: String) -> bool {
    value.eq_ignore_ascii_case("true")
}

/// Rule files of `format` under `dir`, sorted, or none when `dir` is not a
/// directory.
fn rule_files(dir: &Path, format: Format) -> Vec<PathBuf> {
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = walkdir_files(dir, 4)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| format.is_rule_file(n))
        })
        .collect();
    files.sort();
    files
}

/// Files under `dir`, `depth` levels deep at most, in no particular order.
fn walkdir_files(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((dir, level)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() && level + 1 < depth => stack.push((path, level + 1)),
                Ok(t) if t.is_file() || t.is_symlink() => out.push(path),
                _ => {}
            }
        }
    }
    out
}

/// What the walk has read so far, so no file or text is taken twice.
struct Found<'a> {
    root: &'a Path,
    policy: &'a PrivacyPolicy,
    seen_paths: HashSet<PathBuf>,
    seen_texts: HashSet<String>,
}

/// A rules file's text, with its imports expanded.
struct RuleText {
    text: String,
    /// Line of the file `text` starts on, or `None` when imports were
    /// expanded into it (its lines are then no longer the file's).
    first_line: Option<usize>,
}

impl RuleText {
    fn block(self, path: &Path, heading: &str) -> Instructions {
        let block = match self.first_line {
            Some(line) => Instructions::from_file(path, line, self.text),
            None => Instructions::new(self.text),
        };
        block.with_heading(heading)
    }
}

impl Found<'_> {
    /// The text of the file at `path` if it can be read, the privacy policy
    /// allows it, and neither it nor its text was taken before.
    fn read_new(&mut self, path: &Path) -> Option<RuleText> {
        let text = self.read(path)?;
        let expanded = expand_imports(&text, path, self.root, self.policy);
        let first_line = (expanded == text).then_some(1);
        self.take_new(path, &expanded)?;
        Some(RuleText {
            text: expanded,
            first_line,
        })
    }

    /// `path`'s text, when it is a readable file of sensible size the
    /// privacy policy allows.
    fn read(&self, path: &Path) -> Option<String> {
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES || self.policy.is_denied(path) {
            return None;
        }
        std::fs::read_to_string(path).ok()
    }

    /// Marks `path` and `text` as taken; `None` when either already was.
    fn take_new(&mut self, path: &Path, text: &str) -> Option<()> {
        let canonical = path.canonicalize().ok()?;
        let normal = normalize(text);
        if normal.is_empty()
            || self.seen_paths.contains(&canonical)
            || self.seen_texts.contains(&normal)
        {
            return None;
        }
        self.seen_paths.insert(canonical);
        self.seen_texts.insert(normal);
        Some(())
    }

    /// A rules file with front matter: always-on rules go to `always` (with
    /// their menu label), the others to `menu`. `under` names the directory
    /// a nested rules directory belongs to.
    fn structured(
        &mut self,
        path: &Path,
        format: Format,
        under: Option<&Path>,
        always: &mut Vec<(PathBuf, RuleText, String)>,
        menu: &mut Vec<RuleEntry>,
    ) {
        let Some(content) = self.read(path) else {
            return;
        };
        let (meta, body, body_line) = front_matter(&content);
        let when = format.when(&meta);
        if matches!(when, When::Never) {
            return;
        }
        let expanded = expand_imports(body, path, self.root, self.policy);
        let first_line = (expanded == body).then_some(body_line);
        if self.take_new(path, &expanded).is_none() {
            return;
        }
        let tool = format.tool();
        let scope = under
            .map(|dir| format!(" for work under {}/", relative(self.root, dir).display()))
            .unwrap_or_default();
        match when {
            When::Always => {
                let summary = meta
                    .get("description")
                    .or_else(|| first_line_of(body))
                    .map(|d| format!(": {}", shorten(&d)))
                    .unwrap_or_default();
                let label = format!("always on in {tool}{scope}{summary}");
                if under.is_some() {
                    menu.push(RuleEntry {
                        path: path.to_path_buf(),
                        about: label,
                    });
                } else {
                    always.push((
                        path.to_path_buf(),
                        RuleText {
                            text: expanded,
                            first_line,
                        },
                        label,
                    ));
                }
            }
            When::Some { description, globs } => {
                let mut about = description.map(|d| shorten(&d)).unwrap_or_default();
                if !globs.is_empty() {
                    let files = format!("files {}", globs.join(", "));
                    about = if about.is_empty() {
                        files
                    } else {
                        format!("{about} ({files})")
                    };
                }
                if about.is_empty() {
                    // Attached by hand in its own tool; nothing says when.
                    return;
                }
                menu.push(RuleEntry {
                    path: path.to_path_buf(),
                    about: format!("{about}{scope}"),
                });
            }
            When::Never => {}
        }
    }

    /// Rules kept in subdirectories, for work under them: `AGENTS.md`,
    /// `CLAUDE.md`, `GEMINI.md` and `.cursor/rules`. Ignored files (the
    /// project's `.gitignore`) and `node_modules` are skipped.
    fn nested(&mut self, menu: &mut Vec<RuleEntry>) {
        let mut files = Vec::new();
        let mut cursor_dirs = Vec::new();
        let walk = ignore::WalkBuilder::new(self.root)
            .hidden(false)
            .max_depth(Some(MAX_WALK_DEPTH))
            .filter_entry(|e| {
                let name = e.file_name();
                name != ".git" && name != "node_modules"
            })
            .build();
        for entry in walk.flatten().take(MAX_WALK_ENTRIES) {
            let path = entry.path();
            let Some(parent) = path.parent() else {
                continue;
            };
            if parent == self.root {
                continue;
            }
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            let name = entry.file_name().to_string_lossy();
            if is_dir {
                if name == "rules"
                    && parent.file_name().is_some_and(|n| n == ".cursor")
                    && parent.parent().is_some_and(|p| p != self.root)
                {
                    cursor_dirs.push(path.to_path_buf());
                }
            } else if NESTED_FILES.contains(&name.as_ref()) {
                files.push(path.to_path_buf());
            }
        }
        files.sort();
        for path in files {
            if self.read_new(&path).is_some() {
                let dir = relative(self.root, path.parent().unwrap_or(self.root));
                let about = format!("rules for work under {}/", dir.display());
                menu.push(RuleEntry { path, about });
            }
        }
        cursor_dirs.sort();
        for dir in cursor_dirs {
            let Some(owner) = dir.parent().and_then(Path::parent) else {
                continue;
            };
            for path in rule_files(&dir, Format::Cursor) {
                self.structured(&path, Format::Cursor, Some(owner), &mut Vec::new(), menu);
            }
        }
    }
}

/// `text` with `@path` import lines (Claude Code's and Gemini CLI's syntax)
/// replaced by the files they name, outside code fences. An import is
/// expanded when it names a Markdown or text file inside the project that the
/// privacy policy allows; other lines stay as they are.
///
/// Only inside the project: these files come with the repository, so one
/// cloned from anyone could otherwise import `@~/notes/private.md` and send
/// the user's own files (which the project's privacy policy does not cover)
/// to the model.
fn expand_imports(text: &str, file: &Path, root: &Path, policy: &PrivacyPolicy) -> String {
    let mut stack = vec![file.canonicalize().unwrap_or_else(|_| file.to_path_buf())];
    expand(text, file, root, policy, &mut stack)
}

fn expand(
    text: &str,
    file: &Path,
    root: &Path,
    policy: &PrivacyPolicy,
    stack: &mut Vec<PathBuf>,
) -> String {
    if !text.lines().any(|l| import_target(l).is_some()) {
        return text.to_string();
    }
    let base = file.parent().unwrap_or(root);
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let imported = (!in_fence && stack.len() <= MAX_IMPORT_DEPTH)
            .then(|| import_target(line))
            .flatten()
            .and_then(|target| resolve_import(target, base, root, policy))
            .filter(|path| !stack.contains(path))
            .and_then(|path| {
                let content = std::fs::read_to_string(&path).ok()?;
                stack.push(path.clone());
                let expanded = expand(&content, &path, root, policy, stack);
                stack.pop();
                Some(expanded)
            });
        match imported {
            Some(content) => {
                out.push_str(content.trim_end());
                out.push('\n');
            }
            None => out.push_str(line),
        }
    }
    out
}

/// The path of an import line (`@docs/rules.md`), if `line` is one.
fn import_target(line: &str) -> Option<&str> {
    let target = line.trim().strip_prefix('@')?;
    (!target.is_empty()
        && !target.contains(char::is_whitespace)
        && (target.contains('/') || target.contains('.')))
    .then_some(target)
}

fn resolve_import(
    target: &str,
    base: &Path,
    root: &Path,
    policy: &PrivacyPolicy,
) -> Option<PathBuf> {
    // `~/...` names a file in the user's home, never in the project.
    if target.starts_with('~') {
        return None;
    }
    let path = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        base.join(target)
    };
    // Canonical, so neither `..` nor a symbolic link leads out of the project.
    let path = path.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    let allowed_place = path.starts_with(&root);
    let text_file = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "md" | "mdc" | "txt"));
    let meta = std::fs::metadata(&path).ok()?;
    (allowed_place
        && text_file
        && meta.is_file()
        && meta.len() <= MAX_FILE_BYTES
        && !policy.is_denied(&path))
    .then_some(path)
}

/// Front matter of a rules file: `key: value` lines, with YAML lists
/// (`- item` lines or `[a, b]`) for list keys.
#[derive(Debug, Default)]
struct FrontMatter(Vec<(String, Vec<String>)>);

impl FrontMatter {
    fn get(&self, key: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.first().cloned())
            .filter(|v| !v.is_empty())
    }

    /// A key's values: a list, or a comma-separated string.
    fn list(&self, key: &str) -> Vec<String> {
        let Some((_, values)) = self.0.iter().find(|(k, _)| k == key) else {
            return Vec::new();
        };
        values
            .iter()
            .flat_map(|v| v.split(','))
            .map(|v| unquote(v.trim()).to_string())
            .filter(|v| !v.is_empty())
            .collect()
    }
}

/// (front matter, body, line of the file the body starts on).
fn front_matter(content: &str) -> (FrontMatter, &str, usize) {
    let mut meta = FrontMatter::default();
    let mut lines = content.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return (meta, content, 1);
    }
    let mut offset = content.find('\n').map_or(content.len(), |i| i + 1);
    let mut line_no = 1;
    for line in lines {
        offset += line.len();
        line_no += 1;
        let line = line.trim_end();
        if line == "---" {
            let body = &content[offset..];
            // The body starts after the blank lines that follow the fence.
            let trimmed = body.trim_start_matches(['\n', '\r']);
            let skipped = body[..body.len() - trimmed.len()].matches('\n').count();
            return (meta, trimmed, line_no + 1 + skipped);
        }
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            if let Some((_, values)) = meta.0.last_mut() {
                values.push(unquote(item.trim()).to_string());
            }
        } else if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            let values = match value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
                Some(inner) => inner
                    .split(',')
                    .map(|v| unquote(v.trim()).to_string())
                    .collect(),
                None if value.is_empty() => Vec::new(),
                None => vec![unquote(value).to_string()],
            };
            meta.0.push((key.trim().to_string(), values));
        }
    }
    // No closing fence: not front matter.
    (FrontMatter::default(), content, 1)
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value)
}

/// The first non-empty line of `text`, without Markdown heading marks.
fn first_line_of(text: &str) -> Option<String> {
    text.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

fn shorten(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut = crate::text::truncate_bytes(&one_line, MAX_ABOUT_BYTES);
    if cut.len() < one_line.len() {
        format!("{cut}…")
    } else {
        one_line
    }
}

/// Text compared for copies: line endings and surrounding blank space don't
/// count.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

fn relative<'a>(root: &Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(root).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Project {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    impl Project {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            Self { _dir: dir, root }
        }

        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }

        fn rules(&self) -> ProjectRules {
            discover(&self.root)
        }
    }

    fn texts(rules: &ProjectRules) -> Vec<String> {
        rules
            .inline
            .iter()
            .map(|b| format!("{}{}", b.heading, b.text))
            .collect()
    }

    fn listed(rules: &ProjectRules) -> Vec<(String, String)> {
        rules
            .on_demand
            .iter()
            .map(|e| {
                (
                    e.path.file_name().unwrap().to_string_lossy().into(),
                    e.about.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn a_projects_rules_cannot_import_the_users_own_files() {
        // A file in the user's home, outside the project: the kind a cloned
        // repository's CLAUDE.md must not be able to pull into requests.
        let Some(home) = dirs::home_dir().and_then(|h| h.canonicalize().ok()) else {
            return;
        };
        let outside = tempfile::tempdir_in(&home).unwrap();
        let secret = outside.path().join("private.md");
        std::fs::write(&secret, "PRIVATE NOTE 7f3a\n").unwrap();
        let rel = secret.strip_prefix(&home).unwrap().display().to_string();

        let p = Project::new();
        p.write("notes/inside.md", "Inside rule.\n");
        p.write(
            "CLAUDE.md",
            &format!(
                "@notes/inside.md\n@~/{rel}\n@{}\n@notes/../../escape.md\n",
                secret.display()
            ),
        );
        let all = texts(&p.rules()).join("\n");
        assert!(all.contains("Inside rule."), "{all}");
        assert!(!all.contains("PRIVATE NOTE 7f3a"), "{all}");
        // The lines stay as written, so the model sees nothing was imported.
        assert!(all.contains(&format!("@~/{rel}")), "{all}");
    }

    #[test]
    fn the_first_root_file_is_inlined_and_other_tools_files_are_listed() {
        let p = Project::new();
        p.write("CLAUDE.md", "Run cargo test before saying it works.\n");
        p.write("AGENTS.md", "Prefer small functions.\n");
        p.write("GEMINI.md", "Run cargo test before saying it works.\r\n\n");
        let rules = p.rules();
        assert_eq!(texts(&rules), ["Run cargo test before saying it works.\n"]);
        // The main file keeps its path, so a shortened block can say where
        // the rest is.
        assert_eq!(rules.inline[0].file, Some((p.root.join("CLAUDE.md"), 1)));
        // GEMINI.md is a copy (line endings aside), so it costs nothing.
        assert_eq!(
            listed(&rules),
            [(
                "AGENTS.md".to_string(),
                "rules written for Codex and other agents; may repeat the rules above".to_string()
            )]
        );
        let menu = rules.menu().unwrap();
        assert_eq!(
            menu,
            format!(
                "# More project rules\nNot loaded. Before work one covers, read it with \
                 read_file.\n- {}: rules written for Codex and other agents; may repeat the rules \
                 above\n",
                p.root.join("AGENTS.md").display()
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_copy_and_a_file_that_only_imports_the_main_one_are_dropped() {
        let p = Project::new();
        p.write("AGENTS.md", "Use tabs.\n");
        std::os::unix::fs::symlink(p.root.join("AGENTS.md"), p.root.join("CLAUDE.md")).unwrap();
        p.write("GEMINI.md", "@AGENTS.md\n");
        let rules = p.rules();
        assert_eq!(texts(&rules), ["Use tabs.\n"]);
        assert!(rules.on_demand.is_empty(), "{:?}", rules.on_demand);
        assert_eq!(rules.menu(), None);
    }

    #[test]
    fn imports_are_expanded_outside_code_fences_and_only_for_allowed_text_files() {
        let p = Project::new();
        p.write("docs/style.md", "Wrap at 100 columns.\n@nested.md\n");
        p.write("docs/nested.md", "No unwrap in library code.\n");
        p.write("secrets.json", "{\"key\": 1}\n");
        p.write(
            "CLAUDE.md",
            "# Rules\n@docs/style.md\n```\n@docs/style.md\n```\n@secrets.json\n@missing.md\nmail @someone\n",
        );
        let rules = p.rules();
        assert_eq!(
            texts(&rules),
            ["# Rules\nWrap at 100 columns.\nNo unwrap in library code.\n```\n@docs/style.md\n```\n\
              @secrets.json\n@missing.md\nmail @someone\n"]
        );
        // Its lines are no longer the file's, so it names no line numbers.
        assert_eq!(rules.inline[0].file, None);
    }

    #[test]
    fn an_import_cycle_stops() {
        let p = Project::new();
        p.write("a.md", "A\n@b.md\n");
        p.write("b.md", "B\n@a.md\n");
        p.write("CLAUDE.md", "@a.md\n");
        assert_eq!(texts(&p.rules()), ["A\nB\n@a.md\n"]);
    }

    #[test]
    fn claude_local_md_goes_with_the_main_file() {
        let p = Project::new();
        p.write("CLAUDE.md", "Team rules.\n");
        p.write("CLAUDE.local.md", "My sandbox URL is local.\n");
        assert_eq!(
            texts(&p.rules()),
            [
                "Team rules.\n",
                "# CLAUDE.local.md\nMy sandbox URL is local.\n"
            ]
        );
    }

    #[test]
    fn cursor_rules_are_inlined_listed_or_skipped_by_their_front_matter() {
        let p = Project::new();
        let always = p.write(
            ".cursor/rules/style.mdc",
            "---\ndescription: House style\nglobs:\nalwaysApply: true\n---\n\nUse 4 spaces.\n",
        );
        p.write(
            ".cursor/rules/react.mdc",
            "---\ndescription: \"React component conventions\"\nglobs: src/**/*.tsx, src/**/*.jsx\nalwaysApply: false\n---\nHooks first.\n",
        );
        p.write(
            ".cursor/rules/db/migrations.mdc",
            "---\nglobs:\n  - \"migrations/**\"\n---\nNever edit an applied migration.\n",
        );
        p.write(
            ".cursor/rules/manual.mdc",
            "---\ndescription:\nglobs:\nalwaysApply: false\n---\nOnly when attached.\n",
        );

        // With no main rules file, the always-on rule is the project's rules.
        let rules = p.rules();
        assert_eq!(
            texts(&rules),
            ["# .cursor/rules/style.mdc\nUse 4 spaces.\n"]
        );
        // The body starts on line 7 of the file, after the fence and a blank.
        assert_eq!(rules.inline[0].file, Some((always.clone(), 7)));
        assert_eq!(
            listed(&rules),
            [
                ("migrations.mdc".into(), "files migrations/**".into()),
                (
                    "react.mdc".into(),
                    "React component conventions (files src/**/*.tsx, src/**/*.jsx)".into()
                ),
            ]
        );

        // With one, it is listed as always on instead of being paid for twice.
        p.write("AGENTS.md", "Main rules.\n");
        let rules = p.rules();
        assert_eq!(texts(&rules), ["Main rules.\n"]);
        assert_eq!(
            listed(&rules).last().unwrap(),
            &(
                "style.mdc".into(),
                "always on in Cursor: House style".into()
            )
        );
    }

    #[test]
    fn copilot_windsurf_and_cline_rules_are_read() {
        let p = Project::new();
        p.write("AGENTS.md", "Main.\n");
        p.write(
            ".github/instructions/tests.instructions.md",
            "---\napplyTo: \"**/*_test.go\"\ndescription: Go test conventions\n---\nTable tests.\n",
        );
        p.write(
            ".github/instructions/all.instructions.md",
            "---\napplyTo: \"**\"\n---\n# Everywhere\nBe brief.\n",
        );
        p.write(
            ".github/instructions/notes.md",
            "Not an instructions file.\n",
        );
        p.write(
            ".windsurf/rules/api.md",
            "---\ntrigger: model_decision\ndescription: When changing the HTTP API\n---\nVersion every route.\n",
        );
        p.write(
            ".windsurf/rules/hand.md",
            "---\ntrigger: manual\n---\nOnly by hand.\n",
        );
        p.write(
            ".clinerules/01-general.md",
            "# General\nKeep diffs small.\n",
        );
        p.write(
            ".clinerules/web.md",
            "---\npaths:\n  - \"web/**\"\n---\nUse the design tokens.\n",
        );
        let rules = p.rules();
        assert_eq!(
            listed(&rules),
            [
                ("api.md".into(), "When changing the HTTP API".into()),
                (
                    "tests.instructions.md".into(),
                    "Go test conventions (files **/*_test.go)".into()
                ),
                ("web.md".into(), "files web/**".into()),
                (
                    "all.instructions.md".into(),
                    "always on in GitHub Copilot: Everywhere".into()
                ),
                ("01-general.md".into(), "always on in Cline: General".into()),
            ]
        );
    }

    #[test]
    fn rules_in_subdirectories_are_listed_for_work_under_them() {
        let p = Project::new();
        p.write(".gitignore", "build/\n");
        std::fs::create_dir_all(p.root.join(".git")).unwrap();
        p.write("CLAUDE.md", "Root.\n");
        p.write("crates/api/AGENTS.md", "API rules.\n");
        p.write("crates/api/CLAUDE.md", "API rules.\n");
        p.write("web/CLAUDE.md", "Web rules.\n");
        p.write(
            "web/.cursor/rules/ui.mdc",
            "---\ndescription: UI kit usage\n---\nUse the kit.\n",
        );
        p.write("build/AGENTS.md", "Generated.\n");
        p.write("node_modules/pkg/AGENTS.md", "A dependency's rules.\n");
        p.write(".git/AGENTS.md", "Not a rules file.\n");
        let rules = p.rules();
        assert_eq!(
            listed(&rules),
            [
                (
                    "AGENTS.md".into(),
                    "rules for work under crates/api/".into()
                ),
                ("CLAUDE.md".into(), "rules for work under web/".into()),
                ("ui.mdc".into(), "UI kit usage for work under web/".into()),
            ]
        );
    }

    #[test]
    fn denied_files_are_neither_read_nor_listed() {
        let p = Project::new();
        p.write(
            ".phazeai/privacy.toml",
            "deny = [\"CLAUDE.md\", \"private/**\"]\n",
        );
        p.write("CLAUDE.md", "Secret rules.\n");
        p.write("AGENTS.md", "Open rules.\n@private/notes.md\n");
        p.write("private/notes.md", "Acquisition target.\n");
        p.write("private/AGENTS.md", "Private team rules.\n");
        let rules = p.rules();
        assert_eq!(texts(&rules), ["Open rules.\n@private/notes.md\n"]);
        assert!(rules.on_demand.is_empty(), "{:?}", rules.on_demand);
    }

    #[test]
    fn a_long_menu_is_capped() {
        let p = Project::new();
        for i in 0..(MAX_MENU_ENTRIES + 5) {
            p.write(&format!("pkg{i:02}/AGENTS.md"), &format!("Rules {i}.\n"));
        }
        let menu = p.rules().menu().unwrap();
        assert_eq!(menu.lines().count(), 2 + MAX_MENU_ENTRIES + 1);
        assert!(menu.ends_with("- 5 more rules files in subdirectories are not listed.\n"));
        // Sorted, so the prompt is the same on every request.
        assert_eq!(menu, p.rules().menu().unwrap());
    }

    #[test]
    fn front_matter_without_a_closing_fence_is_text() {
        let (meta, body, line) = front_matter("---\nnot: front matter\n");
        assert!(meta.0.is_empty());
        assert_eq!((body, line), ("---\nnot: front matter\n", 1));
        let (meta, body, line) = front_matter("---\nglobs: [a/**, 'b/*']\n---\nBody\n");
        assert_eq!(meta.list("globs"), ["a/**", "b/*"]);
        assert_eq!((body, line), ("Body\n", 4));
    }
}
