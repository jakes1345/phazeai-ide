use std::path::{Path, PathBuf};

use crate::llm::context_budget::MIN_BUDGET;

/// Share of the context budget that instructions (project instruction files,
/// an active skill) may take in a system prompt.
///
/// A quarter, because the rest of the budget has to carry everything else a
/// request holds: the fixed part of the prompt (identity, how to work, safety
/// rules: about 260 tokens), the tool definitions (about 1.2k tokens, over a
/// quarter of the ~4k budget a local Ollama or LM Studio model gets), the
/// question, and tool calls with their results.
/// Compaction keeps 40% of the budget as recent messages and lets its summary
/// take 15% (`context::compaction`), which leaves 45% for the whole system
/// prompt: a quarter for instructions and the rest for the fixed part, on
/// every budget above ~8k tokens. On the smallest budgets nothing fits all of
/// that, and the trim cuts tool output down, but it still reaches the model
/// (an over-budget system prompt left it a bare marker). A cloud model's
/// budget (~190k tokens for Claude) allows ~140 KB of instructions, more than
/// any real instructions file, so those models get them in full, as before.
pub const INSTRUCTIONS_RATIO: f64 = 0.25;

/// Bytes per token in the estimate `ConversationHistory::estimate_tokens`
/// makes (~3 for code-heavy text), which decides what fits the budget.
const BYTES_PER_TOKEN: usize = 3;

/// Bytes the instructions may take in the system prompt of a model with
/// `budget` tokens of context: [`INSTRUCTIONS_RATIO`] of it, counted the way
/// the conversation history counts tokens.
pub fn instructions_allowance(budget: usize) -> usize {
    ((budget as f64 * INSTRUCTIONS_RATIO) as usize).saturating_mul(BYTES_PER_TOKEN)
}

/// One block of instructions for a system prompt: an instructions file, an
/// active skill, or text given directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instructions {
    /// Printed before `text` (e.g. `# From /dir\n`); not part of the file.
    pub heading: String,
    /// The instructions themselves.
    pub text: String,
    /// The file `text` was read from and the line of that file `text` starts
    /// on (1 for a whole file), so a shortened block can say where the rest
    /// is. `None` for text that was given directly.
    pub file: Option<(PathBuf, usize)>,
}

impl Instructions {
    /// Text given directly, not read from a file.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            heading: String::new(),
            text: text.into(),
            file: None,
        }
    }

    /// `text`, read from `path` starting at line `first_line` (1-based).
    pub fn from_file(path: impl Into<PathBuf>, first_line: usize, text: impl Into<String>) -> Self {
        Self {
            heading: String::new(),
            text: text.into(),
            file: Some((path.into(), first_line.max(1))),
        }
    }

    pub fn with_heading(mut self, heading: impl Into<String>) -> Self {
        self.heading = heading.into();
        self
    }
}

/// Separator between instruction blocks.
const BLOCK_SEPARATOR: &str = "\n\n---\n\n";

