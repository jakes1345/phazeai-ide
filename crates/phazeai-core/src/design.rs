//! AI design: turn a written description into a working HTML/CSS prototype
//! that follows the project's own design system.
//!
//! Everything lives inside the project so it can be committed and shared:
//!
//! - `.phazeai/design-system.md` — the design system, in plain Markdown
//!   (colours, type, spacing, components, voice). People edit it by hand; the
//!   IDE can also draft it from the project's existing styles.
//! - `.phazeai/designs/*.html` — generated prototypes. Each one is a single
//!   self-contained file that opens in any browser.
//!
//! The model never touches the disk here. It returns text, and the caller
//! writes exactly one file, so a design request can't modify anything else.

use std::path::{Path, PathBuf};

use futures::StreamExt;

use crate::llm::{LlmClient, Message, StreamEvent};

/// Project-relative location of the design system.
pub const DESIGN_SYSTEM_FILE: &str = ".phazeai/design-system.md";
/// Project-relative folder holding generated designs.
pub const DESIGNS_DIR: &str = ".phazeai/designs";

/// Error text returned by [`complete`] when the request was cancelled.
pub const CANCELLED: &str = "Cancelled";

/// Largest amount of existing styling sent when drafting a design system.
const STYLE_SOURCE_BUDGET: usize = 48 * 1024;
/// Largest single file sent when drafting a design system.
const STYLE_FILE_LIMIT: usize = 12 * 1024;

pub fn design_system_path(root: &Path) -> PathBuf {
    root.join(DESIGN_SYSTEM_FILE)
}

pub fn designs_dir(root: &Path) -> PathBuf {
    root.join(DESIGNS_DIR)
}

/// The project's design system, if it has one with any content.
pub fn load_design_system(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(design_system_path(root)).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Write the design system, creating `.phazeai/` if needed.
pub fn save_design_system(root: &Path, markdown: &str) -> std::io::Result<PathBuf> {
    let path = design_system_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut body = markdown.trim().to_string();
    body.push('\n');
    std::fs::write(&path, body)?;
    Ok(path)
}

/// A generated design on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignEntry {
    /// File name without the `.html` extension.
    pub name: String,
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
}

/// All designs in the project, newest first.
pub fn list_designs(root: &Path) -> Vec<DesignEntry> {
    let Ok(read) = std::fs::read_dir(designs_dir(root)) else {
        return Vec::new();
    };
    let mut entries: Vec<DesignEntry> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("html") {
                return None;
            }
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let modified = e.metadata().ok()?.modified().ok()?;
            Some(DesignEntry {
                name,
                path,
                modified,
            })
        })
        .collect();
    entries.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
    entries
}

/// A short, file-name-safe slug from a free-text request.
pub fn slugify(request: &str) -> String {
    let slug = request
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(6)
        .map(|w| w.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "design".to_string()
    } else {
        slug
    }
}

