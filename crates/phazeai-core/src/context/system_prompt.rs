use std::path::{Path, PathBuf};

/// Builds the system prompt for the AI coding assistant.
/// Incorporates project context, available tools, and user instructions.
pub struct SystemPromptBuilder {
    project_root: Option<PathBuf>,
    project_type: Option<ProjectType>,
    git_branch: Option<String>,
    git_dirty_files: Vec<String>,
    custom_instructions: Option<String>,
    tool_names: Vec<String>,
    model_name: String,
    provider_name: String,
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
            custom_instructions: None,
            tool_names: Vec::new(),
            model_name: String::new(),
            provider_name: String::new(),
        }
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
        self.custom_instructions = Some(instructions);
        self
    }

    pub fn with_tools(mut self, tool_names: Vec<String>) -> Self {
        self.tool_names = tool_names;
        self
    }

    pub fn with_model(mut self, provider: &str, model: &str) -> Self {
        self.provider_name = provider.to_string();
        self.model_name = model.to_string();
        self
    }

    /// Load custom instructions from CLAUDE.md or .phazeai/instructions.md
    pub fn load_project_instructions(mut self) -> Self {
        if let Some(ref root) = self.project_root {
            let mut instructions = Vec::new();

            // Check project root first (highest priority)
            let root_candidates = [
                root.join(".phazerules"),
                root.join(".cursorrules"),
                root.join("CLAUDE.md"),
                root.join(".phazeai").join("instructions.md"),
                root.join(".phazeai").join("config.md"),
                root.join(".ai").join("instructions.md"),
            ];

            for path in &root_candidates {
                if path.exists() {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        instructions.push(content);
                        break; // Only take the first match at project root
                    }
                }
            }

            // Walk up parent directories for additional CLAUDE.md files
            let mut current = root.parent();
            let mut depth = 0;
            while let Some(dir) = current {
                if depth > 5 { break; } // Don't walk up too far

                let parent_claude = dir.join("CLAUDE.md");
                if parent_claude.exists() {
                    if let Ok(content) = std::fs::read_to_string(&parent_claude) {
                        instructions.push(format!("# From {}\n{}", dir.display(), content));
                    }
                }

                // Stop at home dir or filesystem root
                if let Some(home) = dirs::home_dir() {
                    if dir == home {
                        break;
                    }
                }

                current = dir.parent();
                depth += 1;
            }

            // Also check ~/.phazeai/instructions.md for global user instructions
            if let Some(home) = dirs::home_dir() {
                let global_instructions = home.join(".phazeai").join("instructions.md");
                if global_instructions.exists() {
                    if let Ok(content) = std::fs::read_to_string(&global_instructions) {
                        instructions.push(format!("# Global instructions\n{}", content));
                    }
                }
            }

            if !instructions.is_empty() {
                self.custom_instructions = Some(instructions.join("\n\n---\n\n"));
            }
        }
        self
    }

    /// Append additional instructions to existing custom instructions
    pub fn with_additional_instructions(mut self, instructions: String) -> Self {
        match self.custom_instructions {
            Some(ref mut existing) => {
                existing.push_str("\n\n---\n\n");
                existing.push_str(&instructions);
            }
            None => {
                self.custom_instructions = Some(instructions);
            }
        }
        self
    }

    pub fn build(&self) -> String {
        let mut prompt = String::with_capacity(4096);

        // Core identity
        prompt.push_str(CORE_IDENTITY);

        // Project context
        if let Some(ref root) = self.project_root {
            prompt.push_str("\n\n## Project Context\n");
            prompt.push_str(&format!(
                "- Working directory: {}\n",
                root.display()
            ));
            if let Some(ref pt) = self.project_type {
                prompt.push_str(&format!("- Project type: {}\n", pt.name()));
            }
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

        // Available tools
        if !self.tool_names.is_empty() {
            prompt.push_str("\n\n## Available Tools\n");
            prompt.push_str("You have the following tools available. Use them to help the user:\n");
            for name in &self.tool_names {
                prompt.push_str(&format!("- {}\n", name));
            }
        }

        // Tool usage guidelines
        prompt.push_str(PLANNING_GUIDELINES);
        prompt.push_str(TOOL_GUIDELINES);

        // Safety rules
        prompt.push_str(SAFETY_RULES);

        // Custom project instructions
        if let Some(ref instructions) = self.custom_instructions {
            prompt.push_str("\n\n## Project-Specific Instructions\n");
            prompt.push_str(instructions);
        }

        prompt
    }
}

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

