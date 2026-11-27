//! A web page as the text a person reads on it.
//!
//! A fetched page is mostly markup the model never needs: scripts, styles,
//! inline SVG icons, navigation menus, tracking attributes. A GitHub file
//! page is about 280 KB of HTML around a few KB of code. Sent raw, the useful
//! part is a small fraction of what the model pays for, and a byte cap on the
//! raw HTML can stop inside `<head>` before any content at all.
//!
//! [`to_text`] reads the HTML with a small tolerant scanner (real pages are
//! rarely valid HTML) and writes Markdown-style text: headings as `#`, list
//! items as `-` or `1.`, preformatted blocks fenced with three backticks,
//! inline code in backticks, table cells separated by ` | `, and links as
//! `[text](url)`. A link to another page of the same site is written from
//! its path (`/docs/intro`), one elsewhere in full, and an address already
//! linked once is not written again: on a documentation page the same type
//! links recur dozens of times, and a full address costs more tokens than the
//! word it is attached to. It leaves out what a
//! reader doesn't see or need: `script`, `style`, `noscript`, `template`,
//! `svg`, `canvas`, `iframe`, `head` (apart from the title), `nav`, `footer`,
//! forms' controls, images, and anything marked `hidden` or
//! `aria-hidden="true"`. Claude Code's WebFetch hands its model Markdown in
//! the same way.

pub use reqwest::Url;
use std::collections::HashSet;

/// What [`to_text`] found on a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The text of `<title>`, empty when there is none.
    pub title: String,
    /// The readable text.
    pub text: String,
    /// Whether a selector was given and matched (the text is then only what
    /// it matched); `false` with a selector means it matched nothing and the
    /// text is the whole page.
    pub selected: bool,
}

/// How [`to_text`] writes the page.
#[derive(Debug, Clone, Default)]
pub struct Options<'a> {
    /// Write links as `[text](url)`; otherwise only their text.
    pub links: bool,
    /// The page's own address: relative links are resolved against it, and
    /// links to the same site are written from their path.
    pub base: Option<&'a Url>,
    /// Keep only elements matching this simple selector: `tag`, `.class`,
    /// `#id`, `tag.class` or `tag#id` (the first part of a list or a
    /// descendant selector is used).
    pub selector: Option<&'a str>,
}

/// `html` as readable text (see the module docs).
pub fn to_text(html: &str, options: &Options) -> Page {
    let selector = options.selector.and_then(Selector::parse);
    if let Some(selector) = &selector {
        let page = convert(html, options, Some(selector));
        if page.selected {
            return page;
        }
    }
    convert(html, options, None)
}

/// Whether a response with this `Content-Type` is a web page.
pub fn is_html(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime == "text/html" || mime == "application/xhtml+xml"
}

/// Elements whose content is left out, with everything inside them.
const SKIPPED: &[&str] = &[
    "head", "script", "style", "noscript", "template", "svg", "canvas", "iframe", "object",
    "embed", "nav", "footer", "select", "button", "input", "textarea", "dialog", "math",
];

/// Elements whose content is raw text up to their end tag (no markup inside).
const RAW_TEXT: &[&str] = &[
    "script", "style", "textarea", "title", "noscript", "xmp", "iframe", "noembed", "noframes",
];

/// Elements with no end tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements that start on a new line, with a blank line around them.
const BLOCK: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "main",
    "header",
    "aside",
    "blockquote",
    "figure",
    "figcaption",
    "address",
    "details",
    "summary",
    "table",
    "ul",
    "ol",
    "dl",
    "form",
    "fieldset",
    "hr",
];

/// Starting one of these closes an open `<p>` (HTML's implied end tag).
const CLOSES_P: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "main",
    "header",
    "aside",
    "blockquote",
    "figure",
    "address",
    "details",
    "table",
    "ul",
    "ol",
    "dl",
    "form",
    "fieldset",
    "hr",
    "pre",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Selector {
    tag: Option<String>,
    id: Option<String>,
    class: Option<String>,
}

