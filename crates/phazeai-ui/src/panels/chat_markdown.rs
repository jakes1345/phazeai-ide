//! How the chat shows a model's reply and a tool's result.
//!
//! Replies are Markdown. The chat draws the parts that matter for reading
//! code answers (fenced code, headings, lists, paragraphs) as blocks of their
//! own, and leaves inline marks readable as plain text: `**` is dropped,
//! inline `code` keeps its backticks. A reply is drawn while it streams in,
//! so a fence that hasn't closed yet is still code.

/// One block of a reply, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A fenced code block: its language tag (may be empty) and its lines.
    Code { lang: String, text: String },
    /// A `#` heading, without the marks.
    Heading(String),
    /// A paragraph or a list, lines joined with newlines, list marks as `•`.
    Text(String),
}

/// Split a Markdown reply into blocks.
pub fn blocks(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let flush = |para: &mut Vec<String>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Text(para.join("\n")));
            para.clear();
        }
    };
    let mut lines = md.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            flush(&mut para, &mut out);
            let lang = rest.trim().to_string();
            let mut code = Vec::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
                code.push(l);
            }
            out.push(Block::Code {
                lang,
                text: code.join("\n"),
            });
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut para, &mut out);
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut para, &mut out);
            out.push(Block::Heading(inline(trimmed[hashes..].trim())));
            continue;
        }
        para.push(list_item(line));
    }
    flush(&mut para, &mut out);
    out
}

/// A list line with `•` for its mark (keeping its indent), or the line.
fn list_item(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    for mark in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(mark) {
            return format!("{}• {}", " ".repeat(indent), inline(rest));
        }
    }
    inline(line)
}

/// Inline marks made plain: bold and italic stars dropped.
fn inline(text: &str) -> String {
    text.replace("**", "").replace("__", "")
}

/// Longest first line a tool row shows.
const TOOL_LINE_MAX: usize = 120;

/// What a tool row shows of a result: its first line with text, cut to a
/// readable length, and how many lines follow. A `read_file` result is the
/// whole file; the chat showed all of it.
pub fn tool_summary(summary: &str) -> String {
    let lines: Vec<&str> = summary.lines().collect();
    let Some(first) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return String::new();
    };
    let head = lines[first].trim();
    let head = if head.chars().count() > TOOL_LINE_MAX {
        let cut: String = head.chars().take(TOOL_LINE_MAX).collect();
        format!("{cut}…")
    } else {
        head.to_string()
    };
    let more = lines[first + 1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .count();
    match more {
        0 => head,
        1 => format!("{head} (+1 line)"),
        n => format!("{head} (+{n} lines)"),
    }
}

#[cfg(test)]
mod tests {
    use super::{blocks, tool_summary, Block};

    #[test]
    fn a_reply_splits_into_text_headings_lists_and_code() {
        let md = "## The bug\n\nLine 12 **adds** instead of multiplying:\n\n\
                  ```rust\ntotal += a + b;\n```\n\n- one\n  * two\nDone.";
        assert_eq!(
            blocks(md),
            vec![
                Block::Heading("The bug".into()),
                Block::Text("Line 12 adds instead of multiplying:".into()),
                Block::Code {
                    lang: "rust".into(),
                    text: "total += a + b;".into()
                },
                Block::Text("• one\n  • two\nDone.".into()),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_while_streaming_is_still_code() {
        assert_eq!(
            blocks("Fix:\n```\nlet x = 1;"),
            vec![
                Block::Text("Fix:".into()),
                Block::Code {
                    lang: String::new(),
                    text: "let x = 1;".into()
                },
            ]
        );
        // Not a heading: `#` without a space (a Rust attribute, a hashtag).
        assert_eq!(blocks("#[test]"), vec![Block::Text("#[test]".into())]);
    }

    #[test]
    fn a_tool_row_shows_the_first_line_and_a_count() {
        assert_eq!(tool_summary("Edited src/lib.rs"), "Edited src/lib.rs");
        let file = "1\t/// A shop\n2\tpub struct Item {\n3\t}\n";
        assert_eq!(tool_summary(file), "1\t/// A shop (+2 lines)");
        assert_eq!(tool_summary("\n\nonly\n\n"), "only");
        assert_eq!(tool_summary("a\nb"), "a (+1 line)");
        let long = "x".repeat(200);
        assert_eq!(tool_summary(&long).chars().count(), 121);
        assert_eq!(tool_summary(""), "");
    }
}