/// `blocks`, joined by a separator, in at most `max_bytes`.
///
/// When everything fits it is returned as it is. Otherwise each block gets an
/// equal share of the room, and a block that needs less than its share leaves
/// the rest to the others, so a short file is never cut because a long one is
/// there. A block over its share keeps its heading and as many whole lines as
/// fit, followed by a marker that says it was shortened, which lines are not
/// shown and, for a file the tools can open, the `read_file` call that reads
/// them.
pub fn fit_instructions(blocks: &[Instructions], max_bytes: usize) -> String {
    let whole: Vec<String> = blocks
        .iter()
        .map(|b| format!("{}{}", b.heading, b.text))
        .collect();
    let separators = BLOCK_SEPARATOR.len() * blocks.len().saturating_sub(1);
    if whole.iter().map(String::len).sum::<usize>() + separators <= max_bytes {
        return whole.join(BLOCK_SEPARATOR);
    }
    // Equal shares, smallest block first; what a block doesn't need goes back
    // to the ones still to come.
    let mut shares = vec![0; blocks.len()];
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by_key(|&i| whole[i].len());
    let mut left = max_bytes.saturating_sub(separators);
    for (done, &i) in order.iter().enumerate() {
        shares[i] = whole[i].len().min(left / (blocks.len() - done));
        left -= shares[i];
    }
    let fitted = blocks
        .iter()
        .zip(whole)
        .zip(shares)
        .map(|((block, whole), share)| {
            if whole.len() <= share {
                whole
            } else {
                shorten(block, share)
            }
        })
        .collect::<Vec<_>>()
        .join(BLOCK_SEPARATOR);
    if fitted.len() <= max_bytes {
        fitted
    } else {
        // Many blocks in very little room: their markers alone don't fit.
        let note = "[... The rest of these instructions is left out to fit this model's \
                    context budget.]";
        if max_bytes < note.len() {
            return String::new();
        }
        format!("{}{note}", whole_lines(&fitted, max_bytes - note.len()))
    }
}

/// `block` in at most `share` bytes (unless even its heading and marker are
/// longer): its heading, the whole lines of its text that fit, and a marker
/// for the lines left out.
fn shorten(block: &Instructions, share: usize) -> String {
    let readable = block
        .file
        .as_ref()
        .is_some_and(|(path, _)| tools_can_read(path));
    let lines = block.text.lines().count();
    // The marker is longest with the largest line numbers.
    let marker_max = shortened_marker(block, readable, lines, lines).len();
    let kept = whole_lines(
        &block.text,
        share.saturating_sub(block.heading.len() + marker_max),
    );
    // `kept` ends a line, so it holds one line per line break.
    let first_left_out = kept.matches('\n').count() + 1;
    let marker = shortened_marker(block, readable, first_left_out, lines);
    format!("{}{kept}{marker}", block.heading)
}

/// The longest start of `text` that ends a line and is at most `max` bytes.
fn whole_lines(text: &str, max: usize) -> &str {
    let end = text.as_bytes()[..max.min(text.len())]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    &text[..end]
}

/// What a shortened block ends with: lines `first`..=`last` of its text are
/// not shown. Line numbers are the file's, so they work as a `read_file`
/// offset. `readable` says whether the file tools may open the file.
fn shortened_marker(block: &Instructions, readable: bool, first: usize, last: usize) -> String {
    let Some((path, start)) = &block.file else {
        return format!(
            "[... These instructions are shortened here to fit this model's context budget: \
             lines {first}-{last} of them are not shown.]"
        );
    };
    let (first, last) = (
        (start + first).saturating_sub(1),
        (start + last).saturating_sub(1),
    );
    let name = path.display();
    if readable {
        let json_path = serde_json::Value::String(path.to_string_lossy().into_owned());
        format!(
            "[... {name} is shortened here to fit this model's context budget: lines \
             {first}-{last} are not shown. To read them, call read_file with \
             {{\"path\": {json_path}, \"offset\": {first}}}.]"
        )
    } else {
        format!(
            "[... {name} is shortened here to fit this model's context budget: lines \
             {first}-{last} are not shown. It is outside the workspace, so read_file cannot \
             open it.]"
        )
    }
}

/// Whether `read_file` may open `path`: the file tools are confined to the
/// workspace (when one is set), and instruction files can live above it.
fn tools_can_read(path: &Path) -> bool {
    crate::tools::sandbox::resolve_within_workspace("read_file", &path.to_string_lossy()).is_ok()
}