impl Selector {
    fn parse(text: &str) -> Option<Self> {
        let first = text.split(',').next()?.split_whitespace().last()?;
        let (rest, id) = match first.split_once('#') {
            Some((before, id)) => (before, Some(id)),
            None => (first, None),
        };
        let (tag, class, id) = match (rest.split_once('.'), id) {
            (Some((tag, class)), id) => (tag, Some(class), id),
            (None, Some(id)) => match id.split_once('.') {
                Some((id, class)) => (rest, Some(class), Some(id)),
                None => (rest, None, Some(id)),
            },
            (None, None) => (rest, None, None),
        };
        let some = |s: Option<&str>| s.filter(|s| !s.is_empty()).map(str::to_string);
        let selector = Selector {
            tag: some(Some(tag)).map(|t| t.to_ascii_lowercase()),
            id: some(id),
            class: some(class),
        };
        (selector.tag.is_some() || selector.id.is_some() || selector.class.is_some())
            .then_some(selector)
    }

    fn matches(&self, tag: &Tag) -> bool {
        self.tag.as_deref().is_none_or(|t| t == tag.name)
            && self
                .id
                .as_deref()
                .is_none_or(|id| tag.attr("id") == Some(id))
            && self.class.as_deref().is_none_or(|class| {
                tag.attr("class")
                    .is_some_and(|c| c.split_ascii_whitespace().any(|c| c == class))
            })
    }
}

/// A start tag as the scanner read it.
#[derive(Debug)]
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
}

impl Tag {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn hidden(&self) -> bool {
        self.attr("hidden").is_some()
            || self
                .attr("aria-hidden")
                .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }
}

/// An element still open while the page is read.
struct Open {
    name: String,
    /// Left out with everything inside it.
    skipped: bool,
    /// Matched the selector.
    selected: bool,
    /// For a link: where `[` was written and the address to close it with.
    link: Option<(usize, String)>,
    /// For `ol`: the next item number. For `tr`: cells written so far.
    count: usize,
    /// For a heading or a list item: where its marker (`## `, `- `) was
    /// written, so an empty one can be taken back.
    mark: Option<usize>,
}

struct Writer {
    out: String,
    /// Open `pre` elements.
    pre: usize,
    /// Open `ul`/`ol` elements, for indenting nested items.
    lists: usize,
    /// Markup that text follows directly was just written (`[` or an
    /// opening backtick): no space before the next word.
    no_space: bool,
    /// Open table cells and headings: each stays on one line, so blocks
    /// inside them are only separated by a space.
    one_line: usize,
    /// Addresses already written as a link.
    linked: HashSet<String>,
}

impl Writer {
    fn text(&mut self, text: &str) {
        if self.pre > 0 {
            let text = if self.out.ends_with("```\n") {
                // HTML drops a newline right after <pre>.
                text.strip_prefix('\n').unwrap_or(text)
            } else {
                text
            };
            self.out.push_str(text);
            self.no_space = false;
            return;
        }
        let space = |c: char| c.is_ascii_whitespace();
        if text.starts_with(space) {
            self.space();
        }
        let mut wrote = false;
        for word in text.split_ascii_whitespace() {
            if wrote {
                self.space();
            }
            self.out.push_str(word);
            self.no_space = false;
            wrote = true;
        }
        if wrote && text.ends_with(space) {
            self.space();
        }
    }

    fn space(&mut self) {
        if !self.no_space && !self.out.is_empty() && !self.out.ends_with([' ', '\n']) {
            self.out.push(' ');
        }
    }

    fn trim_spaces(&mut self) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
    }

    /// End the current line and leave `blank` empty lines (0 or 1).
    fn block(&mut self, blank: usize) {
        if self.one_line > 0 {
            self.space();
            return;
        }
        self.trim_spaces();
        self.no_space = false;
        if self.out.is_empty() {
            return;
        }
        let want = 1 + blank;
        let have = self.out.len() - self.out.trim_end_matches('\n').len();
        for _ in have..want {
            self.out.push('\n');
        }
    }
}

