use super::*;

fn text(html: &str) -> String {
    to_text(html, &Options::default()).text
}

fn with_links(html: &str, base: &str) -> String {
    let base = Url::parse(base).unwrap();
    to_text(
        html,
        &Options {
            links: true,
            base: Some(&base),
            selector: None,
        },
    )
    .text
}

#[test]
fn scripts_styles_head_and_hidden_parts_are_left_out() {
    let html = r#"<!DOCTYPE html><html><head><title>The  Page</title>
        <style>body { color: red }</style><script>var x = "<p>not text</p>";</script>
        <meta name="x" content="y"></head>
        <body><nav><a href="/">Home</a> <a href="/docs">Docs</a></nav>
        <!-- a comment with <b>tags</b> -->
        <svg viewBox="0 0 16 16"><path d="M1 1"/></svg>
        <p>Hello <b>world</b>.</p>
        <div hidden>secret menu</div><span aria-hidden="true">icon</span>
        <noscript>enable JavaScript</noscript>
        <footer>© 2026</footer></body></html>"#;
    let page = to_text(html, &Options::default());
    assert_eq!(page.title, "The Page");
    assert_eq!(page.text, "Hello world.");
}

#[test]
fn headings_paragraphs_and_lists_become_markdown() {
    let html = "<h1>Title</h1><p>First para\n   wraps here.</p><p>Second</p>\
        <h2>Steps</h2><ol><li>one</li><li>two<ul><li>nested</li></ul></li><li>three</ol>\
        <ul><li>a<li>b</ul><p>after";
    assert_eq!(
        text(html),
        "# Title\n\nFirst para wraps here.\n\nSecond\n\n## Steps\n\n\
         1. one\n2. two\n  - nested\n3. three\n\n- a\n- b\n\nafter"
    );
}

#[test]
fn an_ordered_list_can_start_elsewhere() {
    assert_eq!(text("<ol start=\"4\"><li>d<li>e</ol>"), "4. d\n5. e");
}

#[test]
fn preformatted_code_keeps_its_lines_in_a_fence() {
    let html = "<p>Run:</p><pre><code class=\"lang-rust\">\nfn main() {\n    println!(\"&lt;hi&gt;\");\n}\n</code></pre><p>Done</p>";
    assert_eq!(
        text(html),
        "Run:\n\n```\nfn main() {\n    println!(\"<hi>\");\n}\n```\n\nDone"
    );
}

#[test]
fn inline_code_and_spacing_around_inline_tags() {
    assert_eq!(
        text("<p>Call <code>to_text</code> here, or <em>not</em>.</p>"),
        "Call `to_text` here, or not."
    );
    assert_eq!(text("<p><b>a</b> <i>b</i><span>c</span></p>"), "a bc");
    assert_eq!(text("<p>x<code></code>y</p>"), "xy");
}

#[test]
fn links_are_absolute_and_skip_anchors_and_scripts() {
    let html = r##"<p>See <a href="/docs/intro?x=1&amp;y=2">the intro</a>,
        <a href="#top">top</a>, <a href="javascript:void(0)">menu</a>,
        <a href="https://example.org/">https://example.org/</a> and
        <a href="other.html"><img src="i.png"></a>.</p>"##;
    assert_eq!(
        with_links(html, "https://site.test/a/b.html"),
        "See [the intro](/docs/intro?x=1&y=2), top, menu, https://example.org/ and ."
    );
    // A link inside code is left as its text.
    assert_eq!(
        with_links("<pre>x: <a href=\"/T\">T</a></pre>", "https://s.test/"),
        "```\nx: T\n```"
    );
    // Without links, only the text.
    assert_eq!(
        text(html),
        "See the intro, top, menu, https://example.org/ and ."
    );
}

#[test]
fn tables_become_rows_of_cells() {
    let html = "<table><thead><tr><th>Name</th><th>Size</th></tr></thead>\
        <tbody><tr><td>a.rs</td><td>12</td><tr><td>b.rs<td>7</tbody></table>";
    assert_eq!(text(html), "Name | Size\na.rs | 12\nb.rs | 7");
}