/// Every instruction block for the project at `root`, in prompt order:
/// its rules ([`super::rules`]: the main rules file and what goes with it),
/// `CLAUDE.md` and `AGENTS.md` files in the directories above it (up to the
/// home directory), the user's global `~/.phazeai/instructions.md`, and last
/// the menu of rules read on demand. A block whose text another block
/// already holds is left out.
pub fn project_instructions(root: &Path) -> Vec<Instructions> {
    let rules = super::rules::discover(root);
    let menu = rules.menu();
    let mut blocks = rules.inline;
    let mut seen: std::collections::HashSet<String> =
        blocks.iter().map(|b| b.text.trim().to_string()).collect();
    let mut push = |blocks: &mut Vec<Instructions>, block: Instructions| {
        if seen.insert(block.text.trim().to_string()) {
            blocks.push(block);
        }
    };

    // CLAUDE.md and AGENTS.md in the directories above the project.
    let policy = crate::privacy::policy_for(root);
    let home = dirs::home_dir();
    for dir in root.ancestors().skip(1).take(6) {
        for file in ["CLAUDE.md", "AGENTS.md"] {
            let path = dir.join(file);
            if path.is_file() && !policy.is_denied(&path) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    push(
                        &mut blocks,
                        Instructions::from_file(&path, 1, content)
                            .with_heading(format!("# From {}\n", dir.display())),
                    );
                }
            }
        }
        if home.as_deref() == Some(dir) {
            break;
        }
    }

    // The user's own instructions for every project.
    if let Some(home) = home {
        let global = home.join(".phazeai").join("instructions.md");
        if let Ok(content) = std::fs::read_to_string(&global) {
            push(
                &mut blocks,
                Instructions::from_file(&global, 1, content)
                    .with_heading("# Global instructions\n"),
            );
        }
    }

    if let Some(menu) = menu {
        blocks.push(Instructions::new(menu));
    }
    blocks
}

/// Builds the system prompt for the AI coding assistant.
/// Incorporates project context and user instructions.
///
/// The prompt is sized to the model's context budget
/// ([`Self::with_context_budget`]): instructions take at most
/// [`INSTRUCTIONS_RATIO`] of it (see [`fit_instructions`]), so a long
/// instructions file can't leave a small local model's requests over budget
/// before the conversation even starts.
#[derive(Debug, Clone)]
pub struct SystemPromptBuilder {
    project_root: Option<PathBuf>,
    project_type: Option<ProjectType>,
    git_branch: Option<String>,
    git_dirty_files: Vec<String>,
    instructions: Vec<Instructions>,
    sections: Vec<String>,
    model_name: String,
    provider_name: String,
    context_budget: usize,
}

#[derive(Debug, Clone)]
pub enum ProjectType {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    Java,
    CSharp,
    Cpp,
    Ruby,
    Mixed(Vec<String>),
    Unknown,
}

impl ProjectType {
    pub fn name(&self) -> &str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Go => "Go",
            Self::Java => "Java",
            Self::CSharp => "C#",
            Self::Cpp => "C/C++",
            Self::Ruby => "Ruby",
            Self::Mixed(_) => "Multi-language",
            Self::Unknown => "Unknown",
        }
    }

    /// Detect project type from files in the root directory.
    pub fn detect(root: &Path) -> Self {
        let mut types = Vec::new();

        if root.join("Cargo.toml").exists() {
            types.push("Rust".to_string());
        }
        if root.join("pyproject.toml").exists()
            || root.join("setup.py").exists()
            || root.join("requirements.txt").exists()
        {
            types.push("Python".to_string());
        }
        if root.join("package.json").exists() {
            if root.join("tsconfig.json").exists() {
                types.push("TypeScript".to_string());
            } else {
                types.push("JavaScript".to_string());
            }
        }
        if root.join("go.mod").exists() {
            types.push("Go".to_string());
        }
        if root.join("pom.xml").exists() || root.join("build.gradle").exists() {
            types.push("Java".to_string());
        }
        if root.join("CMakeLists.txt").exists() || root.join("Makefile").exists() {
            // Only detect C++ if no other more specific type found
            if types.is_empty() {
                types.push("C/C++".to_string());
            }
        }
        if root.join("Gemfile").exists() {
            types.push("Ruby".to_string());
        }

        match types.len() {
            0 => Self::Unknown,
            1 => match types[0].as_str() {
                "Rust" => Self::Rust,
                "Python" => Self::Python,
                "JavaScript" => Self::JavaScript,
                "TypeScript" => Self::TypeScript,
                "Go" => Self::Go,
                "Java" => Self::Java,
                "C/C++" => Self::Cpp,
                "Ruby" => Self::Ruby,
                _ => Self::Unknown,
            },
            _ => Self::Mixed(types),
        }
    }
}