fn convert(html: &str, options: &Options, selector: Option<&Selector>) -> Page {
    let mut w = Writer {
        out: String::with_capacity(html.len() / 8),
        pre: 0,
        lists: 0,
        no_space: false,
        one_line: 0,
        linked: HashSet::new(),
    };
    let mut title = String::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut matched = false;
    let bytes = html.as_bytes();
    let mut i = 0;

    // Whether text written inside the open elements is shown: none of them
    // is left out, and one of them matched the selector if there is one.
    let visible = |stack: &[Open]| {
        !stack.iter().any(|o| o.skipped) && (selector.is_none() || stack.iter().any(|o| o.selected))
    };
    // Close the open elements from `pos` up.
    let close_from = |stack: &mut Vec<Open>, w: &mut Writer, pos: usize| {
        while stack.len() > pos {
            let shown = visible(stack);
            let open = stack.pop().expect("non-empty");
            end_element(w, open, shown);
        }
    };

    while i < bytes.len() {
        if bytes[i] != b'<' {
            let end = html[i..].find('<').map_or(html.len(), |n| i + n);
            if visible(&stack) {
                w.text(&decode_entities(&html[i..end]));
            }
            i = end;
            continue;
        }
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(html.len(), |n| i + n + 3);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            i = rest.find('>').map_or(html.len(), |n| i + n + 1);
            continue;
        }
        if let Some(after) = rest.strip_prefix("</") {
            let name_len = after
                .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                .unwrap_or(after.len());
            let name = after[..name_len].to_ascii_lowercase();
            i = rest.find('>').map_or(html.len(), |n| i + n + 1);
            if let Some(pos) = stack.iter().rposition(|o| o.name == name) {
                close_from(&mut stack, &mut w, pos);
            }
            continue;
        }
        let Some((tag, len, self_closing)) = parse_tag(rest) else {
            // A lone "<" in text.
            if visible(&stack) {
                w.text("<");
            }
            i += 1;
            continue;
        };
        i += len;

        if RAW_TEXT.contains(&tag.name.as_str()) {
            let close = format!("</{}", tag.name);
            let end = find_ignore_case(&html[i..], &close).map_or(html.len(), |n| i + n);
            if tag.name == "title" && title.is_empty() {
                title = collapse(&decode_entities(&html[i..end]));
            } else if tag.name == "textarea" && visible(&stack) && !tag.hidden() {
                // Some sites show code or text in a read-only box (GitHub's
                // file view does): kept line for line, like `pre`.
                let body = decode_entities(&html[i..end]);
                let body = body.strip_prefix('\n').unwrap_or(&body).trim_end();
                if !body.trim().is_empty() {
                    w.block(1);
                    w.out.push_str("```\n");
                    w.out.push_str(body);
                    w.out.push_str("\n```");
                    w.block(1);
                }
            }
            i = html[end..].find('>').map_or(html.len(), |n| end + n + 1);
            continue;
        }

        // Implied end tags: a new item closes the open one, a block closes
        // an open paragraph; only within the nearest list or table.
        let implied: &[&str] = match tag.name.as_str() {
            "li" => &["li"],
            "dt" | "dd" => &["dt", "dd"],
            "tr" => &["tr", "td", "th"],
            "td" | "th" => &["td", "th"],
            "option" => &["option"],
            name if CLOSES_P.contains(&name) => &["p"],
            _ => &[],
        };
        if !implied.is_empty() {
            const BOUNDARY: &[&str] = &["ul", "ol", "dl", "table", "tbody", "thead", "select"];
            if let Some(pos) = stack.iter().rposition(|o| {
                implied.contains(&o.name.as_str()) || BOUNDARY.contains(&o.name.as_str())
            }) {
                if implied.contains(&stack[pos].name.as_str()) {
                    close_from(&mut stack, &mut w, pos);
                }
            }
        }

        let skipped = SKIPPED.contains(&tag.name.as_str()) || tag.hidden();
        let selected = selector.is_some_and(|s| s.matches(&tag));
        matched |= selected && !skipped && !stack.iter().any(|o| o.skipped);
        let void = VOID.contains(&tag.name.as_str()) || self_closing;
        let mut open = Open {
            name: tag.name.clone(),
            skipped,
            selected,
            link: None,
            mark: None,
            count: tag
                .attr("start")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(1),
        };
        let shown = !skipped
            && !stack.iter().any(|o| o.skipped)
            && (selector.is_none() || selected || stack.iter().any(|o| o.selected));
        if shown {
            start_element(&mut w, &tag, &mut open, &mut stack, options);
        }
        if !void {
            stack.push(open);
        } else if shown {
            end_element(&mut w, open, true);
        }
    }
    close_from(&mut stack, &mut w, 0);

    Page {
        title,
        text: tidy(&w.out),
        selected: selector.is_some() && matched,
    }
}