#[test]
fn entities_are_decoded_and_unknown_ones_kept() {
    assert_eq!(
        decode_entities("a &amp; b &lt;c&gt; &#39;d&#x27; &mdash; &bogus; & e &nbsp;f"),
        "a & b <c> 'd' — &bogus; & e  f"
    );
    assert_eq!(decode_entities("plain"), "plain");
}

#[test]
fn attributes_with_brackets_and_odd_markup_do_not_break_the_scan() {
    let html =
        r#"<div data-x="a > b" title='it"s'>kept</div>< not a tag <p class=x>also</p><br/>end"#;
    assert_eq!(text(html), "kept\n\n< not a tag\n\nalso\n\nend");
    // Unclosed elements at the end of the input.
    assert_eq!(text("<ul><li>one<li>two"), "- one\n- two");
}

#[test]
fn a_read_only_text_box_is_shown_as_its_text() {
    let html = "<p>File:</p><textarea readonly>line 1\nline &amp; 2</textarea>";
    assert_eq!(text(html), "File:\n\n```\nline 1\nline & 2\n```");
}

#[test]
fn a_selector_keeps_only_what_it_matches() {
    let html = r#"<div class="sidebar">menu</div>
        <main id="content" class="docs body"><h1>API</h1><p>Text</p></main>
        <article class="post"><p>Post</p></article>"#;
    let pick = |sel: &str| {
        to_text(
            html,
            &Options {
                selector: Some(sel),
                ..Options::default()
            },
        )
    };
    assert_eq!(pick("main").text, "# API\n\nText");
    assert_eq!(pick("#content").text, "# API\n\nText");
    assert_eq!(pick(".docs").text, "# API\n\nText");
    assert_eq!(pick("article.post").text, "Post");
    assert_eq!(pick("body .post").text, "Post");
    assert!(pick("main").selected);
    // Nothing matches: the whole page, marked as not selected.
    let none = pick(".missing");
    assert!(!none.selected);
    assert!(none.text.starts_with("menu"), "{}", none.text);
}

#[test]
fn selector_parsing() {
    let s = |t: &str| Selector::parse(t);
    assert_eq!(
        s("div.note"),
        Some(Selector {
            tag: Some("div".into()),
            id: None,
            class: Some("note".into())
        })
    );
    assert_eq!(
        s("section#a.b"),
        Some(Selector {
            tag: Some("section".into()),
            id: Some("a".into()),
            class: Some("b".into())
        })
    );
    assert_eq!(s(""), None);
    assert_eq!(s("  "), None);
}

#[test]
fn content_types() {
    assert!(is_html("text/html; charset=utf-8"));
    assert!(is_html("application/xhtml+xml"));
    assert!(!is_html("application/json"));
    assert!(!is_html("text/plain"));
}

#[test]
fn empty_headings_and_items_are_dropped_and_cells_stay_on_their_row() {
    assert_eq!(
        text("<h2><div></div><div>Title</div></h2>x"),
        "## Title\n\nx"
    );
    let html = "<h2><svg></svg></h2><p>Body</p><ul><li><button>x</button></li><li>real</li></ul>\
        <table><tr><td><div>Name</div><p>more</p></td><td>Date<br>time</td></tr></table>";
    assert_eq!(text(html), "Body\n\n- real\n\nName more | Date time");
}

#[test]
fn same_site_links_are_written_from_their_path_and_each_address_once() {
    let html = r#"<p><a href="/a">A</a> <a href="https://other.test/b">B</a>
        <a href="https://site.test:8443/c">C</a> <a href="http://site.test/d">D</a>
        <a href="/a">A again</a> <a href="mailto:x@site.test">mail</a></p>"#;
    assert_eq!(
        with_links(html, "https://site.test/page"),
        "[A](/a) [B](https://other.test/b) [C](https://site.test:8443/c) \
         [D](http://site.test/d) A again [mail](mailto:x@site.test)"
    );
}