impl SystemPromptBuilder {
    pub fn new() -> Self {
        Self {
            project_root: None,
            project_type: None,
            git_branch: None,
            git_dirty_files: Vec::new(),
            instructions: Vec::new(),
            sections: Vec::new(),
            model_name: String::new(),
            provider_name: String::new(),
            context_budget: MIN_BUDGET,
        }
    }

    /// Size the prompt for a model with `budget` tokens of context (what
    /// `Settings::context_budget()` gives the agent). Until this is called the
    /// smallest budget any model gets ([`MIN_BUDGET`]) is assumed, so a prompt
    /// built without it still fits every model.
    pub fn with_context_budget(mut self, budget: usize) -> Self {
        self.context_budget = budget;
        self
    }

    pub fn with_project_root(mut self, root: PathBuf) -> Self {
        self.project_type = Some(ProjectType::detect(&root));
        self.project_root = Some(root);
        self
    }

    pub fn with_git_info(mut self, branch: Option<String>, dirty_files: Vec<String>) -> Self {
        self.git_branch = branch;
        self.git_dirty_files = dirty_files;
        self
    }

    pub fn with_custom_instructions(mut self, instructions: String) -> Self {
        self.instructions = vec![Instructions::new(instructions)];
        self
    }

    pub fn with_model(mut self, provider: &str, model: &str) -> Self {
        self.provider_name = provider.to_string();
        self.model_name = model.to_string();
        self
    }

    /// Load the project's rules ([`project_instructions`]), replacing any
    /// instructions set before when there are some. How much of them the
    /// prompt carries depends on the context budget.
    pub fn load_project_instructions(mut self) -> Self {
        if let Some(ref root) = self.project_root {
            let instructions = project_instructions(root);
            if !instructions.is_empty() {
                self.instructions = instructions;
            }
        }
        self
    }

    /// Append additional instructions to existing custom instructions
    pub fn with_additional_instructions(mut self, instructions: String) -> Self {
        self.instructions.push(Instructions::new(instructions));
        self
    }

    /// Add `text` as a section of its own after the instructions and before
    /// the git state, as given: the caller decides its size. Sections come in
    /// the order they were added, a blank line apart; blank text adds none.
    /// The IDE chat puts its mode hint and menu of skills here, so they sit
    /// behind the parts of the prompt that stay the same for the project.
    pub fn with_section(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        let text = text.trim();
        if !text.is_empty() {
            self.sections.push(text.to_string());
        }
        self
    }

    pub fn build(&self) -> String {
        let mut prompt = String::with_capacity(4096);

        prompt.push_str(CORE_PROMPT);

        // Project context
        if let Some(ref root) = self.project_root {
            prompt.push_str("\n\n## Project\n");
            prompt.push_str(&format!("- Working directory: {}\n", root.display()));
            if let Some(ref pt) = self.project_type {
                prompt.push_str(&format!("- Project type: {}\n", pt.name()));
            }
        }

        // Custom project instructions, heading included, within their share
        // of the context budget.
        if !self.instructions.is_empty() {
            let room = instructions_allowance(self.context_budget)
                .saturating_sub(INSTRUCTIONS_HEADING.len());
            prompt.push_str(INSTRUCTIONS_HEADING);
            prompt.push_str(&fit_instructions(&self.instructions, room));
        }

        for section in &self.sections {
            prompt.push_str("\n\n");
            prompt.push_str(section);
        }

        // Git state last. It changes as files are edited and committed, while
        // everything above stays the same for the project, and a provider's
        // prompt cache only reuses the part before the first difference.
        if self.project_root.is_some()
            && (self.git_branch.is_some() || !self.git_dirty_files.is_empty())
        {
            prompt.push_str(GIT_HEADING);
            if let Some(ref branch) = self.git_branch {
                prompt.push_str(&format!("- Git branch: {}\n", branch));
            }
            if !self.git_dirty_files.is_empty() {
                prompt.push_str(&format!(
                    "- Modified files: {}\n",
                    self.git_dirty_files.join(", ")
                ));
            }
        }

        prompt
    }
}

