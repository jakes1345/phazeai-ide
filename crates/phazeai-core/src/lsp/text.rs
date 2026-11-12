//! File URIs and text edits as the language server protocol defines them.
//!
//! LSP positions count lines and UTF-16 code units, not bytes, and a file URI
//! percent-encodes its path. Getting either wrong edits the wrong text (or
//! panics on a character boundary) as soon as a file holds non-ASCII text or
//! its path holds a space.

use std::path::{Path, PathBuf};

use lsp_types::{Position, Range, TextEdit, Uri};

/// The `file://` URI for `path` (made absolute against the current directory),
/// percent-encoding everything in each segment but unreserved characters.
pub fn path_to_uri(path: &Path) -> Result<Uri, String> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let text = abs.to_string_lossy().replace('\\', "/");
    let mut encoded = String::from("file://");
    if !text.starts_with('/') {
        // A Windows path (C:/...) still needs the empty authority's slash.
        encoded.push('/');
    }
    let mut segments = text.split('/').peekable();
    while let Some(segment) = segments.next() {
        // Keep a Windows drive letter's colon readable (`C:`).
        let is_drive = segment.len() == 2 && segment.ends_with(':');
        if is_drive {
            encoded.push_str(segment);
        } else {
            encoded.push_str(&urlencoding::encode(segment));
        }
        if segments.peek().is_some() {
            encoded.push('/');
        }
    }
    encoded
        .parse()
        .map_err(|e| format!("Invalid URI {encoded}: {e:?}"))
}

/// The local path a `file://` URI names, with its percent-encoding decoded.
/// `None` for any other scheme or a path that does not decode to UTF-8.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let text = uri.as_str();
    let rest = text.strip_prefix("file://")?;
    // Drop an authority (`file://localhost/...`); the path starts at its slash.
    let path = &rest[rest.find('/')?..];
    let decoded = urlencoding::decode(path).ok()?.into_owned();
    // `/C:/dir` on Windows is `C:/dir`.
    let bytes = decoded.as_bytes();
    if cfg!(windows) && bytes.len() >= 3 && bytes[2] == b':' && bytes[1].is_ascii_alphabetic() {
        return Some(PathBuf::from(&decoded[1..]));
    }
    Some(PathBuf::from(decoded))
}

/// Byte offsets where each line of `text` starts. Lines end at `\n`, `\r\n`
/// or a lone `\r`, as LSP counts them.
fn line_starts(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = vec![0];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => starts.push(i + 1),
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                i += 1;
                starts.push(i + 1);
            }
            b'\r' => starts.push(i + 1),
            _ => {}
        }
        i += 1;
    }
    starts
}

/// The byte offset of `pos` in `text`. A character past the end of its line
/// means the line's end (before its line break), and a line past the last
/// means the end of the text, as the protocol asks. A position inside a
/// character made of two UTF-16 units is an error.
fn offset_of(text: &str, starts: &[usize], pos: Position) -> Result<usize, String> {
    let Some(&start) = starts.get(pos.line as usize) else {
        return Ok(text.len());
    };
    let end = starts
        .get(pos.line as usize + 1)
        .map(|&next| {
            let line = &text[start..next];
            next - (line.len() - line.trim_end_matches(['\n', '\r']).len())
        })
        .unwrap_or(text.len());
    let mut units = 0u32;
    for (i, ch) in text[start..end].char_indices() {
        if units == pos.character {
            return Ok(start + i);
        }
        units += ch.len_utf16() as u32;
        if units > pos.character {
            return Err(format!(
                "position {}:{} is inside a character",
                pos.line, pos.character
            ));
        }
    }
    Ok(end)
}

/// The byte range `range` covers in `text`.
pub fn byte_range(text: &str, range: Range) -> Result<std::ops::Range<usize>, String> {
    let starts = line_starts(text);
    let start = offset_of(text, &starts, range.start)?;
    let end = offset_of(text, &starts, range.end)?;
    if end < start {
        return Err(format!("edit range ends before it starts: {range:?}"));
    }
    Ok(start..end)
}

