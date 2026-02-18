/// Builds a context string from system prompt, context files, and user query.
pub struct ContextBuilder {
    system_prompt: String,
    context_files: Vec<(String, String)>,
    user_query: String,
    repo_map: Option<String>,
}

impl ContextBuilder {
    pub fn new() -> Self {
        Self {
            system_prompt: Self::default_system_prompt(),
            context_files: Vec::new(),
            user_query: String::new(),
            repo_map: None,
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    pub fn add_context_file(mut self, path: impl Into<String>, content: impl Into<String>) -> Self {
        self.context_files.push((path.into(), content.into()));
        self
    }

    pub fn with_user_query(mut self, query: impl Into<String>) -> Self {
        self.user_query = query.into();
        self
    }

    /// Add a repo map (project-wide symbol summary) to the context.
    /// This gives the agent a bird's-eye view of all functions, classes,
    /// and modules in the project — like Aider's repo map.
    pub fn with_repo_map(mut self, repo_map: impl Into<String>) -> Self {
        self.repo_map = Some(repo_map.into());
        self
    }

    pub fn build(self) -> String {
        let mut context = String::new();

        context.push_str(&self.system_prompt);
        context.push_str("\n\n");

        // Include repo map before files — gives the agent project overview
        if let Some(ref repo_map) = self.repo_map {
            context.push_str("## Repository Map (project symbols):\n\n");
            context.push_str(repo_map);
            context.push_str("\n\n");
        }

        if !self.context_files.is_empty() {
            context.push_str("## Context Files:\n\n");
            for (path, content) in self.context_files {
                context.push_str(&format!("### {}\n```\n{}\n```\n\n", path, content));
            }
        }

        if !self.user_query.is_empty() {
            context.push_str(&format!("## User Query:\n{}\n", self.user_query));
        }

        context
    }


    fn default_system_prompt() -> String {
        "You are PhazeAI, an advanced AI-powered development environment. \
        You help users write, analyze, and refactor code using available tools.\n\
        \n\
        When you need to perform actions:\n\
        1. Use the available tools to interact with the filesystem\n\
        2. Read files before modifying them\n\
        3. Always verify changes by reading the file back\n\
        4. Explain your reasoning clearly"
            .to_string()
    }
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}
