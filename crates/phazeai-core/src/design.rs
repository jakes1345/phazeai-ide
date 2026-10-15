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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
    let path = design_system_path(root);
    if crate::privacy::policy_for(root).is_denied(&path) {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Write the design system, creating `.phazeai/` if needed.
pub fn save_design_system(root: &Path, markdown: &str) -> std::io::Result<PathBuf> {
    let path = design_system_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, normalized_markdown(markdown))?;
    Ok(path)
}

fn normalized_markdown(markdown: &str) -> String {
    let mut body = markdown.trim().to_string();
    body.push('\n');
    body
}

/// Where [`backup_then_write`] keeps the previous contents of `path`:
/// `design-system.md` → `design-system.previous.md`.
pub fn backup_path(path: &Path) -> PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => path.with_extension(format!("previous.{ext}")),
        None => path.with_extension("previous"),
    }
}

/// Replace `path` with `body`, first copying the existing file (checked on
/// disk now, not from a cached flag) to [`backup_path`]. When the backup
/// fails nothing is written, so the old contents are never lost.
pub fn backup_then_write(path: &Path, body: &str) -> Result<(), String> {
    if path.is_file() {
        let backup = backup_path(path);
        std::fs::copy(path, &backup).map_err(|e| {
            format!(
                "Couldn't back up {} to {}: {e}. Nothing was changed.",
                path.display(),
                backup.display()
            )
        })?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, body).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
}

/// Save a new design system, keeping the current one (if any) as
/// `design-system.previous.md`. Aborts without writing if the backup fails.
pub fn replace_design_system(root: &Path, markdown: &str) -> Result<PathBuf, String> {
    let path = design_system_path(root);
    backup_then_write(&path, &normalized_markdown(markdown))?;
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
            // The block may run past the document (see `closing_fence`); keep
            // just the document when it is complete.
            return Some(bare_document(&block).unwrap_or(block));
        }
    }
    bare_document(reply)
}

/// `<!doctype`/`<html` through the last `</html>` in `text`.
fn bare_document(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find("<!doctype").or_else(|| lower.find("<html"))?;
    let end = lower.rfind("</html>").map(|i| i + "</html>".len())?;
    (end > start).then(|| text[start..end].trim().to_string())
}

fn looks_like_html(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("<html") || lower.contains("<!doctype") || lower.contains("<body")
}

/// Longest sign-off a model may add after the fence that closes its reply.
const SIGN_OFF_LIMIT: usize = 240;

/// The body of the first fenced code block tagged with one of `langs`.
///
/// When the block wraps the whole reply (a `markdown` design system with
/// `css` examples fenced inside it, an `html` page with a fenced snippet in a
/// `<pre>`), its body runs to the fence that closes the reply, not to the
/// first inner one.
fn fenced_block(text: &str, langs: &[&str]) -> Option<String> {
    let mut rest = text;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let line_end = after.find('\n')?;
        let tag = after[..line_end].trim().to_ascii_lowercase();
        let body = &after[line_end + 1..];
        let first_close = body.find("```").unwrap_or(body.len());
        if langs.iter().any(|l| *l == tag) {
            return Some(body[..closing_fence(body, first_close)].trim().to_string());
        }
        rest = body.get(first_close + 3..).unwrap_or("");
    }
    None
}