const CORE_IDENTITY: &str = "\
You are PhazeAI, an elite AI coding assistant. You are embedded deep within a developer's IDE, \
possessing full read/write access to the codebase and the ability to execute terminal commands. \
Your goal is to be the ultimate pair programmer: efficient, proactive, and technically flawless.

## Personality & Tone
- **Direct & Technical**: Use professional engineering terminology. Avoid fluff.
- **Concise**: If 10 words do the job, don't use 20.
- **High Agency**: Don't just suggest; do. If a task requires 5 steps, plan and execute them.
- **Flawless Code**: Write clean, idiomatic code. Respect existing patterns.

## Performance Principles
- **Read First**: Always read a file before modifying it.
- **Minimal Changes**: Don't refactor code that doesn't need to be touched.
- **Verification**: After a change, verify it with `cargo check` or relevant tests.
- **Error Handling**: Anticipate and handle edge cases in your code.";

const PLANNING_GUIDELINES: &str = "\n\n## Multi-Turn Planning
Before tackling complex tasks, outline your plan. Follow this workflow:
1. **Analyze**: Use `grep`, `glob`, and `read_file` to understand the architecture.
2. **Plan**: Describe the steps you will take.
3. **Execute**: Modify files one by one, verifying as you go.
4. **Finalize**: Run tests/builds and summarize your accomplishments.";

const TOOL_GUIDELINES: &str = "\n\n## The PhazeAI Arsenal
You have 17 powerful tools at your disposal:

### File System
- `read_file`: Read contents (supports offset/limit for large files).
- `write_file`: Create or overwrite files. Creates parent dirs automatically.
- `edit_file`: Targeted search-and-replace for minimal diffs.
- `find_path`: Regex-based file search (like `find` or `fd`).
- `glob`: Search files using glob patterns.
- `copy_path` / `move_path` / `delete_path`: Manage file lifecycle.
- `create_directory`: Recursive directory creation.

### Research & Analysis
- `grep`: Fast text search within files (regex supported).
- `list_files`: Non-recursive list of a directory.
- `diagnostics`: Run `cargo check` or linters and parse structured errors.
- `now`: Get current time/date for context.

### Execution & External
- `bash`: Run any terminal command. Use for builds, tests, and env setup.
- `fetch`: Make HTTP requests to external APIs or documentation.
- `web_search`: Search the internet via DuckDuckGo for docs and solutions.
- `open`: Open a file or URL in the user's host environment.

## Critical Tool Rules
- **Prefer `edit_file`** over `write_file` for existing files to keep diffs tiny.
- **Always verify** using `bash` after significant changes.
- **Truncate large outputs**: If a tool returns too much data, summarize it.";

const SAFETY_RULES: &str = "\n\n## Safety Rules
- **Safe Deletion**: Refuse to delete critical system paths or huge chunks of the project without confirmation.
- **Destructive Commands**: Always ask before running `rm -rf /`, `DROP DATABASE`, or force pushes.
- **Secrets**: NEVER read or write `.env` files or hardcode API keys.
- **Infinite Loops**: Monitor your own iteration count. If stuck, ask the user for a hint.";

/// Collect git info for the system prompt.
pub fn collect_git_info(root: &Path) -> (Option<String>, Vec<String>) {
    let branch = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(root)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });

    let dirty_files = std::process::Command::new("git")
        .args(["status", "--porcelain", "--short"])
        .current_dir(root)
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .take(20) // Limit to 20 files
                .map(|l| l.trim().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    (branch, dirty_files)
}