const INSTRUCTIONS_HEADING: &str = "\n\n## Project-Specific Instructions\n";

/// Title of the project's instructions in a system prompt (as above, without
/// the blank lines that separate it from the section before).
pub const INSTRUCTIONS_TITLE: &str = "## Project-Specific Instructions\n";

/// Heading of the git state, the last section of the system prompt.
pub const GIT_HEADING: &str = "\n\n## Git Status\n";

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// The fixed part of every system prompt: who the agent is, how it works,
/// and its safety rules. Kept short on purpose: it is sent with every request,
/// and the tools describe themselves in their own definitions, so the prompt
/// neither lists them nor shows how to call them (see
/// `docs/token-efficiency-spec.md`, T4).
const CORE_PROMPT: &str = "\
You are PhazeAI, a coding agent in the user's IDE. You can read and change the project's files \
and run commands. Do the task; don't just describe it.

## How to work
- Find code with `grep` and `glob`, and read a file with `read_file` before you edit it.
- Make the smallest change that does the job, in the style of the code around it.
- Use `edit_file` for existing files; `write_file` is for new ones.
- Make independent reads and searches in the same turn.
- After a change, run the build or tests that cover it with `bash` and fix what fails.
- For a task of several steps, give a short plan first.
- Reply briefly: what you did and what is left.

## Safety
- Ask before deleting large parts of the project or running destructive commands \
(`rm -rf`, `DROP`, force-push).
- Never read, print or write secrets such as `.env` files or API keys.
- If you are stuck after a few tries, stop and ask the user.";

