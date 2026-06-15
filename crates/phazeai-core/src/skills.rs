use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where a skill was discovered from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    /// `~/.config/phazeai/skills/`
    Global,
    /// `{workspace}/.phazeai/skills/`
    Project,
}

/// A parsed skill file. Skills are markdown files with optional YAML frontmatter
/// that define an expert persona for the agent to inhabit.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Slug derived from `name:` frontmatter or the filename stem.
    pub name: String,
    /// One-line summary from `description:` frontmatter (used for auto-select menu).
    pub description: String,
    /// Full file content with frontmatter stripped — the expert persona body.
    pub body: String,
    /// Original file path.
    pub path: PathBuf,
    /// Where it came from.
    pub source: SkillSource,
}

impl Skill {
    /// Returns the full system-prompt block to inject when this skill is active.
    pub fn as_system_prompt(&self) -> String {
        format!(
            "# Active Expert Skill: {}\n\n{}\n",
            self.name, self.body
        )
    }

    /// Returns true if this skill's name or description loosely matches `query`.
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.name.to_lowercase().contains(&q)
            || self.description.to_lowercase().contains(&q)
    }
}

/// Discover all skills from both global and project locations.
///
/// Project skills take precedence: if a project skill has the same name as a
/// global skill, the global one is excluded.
pub fn discover_skills(workspace: Option<&Path>) -> Vec<Skill> {
    let global_dir = dirs::config_dir()
        .map(|d| d.join("phazeai").join("skills"));

    let project_dir = workspace.map(|w| w.join(".phazeai").join("skills"));

    let mut skills: Vec<Skill> = Vec::new();

    // Load project skills first so they can shadow globals.
    if let Some(dir) = project_dir {
        skills.extend(load_from_dir(&dir, SkillSource::Project));
    }

    // Load globals, skipping any whose name is already provided by the project.
    if let Some(dir) = global_dir {
        let project_names: std::collections::HashSet<String> =
            skills.iter().map(|s| s.name.clone()).collect();
        for skill in load_from_dir(&dir, SkillSource::Global) {
            if !project_names.contains(&skill.name) {
                skills.push(skill);
            }
        }
    }

    skills
}

/// Find a skill by exact name (case-insensitive) from a list.
pub fn find_skill<'a>(skills: &'a [Skill], name: &str) -> Option<&'a Skill> {
    let lower = name.to_lowercase();
    skills.iter().find(|s| s.name.to_lowercase() == lower)
}

/// Build the auto-select menu block that lists all available skills.
/// Injected into every system prompt so the agent can reference available
/// expert modes without the user needing to know their names.
pub fn skills_menu_block(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Available Expert Skills\n\
         You have access to specialized expert personas below. \
         Draw on the most relevant one automatically, or the user can \
         activate one explicitly with /skill-name.\n\n",
    );
    for skill in skills {
        let src = match skill.source {
            SkillSource::Global => "global",
            SkillSource::Project => "project",
        };
        out.push_str(&format!(
            "- **{}** ({}) — {}\n",
            skill.name, src, skill.description
        ));
    }
    out
}

// ── Internal ──────────────────────────────────────────────────────────────────

fn load_from_dir(dir: &Path, source: SkillSource) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(skill) = parse_skill_file(&path, &content, source.clone()) {
                skills.push(skill);
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Parse a skill markdown file.
/// Extracts `name` and `description` from YAML frontmatter; the rest is the body.
fn parse_skill_file(path: &Path, content: &str, source: SkillSource) -> Option<Skill> {
    let (fm, body) = split_frontmatter(content);

    let name = fm
        .get("name")
        .cloned()
        .filter(|n| !n.is_empty())
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.replace(['-', '_'], " "))
        })?;

    let description = fm
        .get("description")
        .cloned()
        .unwrap_or_else(|| first_sentence(&body));

    Some(Skill {
        name,
        description,
        body: body.trim().to_string(),
        path: path.to_path_buf(),
        source,
    })
}

/// Split `---\nkey: val\n---\nbody` into (frontmatter map, body).
/// If there is no frontmatter the map is empty and the full content is the body.
fn split_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let mut map = HashMap::new();

    if !content.starts_with("---") {
        return (map, content.to_string());
    }

    // Find closing ---
    let after_open = content.get(3..).unwrap_or("");
    let close = after_open.find("\n---");
    let Some(close_idx) = close else {
        return (map, content.to_string());
    };

    let fm_block = &after_open[..close_idx];
    let body = after_open[close_idx + 4..]
        .trim_start_matches('\n')
        .to_string();

    for line in fm_block.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_string();
            // Simple value — strip quotes if present
            let val = v.trim().trim_matches('"').to_string();
            map.insert(key, val);
        }
    }

    (map, body)
}

/// Extract the first sentence (or up to 120 chars) from body text.
fn first_sentence(text: &str) -> String {
    let s = text.trim();
    let end = s
        .find(|c| c == '.' || c == '!' || c == '?')
        .map(|i| i + 1)
        .unwrap_or(s.len().min(120));
    s[..end].trim().to_string()
}