fn start_element(
    w: &mut Writer,
    tag: &Tag,
    open: &mut Open,
    stack: &mut [Open],
    options: &Options,
) {
    let name = tag.name.as_str();
    match name {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            w.block(1);
            open.mark = Some(w.out.len());
            let level = (name.as_bytes()[1] - b'0') as usize;
            w.out.push_str(&"#".repeat(level));
            w.out.push(' ');
            w.no_space = true;
            w.one_line += 1;
        }
        "br" if w.one_line > 0 => w.space(),
        "br" => {
            w.trim_spaces();
            w.out.push('\n');
        }
        "li" => {
            w.block(0);
            open.mark = Some(w.out.len());
            w.out.push_str(&"  ".repeat(w.lists.saturating_sub(1)));
            match stack
                .iter_mut()
                .rev()
                .find(|o| o.name == "ul" || o.name == "ol")
            {
                Some(list) if list.name == "ol" => {
                    w.out.push_str(&format!("{}. ", list.count));
                    list.count += 1;
                }
                _ => w.out.push_str("- "),
            }
            w.no_space = true;
        }
        "ul" | "ol" => {
            w.block(if w.lists == 0 { 1 } else { 0 });
            w.lists += 1;
        }
        "tr" | "dt" | "dd" => w.block(0),
        "td" | "th" => {
            if let Some(row) = stack.iter_mut().rev().find(|o| o.name == "tr") {
                if row.count > 1 {
                    w.trim_spaces();
                    w.out.push_str(" | ");
                    w.no_space = true;
                }
                row.count += 1;
            }
            w.one_line += 1;
        }
        "pre" => {
            w.block(1);
            w.out.push_str("```\n");
            w.pre += 1;
        }
        "code" | "kbd" | "samp" if w.pre == 0 => {
            w.out.push('`');
            w.no_space = true;
        }
        "a" if options.links && w.pre == 0 => {
            if let Some(url) = tag.attr("href").and_then(|h| link_target(h, options.base)) {
                open.link = Some((w.out.len(), url));
                w.out.push('[');
                w.no_space = true;
            }
        }
        _ if BLOCK.contains(&name) => w.block(1),
        _ => {}
    }
}

/// Write the end of an element that `start_element` began; `shown` is
/// whether its start was written.
fn end_element(w: &mut Writer, open: Open, shown: bool) {
    if !shown {
        return;
    }
    match open.name.as_str() {
        "pre" => {
            if w.pre > 0 {
                w.out.truncate(w.out.trim_end().len());
                w.out.push_str("\n```");
                w.pre -= 1;
                w.block(1);
            }
        }
        "ul" | "ol" => {
            w.lists = w.lists.saturating_sub(1);
            w.block(if w.lists == 0 { 1 } else { 0 });
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            w.one_line = w.one_line.saturating_sub(1);
            take_back_if_empty(w, open.mark);
            w.block(1);
        }
        "li" => {
            take_back_if_empty(w, open.mark);
            w.block(0);
        }
        "tr" | "dt" | "dd" => w.block(0),
        "td" | "th" => {
            w.one_line = w.one_line.saturating_sub(1);
            w.trim_spaces();
        }
        "code" | "kbd" | "samp" if w.pre == 0 => {
            w.trim_spaces();
            if w.out.ends_with('`') {
                // Nothing inside: drop the opening backtick.
                w.out.pop();
            } else {
                w.out.push('`');
            }
            w.no_space = false;
        }
        "a" => {
            if let Some((at, url)) = open.link {
                let inner = w.out[at + 1..].trim();
                if inner.is_empty()
                    || inner.contains('\n')
                    || inner == url
                    || !w.linked.insert(url.clone())
                {
                    w.out.remove(at);
                } else {
                    w.trim_spaces();
                    w.out.push_str("](");
                    w.out.push_str(&url);
                    w.out.push(')');
                }
                w.no_space = false;
            }
        }
        name if BLOCK.contains(&name) => w.block(1),
        _ => {}
    }
}

/// Remove a heading's or list item's marker written at `mark` when nothing
/// followed it (an icon-only heading, an item whose content was left out).
fn take_back_if_empty(w: &mut Writer, mark: Option<usize>) {
    let Some(mark) = mark.filter(|&m| m <= w.out.len()) else {
        return;
    };
    let written = w.out[mark..].trim();
    let marker_only = written.is_empty()
        || written.bytes().all(|b| b == b'#')
        || written == "-"
        || written
            .strip_suffix('.')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if marker_only {
        w.out.truncate(mark);
    }
}