/// Collect git info for the system prompt.
///
/// The modified-files list leaves out paths the project's privacy policy
/// denies (the names of secret files are themselves information the model
/// shouldn't see); status lines whose path can't be parsed are left out too.
pub fn collect_git_info(root: &Path) -> (Option<String>, Vec<String>) {
    let git_stdout = |args: &[&str]| {
        crate::process::std_command("git")
            .args(crate::privacy::GIT_NO_QUOTE_PATH)
            .args(args)
            .current_dir(root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };

    let branch = git_stdout(&["branch", "--show-current"]).map(|s| s.trim().to_string());

    let dirty_files = git_stdout(&["status", "--porcelain", "--short"])
        .map(|status| {
            // Porcelain paths are relative to the repository top level, which
            // may be above `root`.
            let repo_root = git_stdout(&["rev-parse", "--show-toplevel"])
                .map(|s| PathBuf::from(s.trim()))
                .filter(|p| p.is_dir())
                .unwrap_or_else(|| root.to_path_buf());
            let policy = crate::privacy::policy_for(root);
            let (kept, omitted) =
                crate::privacy::filter_status_porcelain(&status, &repo_root, &policy);
            for path in &omitted {
                crate::privacy::audit::record_blocked_file(
                    path,
                    "left out of the system prompt's modified-files list",
                    "system prompt",
                );
            }
            kept.iter()
                .take(20) // Limit to 20 files
                .map(|l| l.trim().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    (branch, dirty_files)
}

#[cfg(test)]
pub(crate) mod tests {

    #[test]
    fn every_tool_the_prompt_names_is_one_the_agent_has() {
        // A name the model can't call turns into a failed call.
        let tools = crate::tools::ToolRegistry::default();
        let named: Vec<&str> = CORE_PROMPT
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|w| w.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            .collect();
        assert!(named.len() >= 6, "{named:?}");
        for name in named {
            assert!(tools.get(name).is_some(), "the prompt names `{name}`");
        }
    }

    #[test]
    fn the_fixed_part_of_the_prompt_stays_small() {
        // Sent with every request: about 300 tokens at ~4 bytes a token.
        assert!(CORE_PROMPT.len() <= 1_300, "{} bytes", CORE_PROMPT.len());
    }

    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git must be installed");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn git_status_in_system_prompt_omits_denied_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "t@t"]);
        git(&root, &["config", "user.name", "t"]);
        // Force quoting on, so the test proves `-c core.quotePath=false` wins.
        git(&root, &["config", "core.quotePath", "true"]);
        for (rel, body) in [
            ("src/main.rs", "fn main() {}"),
            ("café.txt", "1"),
            (".env", "A=1"),
            ("private/notes.md", "1"),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "init"]);
        for rel in ["src/main.rs", "café.txt", ".env", "private/notes.md"] {
            std::fs::write(root.join(rel), "changed").unwrap();
        }
        std::fs::create_dir_all(root.join(".phazeai")).unwrap();
        std::fs::write(
            root.join(crate::privacy::POLICY_FILE),
            "deny = [\"private/**\"]\n",
        )
        .unwrap();

        let (branch, dirty) = collect_git_info(&root);
        assert!(branch.is_some());
        let joined = dirty.join("\n");
        assert!(dirty.iter().any(|l| l == "M src/main.rs"), "{joined}");
        assert!(
            dirty.iter().any(|l| l == "M café.txt"),
            "unquoted: {joined}"
        );
        assert!(
            dirty.iter().any(|l| l.starts_with("?? .phazeai")),
            "{joined}"
        );
        assert!(!joined.contains(".env"), "{joined}");
        assert!(!joined.contains("private"), "{joined}");

        // The same list goes into the prompt text.
        let prompt = SystemPromptBuilder::new()
            .with_project_root(root.clone())
            .with_git_info(branch, dirty)
            .build();
        let line = prompt
            .lines()
            .find(|l| l.starts_with("- Modified files:"))
            .expect("modified files line");
        assert!(line.contains("café.txt"), "{line}");
        assert!(
            !line.contains(".env") && !line.contains("private"),
            "{line}"
        );
    }

    #[test]
    fn git_state_comes_after_everything_that_does_not_change() {
        let builder = |dirty: Vec<String>| {
            SystemPromptBuilder::new()
                .with_project_root(PathBuf::from("/work/app"))
                .with_additional_instructions("Run cargo fmt before committing.".into())
                .with_git_info(Some("main".into()), dirty)
                .build()
        };
        let clean = builder(vec![]);
        let edited = builder(vec!["src/lib.rs".into(), "tests/a.rs".into()]);

        // The prompt is the same up to the git section, which ends it.
        let at = clean.find(GIT_HEADING).expect("a git section");
        assert_eq!(edited.find(GIT_HEADING), Some(at));
        assert_eq!(clean[..at], edited[..at]);
        assert!(clean[..at].contains("Run cargo fmt before committing."));
        assert!(clean[..at].contains("- Working directory: /work/app"));
        assert_eq!(&clean[at..], "\n\n## Git Status\n- Git branch: main\n");
        assert_eq!(
            &edited[at..],
            "\n\n## Git Status\n- Git branch: main\n- Modified files: src/lib.rs, tests/a.rs\n"
        );

        // No git, no section.
        let no_git = SystemPromptBuilder::new()
            .with_project_root(PathBuf::from("/work/app"))
            .build();
        assert!(!no_git.contains(GIT_HEADING));
    }

    #[test]
    fn sections_follow_the_instructions_in_order_and_precede_the_git_state() {
        let prompt = SystemPromptBuilder::new()
            .with_project_root(PathBuf::from("/work/app"))
            .with_additional_instructions("Run cargo fmt before committing.".into())
            .with_section("Answer concisely.\n\n")
            .with_section("  \n")
            .with_section("- **reviewer** (project) — Reviews changes")
            .with_git_info(Some("main".into()), vec![])
            .build();
        let without = SystemPromptBuilder::new()
            .with_project_root(PathBuf::from("/work/app"))
            .with_additional_instructions("Run cargo fmt before committing.".into())
            .build();
        assert_eq!(
            prompt,
            format!(
                "{without}\n\nAnswer concisely.\n\n- **reviewer** (project) — Reviews \
                 changes\n\n## Git Status\n- Git branch: main\n"
            )
        );
    }

    // ── Instructions sized to the context budget ────────────────────────────

    /// Holds the file tools' workspace (a process-wide setting other tests
    /// change too) at `root` until dropped.
    pub(crate) struct Workspace {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<PathBuf>,
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            crate::tools::sandbox::set_workspace_root(self.prev.take());
        }
    }

    pub(crate) fn workspace(root: &Path) -> Workspace {
        let lock = crate::tools::sandbox::tests::SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = crate::tools::sandbox::workspace_root();
        crate::tools::sandbox::set_workspace_root(Some(root.to_path_buf()));
        Workspace { _lock: lock, prev }
    }

    /// Numbered instruction lines, at least `bytes` long in all.
    fn rules(bytes: usize) -> String {
        let mut text = String::new();
        let mut n = 0;
        while text.len() < bytes {
            n += 1;
            text.push_str(&format!(
                "- Rule {n:04}: run the tests before saying that a change works.\n"
            ));
        }
        text
    }

    /// A project whose CLAUDE.md holds `rules(bytes)`.
    fn project(bytes: usize) -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let text = rules(bytes);
        std::fs::write(root.join("CLAUDE.md"), &text).unwrap();
        (dir, root, text)
    }

    #[test]
    fn a_long_instructions_file_is_cut_to_its_share_of_a_small_budget() {
        let (_dir, root, text) = project(20 * 1024);
        let _ws = workspace(&root);
        let budget = 4_096;
        let prompt = SystemPromptBuilder::new()
            .with_project_root(root.clone())
            .load_project_instructions()
            .with_context_budget(budget)
            .build();
        let without = SystemPromptBuilder::new()
            .with_project_root(root.clone())
            .build();

        // The instructions come last and, heading and marker included, take
        // at most a quarter of the budget as the history counts tokens.
        let section = prompt.strip_prefix(without.as_str()).unwrap();
        assert!(
            section.len().div_ceil(3) <= budget / 4,
            "the instructions are {} bytes, ~{} tokens",
            section.len(),
            section.len().div_ceil(3)
        );
        // Whole lines from the top of the file, then the marker.
        let body = section.strip_prefix(INSTRUCTIONS_HEADING).unwrap();
        let (kept, marker) = body.split_at(body.find("[... ").expect("a marker"));
        assert!(kept.ends_with('\n') && text.starts_with(kept), "{kept:?}");
        assert!(kept.len() > section.len() / 2, "{} bytes kept", kept.len());
        // It names the file, the lines left out and how to read them.
        let file = root.join("CLAUDE.md");
        let next = kept.lines().count() + 1;
        let total = text.lines().count();
        assert_eq!(
            marker,
            format!(
                "[... {path} is shortened here to fit this model's context budget: lines \
                 {next}-{total} are not shown. To read them, call read_file with \
                 {{\"path\": \"{path}\", \"offset\": {next}}}.]",
                path = file.display()
            )
        );
        assert!(prompt.ends_with(marker));
    }

    #[test]
    fn a_large_budget_gets_the_whole_instructions_file_unchanged() {
        let (_dir, root, text) = project(20 * 1024);
        let prompt = SystemPromptBuilder::new()
            .with_project_root(root.clone())
            .load_project_instructions()
            .with_context_budget(200_000)
            .build();
        let without = SystemPromptBuilder::new()
            .with_project_root(root.clone())
            .build();
        // Exactly what the prompt held before instructions were sized.
        assert_eq!(prompt, format!("{without}{INSTRUCTIONS_HEADING}{text}"));
    }

    #[test]
    fn a_short_block_stays_whole_next_to_a_long_one() {
        let long = rules(20 * 1024);
        let short = "Answer in British English.".to_string();
        let budget = 4_096;
        let prompt = SystemPromptBuilder::new()
            .with_custom_instructions(long.clone())
            .with_additional_instructions(short.clone())
            .with_context_budget(budget)
            .build();
        let section = &prompt[prompt.find(INSTRUCTIONS_HEADING).unwrap()..];
        assert!(section.len() <= instructions_allowance(budget));
        // The short block is whole; the long one is cut before the separator,
        // with a marker for text that has no file to read the rest from.
        let (first, second) = section.split_once(BLOCK_SEPARATOR).unwrap();
        assert_eq!(second, short);
        let kept = first
            .strip_prefix(INSTRUCTIONS_HEADING)
            .unwrap()
            .split("[... ")
            .next()
            .unwrap();
        assert!(long.starts_with(kept) && kept.ends_with('\n'));
        let next = kept.lines().count() + 1;
        assert!(
            first.ends_with(&format!(
                "[... These instructions are shortened here to fit this model's context \
                 budget: lines {next}-{} of them are not shown.]",
                long.lines().count()
            )),
            "{first}"
        );
    }

    #[test]
    fn a_file_the_tools_cannot_open_is_named_without_a_read_file_hint() {
        let (_dir, root, _text) = project(20 * 1024);
        let elsewhere = tempfile::tempdir().unwrap();
        let _ws = workspace(&elsewhere.path().canonicalize().unwrap());
        let prompt = SystemPromptBuilder::new()
            .with_project_root(root.clone())
            .load_project_instructions()
            .with_context_budget(4_096)
            .build();
        let marker = &prompt[prompt.rfind("[... ").unwrap()..];
        assert!(
            marker.ends_with(
                "are not shown. It is outside the workspace, so read_file cannot open it.]"
            ),
            "{marker}"
        );
        assert!(marker.contains(&root.join("CLAUDE.md").display().to_string()));
    }

    #[test]
    fn line_numbers_are_the_files_and_every_layout_fits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let _ws = workspace(&root);
        // A block that starts on line 5 of its file (a skill after its
        // front matter): the offset counts the file's lines.
        let path = root.join("skill.md");
        let block = Instructions::from_file(&path, 5, rules(4_000)).with_heading("# Skill\n\n");
        let out = fit_instructions(std::slice::from_ref(&block), 1_000);
        assert!(out.len() <= 1_000, "{} bytes", out.len());
        let kept = out
            .strip_prefix("# Skill\n\n")
            .unwrap()
            .split("[... ")
            .next()
            .unwrap();
        let offset = 5 + kept.lines().count();
        assert!(out.ends_with(&format!("\"offset\": {offset}}}.]")), "{out}");

        // Many blocks in little room: the markers alone would overflow, and
        // the result still fits.
        let blocks: Vec<Instructions> = (0..30)
            .map(|i| Instructions::from_file(root.join(format!("{i}.md")), 1, rules(2_000)))
            .collect();
        for room in [0, 50, 500, 3_000, 10_000] {
            let out = fit_instructions(&blocks, room);
            assert!(out.len() <= room, "{} bytes in {room}", out.len());
        }
        // Text that fits is returned as it is, blocks joined by the separator.
        let small = [Instructions::new("a\n"), Instructions::new("b")];
        assert_eq!(fit_instructions(&small, 100), "a\n\n\n---\n\nb");
    }
}