/// A path in the designs folder for `request` that doesn't exist yet.
pub fn new_design_path(root: &Path, request: &str) -> PathBuf {
    let dir = designs_dir(root);
    let base = slugify(request);
    let first = dir.join(format!("{base}.html"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{base}-{n}.html")))
        .find(|p| !p.exists())
        .expect("an unused file name exists")
}

/// System prompt for producing a design.
pub const DESIGN_SYSTEM_PROMPT: &str = "\
You are a senior product designer who also writes production-quality front-end code. \
You turn a request into ONE complete, self-contained HTML file: semantic HTML, a single \
<style> block, and a small <script> only when the design needs interaction. No external \
assets, fonts, frameworks or CDNs — it must render correctly offline. Use realistic content \
(not lorem ipsum), a responsive layout that works from 360px wide upwards, visible focus \
states and sufficient colour contrast. Define colours, type sizes, spacing and radii as CSS \
custom properties on :root and use them throughout. When a design system is provided, follow \
it exactly: its tokens, components and tone take priority over your own taste. \
Reply with the HTML file only, inside a single ```html code block.";

/// User prompt for a new design or a revision of an existing one.
pub fn design_prompt(request: &str, design_system: Option<&str>, current: Option<&str>) -> String {
    let mut prompt = String::new();
    match design_system {
        Some(ds) => {
            prompt.push_str("Project design system (follow it):\n<design_system>\n");
            prompt.push_str(ds);
            prompt.push_str("\n</design_system>\n\n");
        }
        None => prompt.push_str(
            "This project has no design system yet. Choose a clean, coherent one and \
             express it as CSS custom properties.\n\n",
        ),
    }
    match current {
        Some(html) => {
            prompt.push_str("Here is the current design:\n<current_design>\n");
            prompt.push_str(html);
            prompt.push_str(
                "\n</current_design>\n\nRevise it as follows, keeping everything else the same:\n",
            );
        }
        None => prompt.push_str("Design the following:\n"),
    }
    prompt.push_str(request.trim());
    prompt
}

/// Pull the HTML document out of a model reply.
///
/// Accepts a fenced ```html block, or a bare document from `<!doctype` / `<html`
/// to `</html>`. Returns `None` when the reply contains no HTML document.
pub fn extract_html(reply: &str) -> Option<String> {
    if let Some(block) = fenced_block(reply, &["html", "htm"]) {
        if looks_like_html(&block) {
            return Some(block);
        }
    }
    let lower = reply.to_ascii_lowercase();
    let start = lower.find("<!doctype").or_else(|| lower.find("<html"))?;
    let end = lower.rfind("</html>").map(|i| i + "</html>".len())?;
    (end > start).then(|| reply[start..end].trim().to_string())
}

fn looks_like_html(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("<html") || lower.contains("<!doctype") || lower.contains("<body")
}

/// The body of the first fenced code block tagged with one of `langs`.
fn fenced_block(text: &str, langs: &[&str]) -> Option<String> {
    let mut rest = text;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let line_end = after.find('\n')?;
        let tag = after[..line_end].trim().to_ascii_lowercase();
        let body = &after[line_end + 1..];
        let close = body.find("```").unwrap_or(body.len());
        if langs.iter().any(|l| *l == tag) {
            return Some(body[..close].trim().to_string());
        }
        rest = body.get(close + 3..).unwrap_or("");
    }
    None
}

/// System prompt for drafting a design system from existing code.
pub const EXTRACT_SYSTEM_PROMPT: &str = "\
You are a design-systems lead. From the style sources you are given, write the project's \
design system as a concise Markdown document with these sections: Overview (product feel in \
two sentences), Colour (named tokens with hex values and where each is used), Typography \
(families, sizes, weights, line heights), Spacing & layout (scale, grid, breakpoints), Shape \
& elevation (radii, borders, shadows), Components (buttons, inputs, cards, navigation — \
anatomy and states), and Voice (how copy should sound). Only state what the sources support; \
where they are silent, propose a value and mark it '(proposed)'. Reply with the Markdown only.";

/// User prompt for drafting a design system from collected style sources.
pub fn extract_prompt(sources: &[(PathBuf, String)]) -> String {
    if sources.is_empty() {
        return "This project has no stylesheets or theme files yet. Propose a clean, \
                accessible starter design system, marking every value '(proposed)'."
            .to_string();
    }
    let mut prompt = String::from("Style sources from the project:\n\n");
    for (path, text) in sources {
        prompt.push_str(&format!("--- {} ---\n{}\n\n", path.display(), text));
    }
    prompt
}

/// Markdown from a model reply, without an outer ```markdown fence if present.
pub fn extract_markdown(reply: &str) -> String {
    fenced_block(reply, &["markdown", "md"]).unwrap_or_else(|| reply.trim().to_string())
}

fn is_style_source(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "css" | "scss" | "sass" | "less")
        || name.starts_with("tailwind.config.")
        || name.contains("tokens")
        || name.starts_with("theme.")
        || name == "theme.rs"
}

/// Stylesheets, Tailwind configs, theme and token files in the project,
/// skipping dependencies and build output, within a fixed size budget.
/// Paths are relative to `root`.
pub fn collect_style_sources(root: &Path) -> Vec<(PathBuf, String)> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "node_modules" | "target" | "dist" | "build" | ".next" | "vendor"
            )
        })
        .build();
    let mut candidates: Vec<PathBuf> = walker
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(|e| e.into_path())
        .filter(|p| is_style_source(p) && !p.to_string_lossy().contains(".min."))
        .collect();
    // Config and token files describe the system most directly; read them first.
    candidates.sort_by_key(|p| {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase());
        let name = name.unwrap_or_default();
        let rank = if name.starts_with("tailwind.config.") || name.contains("tokens") {
            0
        } else if name.starts_with("theme.") {
            1
        } else {
            2
        };
        (rank, p.components().count(), p.clone())
    });

    let mut used = 0;
    let mut out = Vec::new();
    for path in candidates {
        if used >= STYLE_SOURCE_BUDGET {
            break;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let limit = STYLE_FILE_LIMIT.min(STYLE_SOURCE_BUDGET - used);
        let text = crate::text::truncate_bytes(&text, limit).to_string();
        used += text.len();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        out.push((rel, text));
    }
    out
}