/// Where a link's `href` points, as an absolute address worth writing:
/// `None` for in-page anchors, scripts and addresses that don't parse.
fn link_target(href: &str, base: Option<&Url>) -> Option<String> {
    let href = decode_entities(href.trim());
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("javascript:") || lower.starts_with("data:") {
        return None;
    }
    let url = match base {
        Some(base) => base.join(&href).ok()?,
        None => Url::parse(&href).ok()?,
    };
    if !matches!(url.scheme(), "http" | "https" | "mailto") {
        return None;
    }
    let same_site = base.is_some_and(|b| {
        b.scheme() == url.scheme() && b.host_str() == url.host_str() && b.port() == url.port()
    });
    if same_site && url.scheme() != "mailto" {
        let mut short = url.path().to_string();
        for (mark, part) in [('?', url.query()), ('#', url.fragment())] {
            if let Some(part) = part {
                short.push(mark);
                short.push_str(part);
            }
        }
        return Some(short);
    }
    Some(url.to_string())
}

/// A start tag at the start of `text` ("<name ...>"): the tag, its length in
/// bytes and whether it closed itself ("/>"). `None` when `text` is not a
/// tag (a "<" in text).
fn parse_tag(text: &str) -> Option<(Tag, usize, bool)> {
    let bytes = text.as_bytes();
    if bytes.len() < 2 || !bytes[1].is_ascii_alphabetic() {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/'
    {
        i += 1;
    }
    let name = text[1..i].to_ascii_lowercase();
    let mut attrs = Vec::new();
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return Some((Tag { name, attrs }, text.len(), false));
        }
        match bytes[i] {
            b'>' => return Some((Tag { name, attrs }, i + 1, false)),
            b'/' if bytes.get(i + 1) == Some(&b'>') => {
                return Some((Tag { name, attrs }, i + 2, true))
            }
            b'/' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && !matches!(bytes[i], b'=' | b'>' | b'/')
        {
            i += 1;
        }
        let attr = text[start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if bytes.get(i) == Some(&b'=') {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            match bytes.get(i) {
                Some(&q) if q == b'"' || q == b'\'' => {
                    let end = text[i + 1..]
                        .find(q as char)
                        .map_or(text.len(), |n| i + 1 + n);
                    value = text[i + 1..end].to_string();
                    i = (end + 1).min(text.len());
                }
                _ => {
                    let start = i;
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                        i += 1;
                    }
                    value = text[start..i].to_string();
                }
            }
        }
        if !attr.is_empty() {
            attrs.push((attr, value));
        } else if i == start {
            // A stray character: step over it rather than loop.
            i += 1;
        }
    }
}

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` with its character references (`&amp;`, `&#39;`, `&#x2014;`, the
/// common named ones) replaced by the characters they stand for; unknown ones
/// are left as written.
pub fn decode_entities(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('&') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .map_or(rest.len(), |n| n + 1);
        let name = &rest[1..end];
        let decoded = if let Some(num) = name.strip_prefix('#') {
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse().ok(),
            };
            code.and_then(char::from_u32).map(String::from)
        } else {
            named_entity(name).map(String::from)
        };
        match decoded {
            Some(s) if !name.is_empty() => {
                out.push_str(&s);
                rest = &rest[end..];
                rest = rest.strip_prefix(';').unwrap_or(rest);
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "‘",
        "rsquo" => "’",
        "ldquo" => "“",
        "rdquo" => "”",
        "bull" => "•",
        "middot" => "·",
        "times" => "×",
        "laquo" => "«",
        "raquo" => "»",
        "euro" => "€",
        "pound" => "£",
        "yen" => "¥",
        "cent" => "¢",
        "deg" => "°",
        "plusmn" => "±",
        "para" => "¶",
        "sect" => "§",
        "rarr" => "→",
        "larr" => "←",
        "uarr" => "↑",
        "darr" => "↓",
        "check" => "✓",
        "zwj" | "zwnj" | "shy" => "",
        _ => return None,
    })
}

/// Trailing spaces off every line, at most one blank line in a row (outside
/// code fences, which keep their lines), no blank start or end.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    let mut in_fence = false;
    for line in text.lines() {
        let line = if in_fence { line } else { line.trim_end() };
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if line.trim().is_empty() && !in_fence {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank > 0 {
                out.push('\n');
            }
        }
        blank = 0;
        out.push_str(line);
    }
    out
}

#[cfg(test)]
#[path = "readable_tests.rs"]
mod tests;