/// Where the block whose body is `body` ends.
///
/// The first fence closes it unless a later bare fence (nothing else on its
/// line) is followed only by whitespace or a short sign-off: then the block
/// is the whole reply and the earlier fences are code blocks inside it.
fn closing_fence(body: &str, first_close: usize) -> usize {
    let is_bare = |at: usize| {
        body[at + 3..]
            .lines()
            .next()
            .is_none_or(|line| line.trim().is_empty())
    };
    let last_bare = body
        .match_indices("```")
        .map(|(at, _)| at)
        .filter(|&at| is_bare(at))
        .last();
    match last_bare {
        Some(at) if at > first_close && body[at + 3..].trim().chars().count() <= SIGN_OFF_LIMIT => {
            at
        }
        _ => first_close,
    }
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

fn lowercase_name_and_ext(path: &Path) -> (String, String) {
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
    (name, ext)
}

/// A design-token file by name: `tokens.json`, `design-tokens*.json` or
/// `*.tokens.json` (W3C Design Tokens style). The contents must still pass
/// [`looks_like_design_tokens`]: `src/auth/tokens.json` is usually OAuth
/// tokens, and those must never be sent to the model.
fn is_token_file_name(name: &str) -> bool {
    name == "tokens.json"
        || (name.starts_with("design-tokens") && name.ends_with(".json"))
        || name.ends_with(".tokens.json")
}

/// Whether `path` may describe the project's styling, judging by its name:
/// stylesheets, Tailwind configs, `theme.*` sources and design-token files.
/// JSON files also need their contents checked (see [`collect_style_sources`]).
fn is_style_source(path: &Path) -> bool {
    let (name, ext) = lowercase_name_and_ext(path);
    matches!(ext.as_str(), "css" | "scss" | "sass" | "less")
        || name.starts_with("tailwind.config.")
        || (name.starts_with("theme.")
            && matches!(ext.as_str(), "css" | "scss" | "json" | "js" | "ts" | "rs"))
        || is_token_file_name(&name)
}

/// Whether JSON text is a design-token file rather than something that merely
/// shares the word "tokens": it must carry token values (`"$value"` in the
/// W3C format, `"value"` in Style Dictionary's) and nothing that looks like
/// credentials.
pub fn looks_like_design_tokens(json: &str) -> bool {
    let lower = json.to_ascii_lowercase();
    (lower.contains("\"$value\"") || lower.contains("\"value\"")) && !looks_like_credentials(&lower)
}

/// Keys and values that mark a file as secrets, not styling.
fn looks_like_credentials(lower: &str) -> bool {
    [
        "access_token",
        "refresh_token",
        "secret",
        "password",
        "bearer ",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Whether a JSON candidate's contents may be sent: token files must look
/// like design tokens; any other JSON (e.g. `theme.json`) must at least not
/// look like credentials.
fn json_source_is_safe(path: &Path, text: &str) -> bool {
    let (name, ext) = lowercase_name_and_ext(path);
    if ext != "json" {
        return true;
    }
    if is_token_file_name(&name) {
        looks_like_design_tokens(text)
    } else {
        !looks_like_credentials(&text.to_ascii_lowercase())
    }
}

/// Stylesheets, Tailwind configs, theme and token files in the project,
/// skipping dependencies and build output, within a fixed size budget.
/// Paths are relative to `root`.
pub fn collect_style_sources(root: &Path) -> Vec<(PathBuf, String)> {
    // These are sent to the model: files the privacy policy denies are skipped.
    let policy = crate::privacy::policy_for(root);
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .filter_entry(move |e| {
            let name = e.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "node_modules" | "target" | "dist" | "build" | ".next" | "vendor"
            ) && (e.depth() == 0 || !policy.is_denied(&crate::privacy::absolutize(e.path())))
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
        let (name, _) = lowercase_name_and_ext(p);
        let rank = if name.starts_with("tailwind.config.") || is_token_file_name(&name) {
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
        // Checked on the whole file, before truncation, so a secret near the
        // end still disqualifies it.
        if !json_source_is_safe(&path, &text) {
            continue;
        }
        let limit = STYLE_FILE_LIMIT.min(STYLE_SOURCE_BUDGET - used);
        let text = crate::text::truncate_bytes(&text, limit).to_string();
        used += text.len();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        out.push((rel, text));
    }
    out
}

/// How long [`complete`] waits for the provider to send anything — the first
/// byte or the next chunk — before giving up on a stalled connection.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// How often [`complete`] checks `cancelled` while waiting on the provider.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// Stream one completion without tools, calling `on_text` with the text so far.
///
/// Returns [`CANCELLED`] within about 100 ms of `cancelled` being set, even
/// when the provider has gone quiet, and gives up after [`IDLE_TIMEOUT`]
/// without data.
pub async fn complete(
    client: &dyn LlmClient,
    system: &str,
    user: &str,
    cancelled: &AtomicBool,
    on_text: impl FnMut(&str),
) -> Result<String, String> {
    complete_with_idle_timeout(client, system, user, cancelled, IDLE_TIMEOUT, on_text).await
}

/// [`complete`] with a caller-chosen idle timeout.
pub async fn complete_with_idle_timeout(
    client: &dyn LlmClient,
    system: &str,
    user: &str,
    cancelled: &AtomicBool,
    idle_timeout: Duration,
    mut on_text: impl FnMut(&str),
) -> Result<String, String> {
    let messages = [Message::system(system), Message::user(user)];
    // Both waits below race against a short timer, so a Stop click or a dead
    // connection is noticed even when no stream event ever arrives.
    let mut waiting_since = Instant::now();
    let check = |waiting_since: Instant| -> Result<(), String> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(CANCELLED.to_string());
        }
        if waiting_since.elapsed() >= idle_timeout {
            return Err(idle_timeout_error(idle_timeout));
        }
        Ok(())
    };

    // Opening the stream includes connecting and waiting for response headers,
    // which is where a stalled provider usually hangs.
    let request = client.chat_stream(&messages, &[]);
    tokio::pin!(request);
    let mut stream = loop {
        tokio::select! {
            opened = &mut request => break opened.map_err(|e| e.to_string())?,
            _ = tokio::time::sleep(CANCEL_POLL) => check(waiting_since)?,
        }
    };

    let mut text = String::new();
    waiting_since = Instant::now();
    loop {
        tokio::select! {
            event = stream.next() => {
                waiting_since = Instant::now();
                match event {
                    Some(StreamEvent::TextDelta(delta)) => {
                        text.push_str(&delta);
                        on_text(&text);
                    }
                    Some(StreamEvent::Error(e)) => return Err(e),
                    Some(StreamEvent::Done) | None => break,
                    Some(_) => {}
                }
            }
            _ = tokio::time::sleep(CANCEL_POLL) => check(waiting_since)?,
        }
    }
    if text.trim().is_empty() {
        return Err("The model returned an empty reply.".to_string());
    }
    Ok(text)
}

fn idle_timeout_error(idle_timeout: Duration) -> String {
    let secs = idle_timeout.as_secs();
    let for_how_long = if secs == 60 {
        "1 minute".to_string()
    } else if secs > 60 && secs.is_multiple_of(60) {
        format!("{} minutes", secs / 60)
    } else {
        format!("{secs} seconds")
    };
    format!("No response from the provider for {for_how_long}. Check the connection and try again.")
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

    fn collected(root: &Path) -> Vec<String> {
        collect_style_sources(root)
            .into_iter()
            .map(|(p, _)| p.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn collect_style_sources_keeps_design_tokens_but_never_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for sub in ["src/auth", "config", "styles"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        // The word "tokens" in a name is not enough: these are secrets.
        std::fs::write(
            root.join("src/auth/tokens.json"),
            r#"{"access_token":"ya29.a0AfH6","refresh_token":"1//0gX","expires_in":3600}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("config/oauth_tokens.json"),
            r#"{"value":"ya29","token_type":"Bearer abc"}"#,
        )
        .unwrap();
        std::fs::write(root.join("api_tokens.txt"), "sk-live-123").unwrap();
        // A token file whose contents merely happen to be shaped like tokens
        // but hold a secret is still skipped.
        std::fs::write(
            root.join("styles/brand.tokens.json"),
            r#"{"api":{"value":"x","secret":"hunter2"}}"#,
        )
        .unwrap();
        // Real design tokens, in W3C and Style Dictionary shapes, are kept.
        std::fs::write(
            root.join("design-tokens.json"),
            r##"{"color":{"brand":{"$type":"color","$value":"#ff6600"}}}"##,
        )
        .unwrap();
        std::fs::write(
            root.join("styles/tokens.json"),
            r#"{"size":{"font":{"small":{"value":"12px"}}}}"#,
        )
        .unwrap();
        std::fs::write(root.join("styles/tokens.css"), ":root{--brand:#f60}").unwrap();
        std::fs::write(
            root.join("theme.json"),
            r##"{"palette":{"primary":"#123"}}"##,
        )
        .unwrap();

        let found = collected(root);
        assert_eq!(
            found,
            vec![
                "design-tokens.json",
                "styles/tokens.json",
                "theme.json",
                "styles/tokens.css",
            ]
        );
    }

    #[test]
    fn design_token_detection_needs_values_and_no_secrets() {
        assert!(looks_like_design_tokens(r##"{"a":{"$value":"#fff"}}"##));
        assert!(looks_like_design_tokens(r#"{"a":{"value":"4px"}}"#));
        assert!(!looks_like_design_tokens(r##"{"a":{"color":"#fff"}}"##));
        assert!(!looks_like_design_tokens(
            r#"{"a":{"value":"x"},"password":"p"}"#
        ));
        assert!(!looks_like_design_tokens(
            r#"{"a":{"value":"Bearer eyJhbGci"}}"#
        ));
        assert!(!looks_like_design_tokens(r#"{"access_token":"ya29"}"#));
    }

    #[test]
    fn extract_markdown_keeps_inner_code_fences() {
        let reply = "```markdown\n# DS\n\n## Colour\n```css\n:root{--brand:#f60}\n```\n\n## Type\nBody 14px.\n```\n";
        assert_eq!(
            extract_markdown(reply),
            "# DS\n\n## Colour\n```css\n:root{--brand:#f60}\n```\n\n## Type\nBody 14px."
        );
        // A short sign-off after the closing fence is fine.
        let signed = format!("{reply}\nLet me know if you'd like changes.");
        assert!(extract_markdown(&signed).ends_with("Body 14px."));
        // Without an outer fence nothing is cut.
        let plain = "# DS\n```css\na{}\n```\nmore";
        assert_eq!(extract_markdown(plain), plain);
    }

    #[test]
    fn extract_html_keeps_inner_code_fences() {
        let doc = "<!DOCTYPE html><html><body><pre>```js\nconsole.log(1)\n```</pre></body></html>";
        let reply = format!("```html\n{doc}\n```\nDone.");
        assert_eq!(extract_html(&reply).unwrap(), doc);
    }

    #[test]
    fn extract_html_ignores_blocks_after_the_document() {
        let reply = "```html\n<html><body>ok</body></html>\n```\nAlso:\n```css\na{}\n```\nBye";
        assert_eq!(extract_html(reply).unwrap(), "<html><body>ok</body></html>");
    }

    #[test]
    fn replace_design_system_backs_up_or_leaves_the_old_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // First save: nothing to back up.
        let path = replace_design_system(root, "# One").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# One\n");
        assert!(!backup_path(&path).exists());

        // The backup target can't be written (a directory sits there): the
        // save is refused and the current file is untouched.
        std::fs::create_dir(backup_path(&path)).unwrap();
        let err = replace_design_system(root, "# Two").unwrap_err();
        assert!(err.contains("back up"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# One\n");

        std::fs::remove_dir(backup_path(&path)).unwrap();
        replace_design_system(root, "# Two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Two\n");
        assert_eq!(
            std::fs::read_to_string(backup_path(&path)).unwrap(),
            "# One\n"
        );
        assert!(backup_path(&path).ends_with("design-system.previous.md"));
    }

    #[test]
    fn idle_timeout_message_names_the_wait() {
        assert_eq!(
            idle_timeout_error(IDLE_TIMEOUT),
            "No response from the provider for 2 minutes. Check the connection and try again."
        );
        assert!(idle_timeout_error(Duration::from_secs(30)).contains("30 seconds"));
    }

    /// A provider that accepts the connection and then sends `head` (possibly
    /// nothing) and never anything else. The socket stays open until the
    /// returned guard is dropped.
    async fn stalled_server(head: &'static str) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 64 * 1024];
            let _ = sock.read(&mut buf).await;
            if !head.is_empty() {
                sock.write_all(head.as_bytes()).await.unwrap();
            }
            // Hold the connection open, silently, until aborted.
            std::future::pending::<()>().await;
        });
        (format!("http://{addr}"), task)
    }

    #[tokio::test]
    async fn stop_returns_promptly_when_the_provider_never_answers() {
        let (base, server) = stalled_server("").await;
        let client = crate::llm::OpenAIClient::new("k").with_base_url(base);
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            flag.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            complete(&client, "sys", "user", &cancelled, |_| {}),
        )
        .await
        .expect("Stop must not hang");
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "took {:?}",
            started.elapsed()
        );
        server.abort();
    }

    #[tokio::test]
    async fn idle_timeout_gives_up_on_a_silent_provider() {
        let (base, server) = stalled_server("").await;
        let client = crate::llm::OpenAIClient::new("k").with_base_url(base);
        let cancelled = AtomicBool::new(false);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            complete_with_idle_timeout(
                &client,
                "sys",
                "user",
                &cancelled,
                Duration::from_millis(300),
                |_| {},
            ),
        )
        .await
        .expect("the idle timeout must fire");
        let err = result.unwrap_err();
        assert!(err.starts_with("No response from the provider"), "{err}");
        server.abort();
    }

    #[tokio::test]
    async fn idle_timeout_gives_up_on_a_stalled_stream() {
        // Headers and one delta arrive, then the stream goes quiet.
        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"<html>\"}}]}\n\n";
        let (base, server) = stalled_server(head).await;
        let client = crate::llm::OpenAIClient::new("k").with_base_url(base);
        let cancelled = AtomicBool::new(false);
        let mut seen = String::new();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            complete_with_idle_timeout(
                &client,
                "sys",
                "user",
                &cancelled,
                Duration::from_millis(300),
                |text| seen = text.to_string(),
            ),
        )
        .await
        .expect("the idle timeout must fire");
        assert!(result.unwrap_err().starts_with("No response"));
        assert_eq!(seen, "<html>");
        server.abort();
    }
}
