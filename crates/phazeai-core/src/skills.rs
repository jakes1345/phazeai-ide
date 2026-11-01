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
    /// The line of `path` that `body` starts on (1-based).
    pub body_line: usize,
    /// Original file path.
    pub path: PathBuf,
    /// Where it came from.
    pub source: SkillSource,
}

impl Skill {
    /// Returns the full system-prompt block to inject when this skill is active.
    pub fn as_system_prompt(&self) -> String {
        let block = self.instructions();
        format!("{}{}", block.heading, block.text)
    }

    /// This skill as a block of system-prompt instructions: what
    /// [`Self::as_system_prompt`] renders, in a form
    /// [`crate::context::system_prompt::fit_instructions`] can shorten to a
    /// context budget (a cut points at the skill file's own line numbers).
    pub fn instructions(&self) -> crate::context::system_prompt::Instructions {
        crate::context::system_prompt::Instructions::from_file(
            &self.path,
            self.body_line,
            format!("{}\n", self.body),
        )
        .with_heading(format!("# Active Expert Skill: {}\n\n", self.name))
    }

    /// Returns true if this skill's name or description loosely matches `query`.
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.name.to_lowercase().contains(&q) || self.description.to_lowercase().contains(&q)
    }
}

/// Discover all skills from both global and project locations.
///
/// Project skills take precedence: if a project skill has the same name as a
/// global skill, the global one is excluded.
pub fn discover_skills(workspace: Option<&Path>) -> Vec<Skill> {
    let global_dir = dirs::config_dir().map(|d| d.join("phazeai").join("skills"));

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
        // Skill bodies go into the system prompt; honour the privacy deny list.
        if crate::privacy::active().is_denied(&path) {
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

    // `body` is the end of `content`, so the trimmed body starts this far in.
    let body_start = content.len() - body.trim_start().len();
    let body_line = content[..body_start].matches('\n').count() + 1;

    Some(Skill {
        name,
        description,
        body: body.trim().to_string(),
        body_line,
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

/// Extract the first sentence (or up to 120 bytes, cut on a char boundary)
/// from body text.
fn first_sentence(text: &str) -> String {
    let s = text.trim();
    let head = match s.find(['.', '!', '?']) {
        Some(i) => &s[..=i], // ASCII terminator: i + 1 is a char boundary
        None => crate::text::truncate_bytes(s, 120),
    };
    head.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sentence_never_splits_a_multibyte_char() {
        // 1 ASCII byte + 50 × 3-byte '€' = 151 bytes; byte 120 falls inside a '€'.
        let body = format!("a{}", "€".repeat(50));
        assert!(!body.is_char_boundary(120));
        let got = first_sentence(&body);
        assert!(got.len() <= 120 && body.starts_with(&got), "{got:?}");
        assert_eq!(got, format!("a{}", "€".repeat(39)));

        // Same path through a skill file with no `description:` frontmatter.
        let skill =
            parse_skill_file(Path::new("x/unicode-skill.md"), &body, SkillSource::Project).unwrap();
        assert_eq!(skill.description, got);
        assert_eq!(skill.name, "unicode skill");

        // Sentence terminators still win, however long the text.
        let long = format!("{} wins. {}", "é".repeat(100), "x".repeat(300));
        assert_eq!(first_sentence(&long), format!("{} wins.", "é".repeat(100)));
        assert_eq!(first_sentence("short"), "short");
        assert_eq!(first_sentence(""), "");
    }

    #[test]
    fn a_shortened_skill_points_at_the_lines_of_its_own_file() {
        use crate::context::system_prompt::fit_instructions;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let _ws = crate::context::system_prompt::tests::workspace(&root);

        let path = root.join("reviewer.md");
        let mut content = "---\nname: reviewer\ndescription: Reviews changes\n---\n\n".to_string();
        for n in 1..=300 {
            content.push_str(&format!(
                "Step {n:03}: check the change against the spec.\n"
            ));
        }
        let skill = parse_skill_file(&path, &content, SkillSource::Project).unwrap();
        // Four lines of front matter and a blank line come first.
        assert_eq!(skill.body_line, 6);
        assert!(skill.body.starts_with("Step 001:"));
        // Whole, the block is exactly the old system-prompt text.
        assert_eq!(
            fit_instructions(&[skill.instructions()], usize::MAX),
            format!("# Active Expert Skill: reviewer\n\n{}\n", skill.body)
        );
        // Cut short, the read_file offset is the file line of the first step
        // left out.
        let out = fit_instructions(&[skill.instructions()], 2_000);
        assert!(out.len() <= 2_000, "{} bytes", out.len());
        let shown = out.matches("Step ").count();
        let next = format!("Step {:03}:", shown + 1);
        let next_line = content.lines().position(|l| l.starts_with(&next)).unwrap() + 1;
        assert_eq!(next_line, 6 + shown);
        assert!(
            out.ends_with(&format!("\"offset\": {next_line}}}.]")),
            "{}",
            &out[out.rfind("[... ").unwrap()..]
        );
    }
}