/// Stream one completion without tools, calling `on_text` with the text so far.
/// Stops with an error as soon as `cancelled` is set.
pub async fn complete(
    client: &dyn LlmClient,
    system: &str,
    user: &str,
    cancelled: &std::sync::atomic::AtomicBool,
    mut on_text: impl FnMut(&str),
) -> Result<String, String> {
    let messages = [Message::system(system), Message::user(user)];
    let mut stream = client
        .chat_stream(&messages, &[])
        .await
        .map_err(|e| e.to_string())?;
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(CANCELLED.to_string());
        }
        match event {
            StreamEvent::TextDelta(delta) => {
                text.push_str(&delta);
                on_text(&text);
            }
            StreamEvent::Error(e) => return Err(e),
            StreamEvent::Done => break,
            _ => {}
        }
    }
    if text.trim().is_empty() {
        return Err("The model returned an empty reply.".to_string());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_keeps_first_six_words() {
        assert_eq!(
            slugify("A pricing page, with 3 tiers & a FAQ section below"),
            "a-pricing-page-with-3-tiers"
        );
        assert_eq!(slugify("¿¡!!"), "design");
    }

    #[test]
    fn new_design_path_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let first = new_design_path(dir.path(), "Login screen");
        assert!(first.ends_with(".phazeai/designs/login-screen.html"));
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::write(&first, "x").unwrap();
        let second = new_design_path(dir.path(), "Login screen");
        assert!(second.ends_with("login-screen-2.html"));
    }

    #[test]
    fn extract_html_from_fence_or_bare_document() {
        let fenced =
            "Here you go:\n```html\n<!DOCTYPE html><html><body>hi</body></html>\n```\nEnjoy";
        assert_eq!(
            extract_html(fenced).unwrap(),
            "<!DOCTYPE html><html><body>hi</body></html>"
        );
        let bare = "Sure. <!doctype html><html><body>x</body></html> done";
        assert_eq!(
            extract_html(bare).unwrap(),
            "<!doctype html><html><body>x</body></html>"
        );
        assert!(extract_html("I can't do that.").is_none());
    }

    #[test]
    fn extract_html_skips_non_html_fences() {
        let reply = "```css\nbody{}\n```\n```html\n<html><body>ok</body></html>\n```";
        assert_eq!(extract_html(reply).unwrap(), "<html><body>ok</body></html>");
    }

    #[test]
    fn extract_markdown_strips_outer_fence() {
        assert_eq!(extract_markdown("```markdown\n# DS\nx\n```"), "# DS\nx");
        assert_eq!(extract_markdown("  # DS\n"), "# DS");
    }

    #[test]
    fn design_prompt_includes_system_and_current() {
        let p = design_prompt(
            "make the button red",
            Some("Primary: #123456"),
            Some("<html/>"),
        );
        assert!(p.contains("Primary: #123456"));
        assert!(p.contains("<current_design>\n<html/>"));
        assert!(p.ends_with("make the button red"));
        let p = design_prompt("a login page", None, None);
        assert!(p.contains("no design system yet"));
    }

    #[test]
    fn design_system_round_trip_and_listing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_design_system(dir.path()).is_none());
        save_design_system(dir.path(), "\n# Brand\n").unwrap();
        assert_eq!(load_design_system(dir.path()).unwrap(), "# Brand");

        assert!(list_designs(dir.path()).is_empty());
        let d = designs_dir(dir.path());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("home.html"), "<html></html>").unwrap();
        std::fs::write(d.join("notes.txt"), "skip").unwrap();
        let list = list_designs(dir.path());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "home");
    }

    #[test]
    fn collect_style_sources_finds_styles_and_skips_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/lib")).unwrap();
        std::fs::write(root.join("src/app.css"), ":root{--brand:#f60}").unwrap();
        std::fs::write(root.join("tailwind.config.js"), "module.exports={}").unwrap();
        std::fs::write(root.join("node_modules/lib/x.css"), "a{}").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main(){}").unwrap();

        let found: Vec<String> = collect_style_sources(root)
            .into_iter()
            .map(|(p, _)| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(found, vec!["tailwind.config.js", "src/app.css"]);
    }
}