/// `text` with `edits` applied. Every range refers to `text` as given, as the
/// protocol requires. Line breaks and the final newline are left as they are.
/// Overlapping edits or a position inside a character are an error, and then
/// nothing is applied.
pub fn apply_text_edits(text: &str, edits: &[TextEdit]) -> Result<String, String> {
    let starts = line_starts(text);
    let mut resolved = edits
        .iter()
        .enumerate()
        .map(|(order, edit)| {
            let start = offset_of(text, &starts, edit.range.start)?;
            let end = offset_of(text, &starts, edit.range.end)?;
            if end < start {
                return Err(format!(
                    "edit range ends before it starts: {:?}",
                    edit.range
                ));
            }
            Ok((start, end, order, edit.new_text.as_str()))
        })
        .collect::<Result<Vec<_>, String>>()?;
    // Inserts at the same spot keep the order they were given in.
    resolved.sort_by_key(|&(start, end, order, _)| (start, end, order));
    for pair in resolved.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err("the language server sent overlapping edits".to_string());
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, _, new_text) in resolved {
        out.push_str(&text[at..start]);
        out.push_str(new_text);
        at = end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(sl: u32, sc: u32, el: u32, ec: u32, new_text: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: Position::new(sl, sc),
                end: Position::new(el, ec),
            },
            new_text: new_text.to_string(),
        }
    }

    #[test]
    fn columns_count_utf16_units_not_bytes() {
        // "é" is 2 bytes and 1 unit; "😀" is 4 bytes and 2 units.
        let text = "let é = 1; let 😀x = 2; x\n";
        let edits = [edit(0, 17, 0, 18, "renamed"), edit(0, 24, 0, 25, "renamed")];
        assert_eq!(
            apply_text_edits(text, &edits).unwrap(),
            "let é = 1; let 😀renamed = 2; renamed\n"
        );
    }

    #[test]
    fn line_breaks_and_the_final_newline_are_kept() {
        let text = "fn old() {}\r\nfn main() { old(); }\r\n";
        let edits = [edit(0, 3, 0, 6, "new"), edit(1, 12, 1, 15, "new")];
        assert_eq!(
            apply_text_edits(text, &edits).unwrap(),
            "fn new() {}\r\nfn main() { new(); }\r\n"
        );
        assert_eq!(
            apply_text_edits("a\nb", &[edit(1, 0, 1, 1, "c")]).unwrap(),
            "a\nc",
            "no newline is added where there was none"
        );
    }

    #[test]
    fn edits_may_span_lines_and_come_in_any_order() {
        let text = "one\ntwo\r\nthree\n";
        let edits = [edit(2, 0, 2, 5, "3"), edit(0, 1, 1, 2, "-")];
        assert_eq!(apply_text_edits(text, &edits).unwrap(), "o-o\r\n3\n");
    }

    #[test]
    fn positions_past_a_line_or_the_text_clamp_to_its_end() {
        let text = "ab\r\ncd";
        assert_eq!(
            apply_text_edits(text, &[edit(0, 99, 0, 99, "!")]).unwrap(),
            "ab!\r\ncd"
        );
        assert_eq!(
            apply_text_edits(text, &[edit(5, 0, 5, 0, "\nend")]).unwrap(),
            "ab\r\ncd\nend"
        );
    }

    #[test]
    fn a_position_inside_a_character_or_overlapping_edits_change_nothing() {
        let text = "😀\n";
        assert!(apply_text_edits(text, &[edit(0, 1, 0, 2, "x")]).is_err());
        let text = "abcdef";
        let edits = [edit(0, 0, 0, 3, "x"), edit(0, 2, 0, 4, "y")];
        assert!(apply_text_edits(text, &edits).is_err());
    }

    #[test]
    fn inserts_at_one_spot_keep_their_order() {
        let edits = [edit(0, 1, 0, 1, "1"), edit(0, 1, 0, 1, "2")];
        assert_eq!(apply_text_edits("ab", &edits).unwrap(), "a12b");
    }

    #[test]
    fn paths_with_spaces_and_non_ascii_round_trip_through_uris() {
        let path = Path::new("/home/me/My Projects/naïve #1/src/main.rs");
        let uri = path_to_uri(path).unwrap();
        assert_eq!(
            uri.as_str(),
            "file:///home/me/My%20Projects/na%C3%AFve%20%231/src/main.rs"
        );
        assert_eq!(uri_to_path(&uri).unwrap(), path);
        let plain: Uri = "file:///tmp/a.rs".parse().unwrap();
        assert_eq!(uri_to_path(&plain).unwrap(), Path::new("/tmp/a.rs"));
        let other: Uri = "https://example.com/a.rs".parse().unwrap();
        assert_eq!(uri_to_path(&other), None);
    }
}
