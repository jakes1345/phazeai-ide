use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
};

use floem::{
    event::{Event, EventListener},
    keyboard::{Key, Modifiers},
    kurbo::{Circle, Point},
    reactive::{create_effect, create_memo, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    text::{Attrs, AttrsList, FamilyOwned, Stretch, Style as TextStyle, Weight},
    views::{
        canvas,
        container,
        editor::{
            core::{
                buffer::rope_text::RopeText,
                cursor::{Cursor, CursorMode},
                editor::EditType,
                selection::Selection,
            },
            id::EditorId,
            layout::TextLayoutLine,
            text::{default_dark_color, Document, SimpleStylingBuilder, Styling, WrapMethod},
            EditorStyle,
        },
        dyn_stack, label, stack, text_editor, text_input, Decorators,
    },
    IntoView, Renderer,
};
use lazy_static::lazy_static;
use syntect::{
    highlighting::{FontStyle, HighlightState, Highlighter, RangedHighlightIterator, ThemeSet},
    parsing::{ParseState, ScopeStack, SyntaxSet},
};

use crate::{
    components::icon::{icons, phaze_icon},
    theme::PhazeTheme,
};

// ── Syntect globals (lazy_static → 'static lifetimes) ────────────────────────

lazy_static! {
    static ref SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
    static ref THEME_SET: ThemeSet = ThemeSet::load_defaults();
}

// ── Syntax Highlighting Styling ───────────────────────────────────────────────

/// A `Styling` implementation that uses syntect for per-line syntax highlighting.
/// Wraps an inner `Rc<dyn Styling>` (the SimpleStylingBuilder output) for
/// font/layout settings and adds color spans from syntect on top.
struct SyntaxStyle {
    inner: Rc<dyn Styling>,
    highlighter: Highlighter<'static>,
    parse_state_proto: ParseState,
    doc: Option<Rc<dyn Document>>,
    /// Cached incremental states, one entry per 16-line block.
    states: RefCell<Vec<(ParseState, HighlightState)>>,
}

impl SyntaxStyle {
    /// Create a `SyntaxStyle` for the given file extension.
    /// Falls back to plain-text if no matching grammar is found.
    fn for_extension(ext: &str, inner: Rc<dyn Styling>) -> Self {
        let theme = &THEME_SET.themes["base16-ocean.dark"];
        let highlighter = Highlighter::new(theme);

        // Map common extensions → syntect scope names
        let syntax = match ext {
            "rs"                           => SYNTAX_SET.find_syntax_by_extension("rs"),
            "py" | "pyw"                   => SYNTAX_SET.find_syntax_by_extension("py"),
            "js" | "mjs" | "cjs"           => SYNTAX_SET.find_syntax_by_extension("js"),
            "ts" | "tsx"                   => SYNTAX_SET.find_syntax_by_extension("ts"),
            "jsx"                          => SYNTAX_SET.find_syntax_by_extension("jsx"),
            "json" | "jsonc"               => SYNTAX_SET.find_syntax_by_extension("json"),
            "toml"                         => SYNTAX_SET.find_syntax_by_extension("toml"),
            "md" | "mdx" | "markdown"      => SYNTAX_SET.find_syntax_by_extension("md"),
            "html" | "htm"                 => SYNTAX_SET.find_syntax_by_extension("html"),
            "css"                          => SYNTAX_SET.find_syntax_by_extension("css"),
            "scss" | "sass"                => SYNTAX_SET.find_syntax_by_extension("scss"),
            "c" | "h"                      => SYNTAX_SET.find_syntax_by_extension("c"),
            "cpp" | "cc" | "cxx" | "hpp"   => SYNTAX_SET.find_syntax_by_extension("cpp"),
            "go"                           => SYNTAX_SET.find_syntax_by_extension("go"),
            "sh" | "bash" | "zsh"          => SYNTAX_SET.find_syntax_by_extension("sh"),
            "yaml" | "yml"                 => SYNTAX_SET.find_syntax_by_extension("yaml"),
            "xml"                          => SYNTAX_SET.find_syntax_by_extension("xml"),
            "sql"                          => SYNTAX_SET.find_syntax_by_extension("sql"),
            "lua"                          => SYNTAX_SET.find_syntax_by_extension("lua"),
            "rb"                           => SYNTAX_SET.find_syntax_by_extension("rb"),
            "java"                         => SYNTAX_SET.find_syntax_by_extension("java"),
            "kt" | "kts"                   => SYNTAX_SET.find_syntax_by_extension("kt"),
            "swift"                        => SYNTAX_SET.find_syntax_by_extension("swift"),
            "cs"                           => SYNTAX_SET.find_syntax_by_extension("cs"),
            _                              => None,
        }
        .or_else(|| SYNTAX_SET.find_syntax_plain_text().into())
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

        let parse_state_proto = ParseState::new(syntax);

        Self {
            inner,
            highlighter,
            parse_state_proto,
            doc: None,
            states: RefCell::new(Vec::new()),
        }
    }

    fn set_doc(&mut self, doc: Rc<dyn Document>) {
        self.doc = Some(doc);
    }
}

impl Styling for SyntaxStyle {
    fn id(&self) -> u64 { self.inner.id() }

    fn font_size(&self, edid: EditorId, line: usize) -> usize {
        self.inner.font_size(edid, line)
    }

    fn line_height(&self, edid: EditorId, line: usize) -> f32 {
        self.inner.line_height(edid, line)
    }

    fn font_family(&self, edid: EditorId, line: usize) -> Cow<'_, [FamilyOwned]> {
        self.inner.font_family(edid, line)
    }

    fn weight(&self, edid: EditorId, line: usize) -> Weight {
        self.inner.weight(edid, line)
    }

    fn italic_style(&self, edid: EditorId, line: usize) -> TextStyle {
        self.inner.italic_style(edid, line)
    }

    fn stretch(&self, edid: EditorId, line: usize) -> Stretch {
        self.inner.stretch(edid, line)
    }

    fn indent_line(&self, edid: EditorId, line: usize, line_content: &str) -> usize {
        self.inner.indent_line(edid, line, line_content)
    }

    fn tab_width(&self, edid: EditorId, line: usize) -> usize {
        self.inner.tab_width(edid, line)
    }

    fn atomic_soft_tabs(&self, edid: EditorId, line: usize) -> bool {
        self.inner.atomic_soft_tabs(edid, line)
    }

    fn apply_attr_styles(
        &self,
        _edid: EditorId,
        _style: &EditorStyle,
        line: usize,
        default: Attrs,
        attrs: &mut AttrsList,
    ) {
        attrs.clear_spans();
        let Some(doc) = &self.doc else { return };

        let mut states_cache = self.states.borrow_mut();
        // Rebuild cache up to the nearest 16-line boundary before `line`
        let start = (line >> 4).min(states_cache.len());
        states_cache.truncate(start);

        // Seed from the cached state or from scratch
        let mut states = states_cache.last().cloned().unwrap_or_else(|| {
            (
                self.parse_state_proto.clone(),
                HighlightState::new(&self.highlighter, ScopeStack::new()),
            )
        });

        let rope = doc.rope_text();
        for line_no in start..=line {
            let text = rope.line_content(line_no).to_string();
            if let Ok(ops) = states.0.parse_line(&text, &SYNTAX_SET) {
                if line_no == line {
                    for (style, _text, range) in RangedHighlightIterator::new(
                        &mut states.1,
                        &ops,
                        &text,
                        &self.highlighter,
                    ) {
                        let mut attr = default.clone();
                        if style.font_style.contains(FontStyle::ITALIC) {
                            attr = attr.style(TextStyle::Italic);
                        }
                        if style.font_style.contains(FontStyle::BOLD) {
                            attr = attr.weight(Weight::BOLD);
                        }
                        attr = attr.color(floem::peniko::Color::from_rgba8(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                            style.foreground.a,
                        ));
                        attrs.add_span(range, attr);
                    }
                }
            }

            // Cache state every 16 lines
            if line_no & 0xF == 0xF {
                states_cache.push(states.clone());
            }
        }
    }

    fn apply_layout_styles(
        &self,
        edid: EditorId,
        style: &EditorStyle,
        line: usize,
        layout_line: &mut TextLayoutLine,
    ) {
        self.inner.apply_layout_styles(edid, style, line, layout_line);
    }

    fn paint_caret(&self, edid: EditorId, line: usize) -> bool {
        self.inner.paint_caret(edid, line)
    }
}

// ── Tab state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct TabState {
    path: PathBuf,
    name: String,
    dirty: RwSignal<bool>,
}

// ── Editor panel ──────────────────────────────────────────────────────────────

/// Full multi-tab code editor with syntect syntax highlighting.
///
/// `ai_thinking` drives the sentient-gutter glow on the left edge.
/// `lsp_cmd` notifies the LSP server on every edit (did_change).
/// `active_cursor` is written with (path, 0-based line, 0-based col) whenever
///   the active editor's cursor moves — read by the completion popup.
pub fn editor_panel(
    open_file: RwSignal<Option<PathBuf>>,
    theme: RwSignal<PhazeTheme>,
    ai_thinking: RwSignal<bool>,
    lsp_cmd: tokio::sync::mpsc::UnboundedSender<crate::lsp_bridge::LspCommand>,
    active_cursor: RwSignal<Option<(PathBuf, u32, u32)>>,
    pending_completion: RwSignal<Option<String>>,
    diagnostics: RwSignal<Vec<crate::lsp_bridge::DiagEntry>>,
) -> impl IntoView {
    let tabs: RwSignal<Vec<TabState>> = create_rw_signal(vec![]);
    let active_idx: RwSignal<Option<usize>> = create_rw_signal(None);
    let font_size: RwSignal<usize> = create_rw_signal(14);

    let docs: Rc<RefCell<HashMap<String, Rc<dyn Document>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let docs_for_stack = docs.clone();
    let docs_for_save  = docs.clone();
    let docs_for_find  = docs.clone();

    // ── Find in file (Ctrl+F) ────────────────────────────────────────────────
    let find_open:  RwSignal<bool>   = create_rw_signal(false);
    let find_query: RwSignal<String> = create_rw_signal(String::new());
    // Incremented to trigger a cursor jump to `find_jump_offset`.
    let find_jump_nonce:  RwSignal<u64>   = create_rw_signal(0u64);
    let find_jump_offset: RwSignal<usize> = create_rw_signal(0usize);
    let find_cur_match:   RwSignal<usize> = create_rw_signal(0usize);

    // ── Replace (Ctrl+H) ─────────────────────────────────────────────────────
    let replace_query: RwSignal<String> = create_rw_signal(String::new());
    let replace_open:  RwSignal<bool>   = create_rw_signal(false);
    // Incremented to trigger "replace current match" in the active editor.
    let replace_nonce:     RwSignal<u64> = create_rw_signal(0u64);
    // Incremented to trigger "replace all matches" in the active editor.
    let replace_all_nonce: RwSignal<u64> = create_rw_signal(0u64);

    // Compute match offsets reactively (for display + navigation)
    let find_match_offsets = create_memo({
        let docs_for_find = docs_for_find.clone();
        move |_| -> Vec<usize> {
            let q = find_query.get();
            if q.is_empty() { return vec![]; }
            let Some(idx) = active_idx.get() else { return vec![]; };
            let list = tabs.get();
            let Some(tab) = list.get(idx) else { return vec![]; };
            let key = tab.path.to_string_lossy().to_string();
            let reg = docs_for_find.borrow();
            let Some(doc) = reg.get(&key) else { return vec![]; };
            let text = doc.text().to_string();
            let q_lo = q.to_lowercase();
            let t_lo = text.to_lowercase();
            let mut offs = vec![];
            let mut start = 0usize;
            while let Some(pos) = t_lo[start..].find(&q_lo) {
                offs.push(start + pos);
                start += pos + q_lo.len().max(1);
            }
            offs
        }
    });

    // ── Go-to line (Ctrl+G) ──────────────────────────────────────────────────
    let goto_open:  RwSignal<bool>   = create_rw_signal(false);
    let goto_query: RwSignal<String> = create_rw_signal(String::new());
    // Nonce — when incremented, the active editor recreates at `goto_line`.
    let goto_nonce: RwSignal<u64>   = create_rw_signal(0u64);
    let goto_line:  RwSignal<usize> = create_rw_signal(1usize);

    // React to file-open requests from the explorer
    let _ = create_memo(move |_| {
        let path = open_file.get();
        if let Some(p) = path {
            let existing = tabs.get().iter().position(|t| t.path == p);
            if let Some(idx) = existing {
                active_idx.set(Some(idx));
            } else {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "untitled".to_string());
                tabs.update(|list| {
                    list.push(TabState { path: p.clone(), name, dirty: create_rw_signal(false) });
                    active_idx.set(Some(list.len() - 1));
                });
            }
        }
    });

    // Ctrl+S save handler
    let save_fn = Rc::new(move || {
        let Some(idx) = active_idx.get() else { return };
        let tab_list = tabs.get();
        let Some(tab) = tab_list.get(idx) else { return };
        let key = tab.path.to_string_lossy().to_string();
        let registry = docs_for_save.borrow();
        let Some(doc) = registry.get(&key) else { return };
        let content = doc.text().to_string();
        if std::fs::write(&tab.path, content).is_ok() {
            tab.dirty.set(false);
            // Run formatter in background — file is already saved to disk
            let path = tab.path.clone();
            std::thread::spawn(move || {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let formatter: Option<(&str, Vec<String>)> = match ext {
                    "rs" => Some(("rustfmt", vec![path.to_string_lossy().to_string()])),
                    "js" | "ts" | "jsx" | "tsx" | "json" => Some((
                        "prettier",
                        vec![
                            "--write".to_string(),
                            path.to_string_lossy().to_string(),
                        ],
                    )),
                    "py" => Some(("black", vec![path.to_string_lossy().to_string()])),
                    _ => None,
                };
                if let Some((cmd, args)) = formatter {
                    let _ = std::process::Command::new(cmd).args(&args).status();
                }
            });
        }
    });
    let save_fn_bar = save_fn.clone();
    let save_fn_key = save_fn.clone();

    let tab_bar = tab_bar_view(tabs, active_idx, theme, save_fn_bar, diagnostics);

    // ── Sentient gutter ────────────────────────────────────────────────────
    let sentient_gutter = canvas(move |cx, size| {
        let t = theme.get();
        let p = &t.palette;
        cx.fill(&floem::kurbo::Rect::ZERO.with_size(size), p.bg_base, 0.0);
        let h = size.height;
        let w = size.width;
        if ai_thinking.get() {
            for frac in [0.15, 0.50, 0.85] {
                cx.fill(&Circle::new(Point::new(w * 0.5, h * frac), 18.0), p.accent.with_alpha(0.70), 14.0);
            }
        } else {
            cx.fill(&Circle::new(Point::new(w * 0.5, h * 0.08), 22.0), p.accent.with_alpha(0.39), 18.0);
            cx.fill(&Circle::new(Point::new(w * 0.5, h * 0.30), 16.0), p.accent.with_alpha(0.16), 12.0);
        }
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.width(4.0).height_full().min_width(4.0).background(p.bg_base)
    });

    // ── Editor body ─────────────────────────────────────────────────────────
    // Key by path only — editors are NEVER recreated on font-size or goto-line
    // changes.  Font-size updates call editor.update_styling() reactively.
    // Goto-line uses the same nonce-effect pattern as find-cursor-jump.
    // This preserves the undo/redo stack across zoom and navigation.
    let editor_body = dyn_stack(
        move || {
            tabs.get()
                .into_iter()
                .enumerate()
                .collect::<Vec<_>>()
        },
        |(_i, tab)| format!("{}", tab.path.to_string_lossy()),
        move |(i, tab)| {
            let is_active = move || active_idx.get() == Some(i);
            let key = tab.path.to_string_lossy().to_string();
            let dirty = tab.dirty;

            // Read font_size once for initial construction (not tracked).
            let initial_fs = font_size.get_untracked();

            // Preserve unsaved edits across tab switches by reading doc registry first.
            let content = {
                let reg = docs_for_stack.borrow();
                reg.get(&key)
                    .map(|d| d.text().to_string())
                    .unwrap_or_else(|| std::fs::read_to_string(&tab.path).unwrap_or_default())
            };

            let tab_ext = tab.path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string();

            let make_base_styling = |fs: usize| -> Rc<dyn Styling> {
                Rc::new(
                    SimpleStylingBuilder::default()
                        .wrap(WrapMethod::None)
                        .font_size(fs)
                        .font_family(vec![
                            FamilyOwned::Name("JetBrains Mono".to_string()),
                            FamilyOwned::Name("Fira Code".to_string()),
                            FamilyOwned::Name("Cascadia Code".to_string()),
                            FamilyOwned::Monospace,
                        ])
                        .build(),
                )
            };

            // Build the raw editor first so we can grab the doc and cursor.
            let raw_editor = text_editor(content);
            let cursor_sig  = raw_editor.editor().cursor;   // RwSignal<Cursor>
            let editor_ref  = raw_editor.editor().clone();  // Clone for reactive updates
            let doc         = raw_editor.doc().clone();
            // Clone doc ref for the LSP update callback (same Rc — UI-thread only).
            let doc_for_lsp = doc.clone();
            let lsp_ver: RwSignal<i32> = create_rw_signal(0i32);
            let lsp_path    = tab.path.clone();
            let lsp_tx      = lsp_cmd.clone();

            // ── Goto-line cursor jump (reactive effect, no editor recreation) ─
            {
                let last_nonce = create_rw_signal(0u64);
                let doc_for_goto = doc.clone();
                create_effect(move |_| {
                    let nonce = goto_nonce.get();
                    if nonce == 0 || nonce == last_nonce.get() { return; }
                    if active_idx.get() != Some(i) { return; }
                    last_nonce.set(nonce);
                    let rope     = doc_for_goto.rope_text();
                    let line_0   = goto_line.get().saturating_sub(1);
                    let max_line = rope.num_lines().saturating_sub(1);
                    let offset   = rope.offset_of_line(line_0.min(max_line));
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(offset)),
                        None,
                        None,
                    ));
                });
            }

            // ── Find-in-file cursor jump effect ──────────────────────────
            // When `find_jump_nonce` increments and this tab is active, jump
            // cursor to `find_jump_offset` WITHOUT recreating the editor.
            {
                let last_nonce = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = find_jump_nonce.get();
                    if nonce == 0 || nonce == last_nonce.get() { return; }
                    if active_idx.get() != Some(i) { return; }
                    last_nonce.set(nonce);
                    let offset = find_jump_offset.get();
                    cursor_sig.set(Cursor::new(
                        CursorMode::Insert(Selection::caret(offset)),
                        None,
                        None,
                    ));
                });
            }

            // ── Cursor position tracking → active_cursor signal ──────────
            // Fires whenever cursor moves in the active editor; converts the
            // byte offset to (line, col) and writes to the shared signal so
            // Ctrl+Space can pass the right position to the LSP.
            {
                let track_path = tab.path.clone();
                let track_doc  = doc.clone();
                create_effect(move |_| {
                    if active_idx.get() != Some(i) { return; }
                    let cursor = cursor_sig.get();
                    let offset = cursor.offset();
                    let rope  = track_doc.rope_text();
                    let line  = rope.line_of_offset(offset) as u32;
                    let col   = (offset - rope.offset_of_line(line as usize)) as u32;
                    active_cursor.set(Some((track_path.clone(), line, col)));
                });
            }

            // ── Completion insertion effect ───────────────────────────────
            // When `pending_completion` is set and this tab is active, insert
            // the text at the current cursor offset and clear the signal.
            {
                let doc_for_comp = doc.clone();
                let last_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let Some(text) = pending_completion.get() else { return };
                    if active_idx.get() != Some(i) { return; }
                    // Consume the signal immediately to avoid re-triggering.
                    pending_completion.set(None);
                    let offset = cursor_sig.get().offset();
                    let sel = Selection::caret(offset);
                    doc_for_comp.edit_single(sel, &text, EditType::InsertChars);
                    // Suppress the nonce-based spurious re-run
                    last_nonce.update(|n| *n += 1);
                });
            }

            // ── Replace current match ─────────────────────────────────────
            {
                let doc_for_repl = doc.clone();
                let last_repl_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = replace_nonce.get();
                    if nonce == 0 || nonce == last_repl_nonce.get() { return; }
                    if active_idx.get() != Some(i) { return; }
                    last_repl_nonce.set(nonce);
                    let offsets = find_match_offsets.get();
                    let cur = find_cur_match.get();
                    let Some(&start) = offsets.get(cur) else { return };
                    let q = find_query.get();
                    let end = start + q.len();
                    let sel = Selection::region(start, end);
                    let replacement = replace_query.get();
                    doc_for_repl.edit_single(sel, &replacement, EditType::InsertChars);
                });
            }

            // ── Replace all matches ───────────────────────────────────────
            {
                let doc_for_repl_all = doc.clone();
                let last_repl_all_nonce: RwSignal<u64> = create_rw_signal(0u64);
                create_effect(move |_| {
                    let nonce = replace_all_nonce.get();
                    if nonce == 0 || nonce == last_repl_all_nonce.get() { return; }
                    if active_idx.get() != Some(i) { return; }
                    last_repl_all_nonce.set(nonce);
                    let offsets = find_match_offsets.get();
                    if offsets.is_empty() { return; }
                    let q = find_query.get();
                    let replacement = replace_query.get();
                    // Replace from last to first to preserve earlier offsets.
                    for &start in offsets.iter().rev() {
                        let end = start + q.len();
                        let sel = Selection::region(start, end);
                        doc_for_repl_all.edit_single(sel, &replacement, EditType::InsertChars);
                    }
                });
            }

            // Build initial syntect-based styling for this file's language
            let base_styling = make_base_styling(initial_fs);
            let mut syn_style = SyntaxStyle::for_extension(&tab_ext, base_styling);
            syn_style.set_doc(doc.clone());

            // ── Reactive font-size update (preserves undo stack) ──────────
            // When font_size changes, rebuild styling in place instead of
            // recreating the editor (which would destroy undo history).
            {
                let doc_for_style = doc.clone();
                let ext_for_style = tab_ext.clone();
                let editor_for_style = editor_ref.clone();
                create_effect(move |_| {
                    let fs = font_size.get();
                    let new_base = make_base_styling(fs);
                    let mut new_style = SyntaxStyle::for_extension(&ext_for_style, new_base);
                    new_style.set_doc(doc_for_style.clone());
                    editor_for_style.update_styling(Rc::new(new_style));
                });
            }

            // Store in registry for save + find
            docs_for_stack.borrow_mut().insert(key, doc);

            raw_editor
                .styling(syn_style)
                .editor_style(move |style| {
                    let t = theme.get();
                    let p = &t.palette;
                    default_dark_color(style)
                        .gutter_dim_color(p.text_disabled)
                        .gutter_accent_color(p.text_primary)
                        .gutter_current_color(p.bg_elevated)
                        .gutter_left_padding(6.0)
                        .gutter_right_padding(10.0)
                })
                .update(move |_| {
                    dirty.set(true);
                    // Notify LSP server of content change (textDocument/didChange).
                    let text = doc_for_lsp.text().to_string();
                    let ver = lsp_ver.get();
                    lsp_ver.update(|v| *v += 1);
                    let _ = lsp_tx.send(crate::lsp_bridge::LspCommand::ChangeFile {
                        path: lsp_path.clone(),
                        text,
                        version: ver,
                    });
                })
                .style(move |s| {
                    s.size_full()
                     .apply_if(!is_active(), |s| s.display(floem::style::Display::None))
                })
        },
    )
    .style(|s| s.flex_grow(1.0).min_height(0.0).min_width(0.0).width_full());

    // ── Neon scrollbar heatmap ─────────────────────────────────────────────
    let heatmap = canvas(move |cx, size| {
        let t = theme.get();
        let p = &t.palette;
        let h = size.height;
        let w = size.width;
        cx.fill(&floem::kurbo::Rect::ZERO.with_size(size), p.glass_bg, 0.0);
        cx.fill(&Circle::new(Point::new(w * 0.5, h * 0.02 + 6.0), 5.0), p.success.with_alpha(0.86), 6.0);
        cx.fill(&Circle::new(Point::new(w * 0.5, h * 0.5), 5.0), p.accent.with_alpha(0.86), 7.0);
        cx.fill(&floem::kurbo::Rect::new(0.0, 0.0, 1.0, h), p.border, 0.0);
    })
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let bg = if t.is_cosmic() { p.glass_bg } else { p.bg_deep };
        s.width(8.0).height_full().min_width(8.0).background(bg)
    });

    // ── Welcome screen ─────────────────────────────────────────────────────
    let welcome = container(
        stack((
            phaze_icon(icons::FILE, 48.0, move |p| p.text_muted, theme),
            label(|| "Open a file to start editing")
                .style(move |s| {
                    s.color(theme.get().palette.text_muted).font_size(13.0).margin_top(8.0)
                }),
            label(|| "Use the explorer panel on the left")
                .style(move |s| {
                    s.color(theme.get().palette.text_disabled).font_size(11.0).margin_top(4.0)
                }),
        ))
        .style(|s| s.flex_col().items_center()),
    )
    .style(move |s| {
        let has = !tabs.get().is_empty();
        s.flex_grow(1.0).min_height(0.0).items_center().justify_center()
         .apply_if(has, |s| s.display(floem::style::Display::None))
    });

    let content_area = stack((welcome, editor_body))
        .style(|s| s.flex_grow(1.0).min_height(0.0).min_width(0.0).width_full());

    let editor_row = stack((sentient_gutter, content_area, heatmap))
        .style(|s| s.flex_grow(1.0).min_height(0.0).min_width(0.0).width_full());

    // ── Find bar (Ctrl+F) ─────────────────────────────────────────────────────
    let find_bar = {
        let match_label = label(move || {
            let offsets = find_match_offsets.get();
            let cur = find_cur_match.get();
            if offsets.is_empty() {
                if find_query.get().is_empty() { "".to_string() }
                else { "No matches".to_string() }
            } else {
                format!("{}/{}", cur + 1, offsets.len())
            }
        })
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let no_match = !find_query.get().is_empty() && find_match_offsets.get().is_empty();
            s.font_size(12.0)
             .color(if no_match { p.error } else { p.text_muted })
             .margin_left(8.0)
             .min_width(80.0)
        });

        let prev_btn = container(label(|| "↑"))
            .style(move |s| {
                s.padding_horiz(8.0).padding_vert(3.0)
                 .font_size(13.0)
                 .color(theme.get().palette.text_secondary)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .border_radius(4.0)
                 .hover(|s| s.background(theme.get().palette.bg_elevated))
            })
            .on_click_stop(move |_| {
                let offs = find_match_offsets.get();
                if offs.is_empty() { return; }
                let cur = find_cur_match.get();
                let prev = if cur == 0 { offs.len() - 1 } else { cur - 1 };
                find_cur_match.set(prev);
                find_jump_offset.set(offs[prev]);
                find_jump_nonce.update(|n| *n += 1);
            });

        let next_btn = container(label(|| "↓"))
            .style(move |s| {
                s.padding_horiz(8.0).padding_vert(3.0)
                 .font_size(13.0)
                 .color(theme.get().palette.text_secondary)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .border_radius(4.0)
                 .hover(|s| s.background(theme.get().palette.bg_elevated))
            })
            .on_click_stop(move |_| {
                let offs = find_match_offsets.get();
                if offs.is_empty() { return; }
                let cur = find_cur_match.get();
                let next = (cur + 1) % offs.len();
                find_cur_match.set(next);
                find_jump_offset.set(offs[next]);
                find_jump_nonce.update(|n| *n += 1);
            });

        let close_btn = container(label(|| "✕"))
            .style(move |s| {
                s.padding_horiz(8.0).padding_vert(3.0)
                 .font_size(12.0)
                 .color(theme.get().palette.text_muted)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .border_radius(4.0)
                 .hover(|s| s.background(theme.get().palette.bg_elevated))
            })
            .on_click_stop(move |_| {
                find_open.set(false);
                find_query.set(String::new());
            });

        let find_input = text_input(find_query)
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.width(220.0).padding_horiz(8.0).padding_vert(4.0)
                 .font_size(13.0)
                 .color(p.text_primary)
                 .background(p.bg_elevated)
                 .border(1.0)
                 .border_color(p.border_focus)
                 .border_radius(4.0)
            });

        let replace_input = text_input(replace_query)
            .placeholder("Replace…")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.width(180.0).padding_horiz(8.0).padding_vert(4.0)
                 .font_size(13.0)
                 .color(p.text_primary)
                 .background(p.bg_elevated)
                 .border(1.0)
                 .border_color(p.border)
                 .border_radius(4.0)
                 .apply_if(!replace_open.get(), |s| s.display(floem::style::Display::None))
            });

        let replace_btn = container(label(|| "Replace"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(8.0).padding_vert(3.0).font_size(12.0)
                 .color(p.text_secondary)
                 .background(p.bg_elevated)
                 .border(1.0).border_color(p.border)
                 .border_radius(4.0)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .hover(|s| s.background(p.accent_dim))
                 .apply_if(!replace_open.get(), |s| s.display(floem::style::Display::None))
            })
            .on_click_stop(move |_| {
                replace_nonce.update(|n| *n += 1);
            });

        let replace_all_btn = container(label(|| "All"))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(8.0).padding_vert(3.0).font_size(12.0)
                 .color(p.accent)
                 .background(p.bg_elevated)
                 .border(1.0).border_color(p.border)
                 .border_radius(4.0)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .hover(|s| s.background(p.accent_dim))
                 .apply_if(!replace_open.get(), |s| s.display(floem::style::Display::None))
            })
            .on_click_stop(move |_| {
                replace_all_nonce.update(|n| *n += 1);
            });

        let replace_toggle = container(label(move || if replace_open.get() { "▾" } else { "▸" }))
            .style(move |s| {
                let t = theme.get();
                s.padding_horiz(6.0).padding_vert(3.0).font_size(11.0)
                 .color(t.palette.text_muted)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .border_radius(3.0)
                 .hover(|s| s.background(t.palette.bg_elevated))
            })
            .on_click_stop(move |_| replace_open.update(|v| *v = !*v));

        container(
            stack((replace_toggle, find_input, match_label, prev_btn, next_btn,
                   replace_input, replace_btn, replace_all_btn, close_btn))
                .style(|s| s.items_center().gap(4.0)),
        )
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let shown = find_open.get();
            s.width_full().height(36.0).items_center()
             .padding_horiz(12.0)
             .background(p.bg_elevated)
             .border_bottom(1.0)
             .border_color(p.border)
             .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        // Escape closes the find bar
        .on_event_stop(EventListener::KeyDown, move |ev| {
            if let Event::KeyDown(e) = ev {
                match &e.key.logical_key {
                    Key::Named(floem::keyboard::NamedKey::Escape) => {
                        find_open.set(false);
                        find_query.set(String::new());
                        replace_open.set(false);
                    }
                    Key::Named(floem::keyboard::NamedKey::Enter) => {
                        let offs = find_match_offsets.get();
                        if offs.is_empty() { return; }
                        let cur = find_cur_match.get();
                        let next = (cur + 1) % offs.len();
                        find_cur_match.set(next);
                        find_jump_offset.set(offs[next]);
                        find_jump_nonce.update(|n| *n += 1);
                    }
                    _ => {}
                }
            }
        })
    };

    // ── Goto-line overlay (Ctrl+G) ────────────────────────────────────────────
    let goto_overlay = {
        let input = text_input(goto_query)
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.width(160.0).padding(8.0)
                 .font_size(14.0)
                 .color(p.text_primary)
                 .background(p.bg_elevated)
                 .border(1.0)
                 .border_color(p.border_focus)
                 .border_radius(6.0)
            });

        let hint = label(|| "Go to line…")
            .style(move |s| {
                s.font_size(11.0).color(theme.get().palette.text_muted).margin_bottom(6.0)
            });

        let box_view = stack((hint, input))
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.flex_col().padding(16.0)
                 .background(p.bg_panel)
                 .border(1.0)
                 .border_color(p.glass_border)
                 .border_radius(8.0)
                 .box_shadow_h_offset(0.0)
                 .box_shadow_v_offset(4.0)
                 .box_shadow_blur(30.0)
                 .box_shadow_color(p.glow)
                 .box_shadow_spread(0.0)
            })
            .on_event_stop(EventListener::KeyDown, move |ev| {
                if let Event::KeyDown(e) = ev {
                    match &e.key.logical_key {
                        Key::Named(floem::keyboard::NamedKey::Escape) => {
                            goto_open.set(false);
                            goto_query.set(String::new());
                        }
                        Key::Named(floem::keyboard::NamedKey::Enter) => {
                            let q = goto_query.get();
                            if let Ok(n) = q.trim().parse::<usize>() {
                                goto_line.set(n);
                                goto_nonce.update(|v| *v += 1);
                            }
                            goto_open.set(false);
                            goto_query.set(String::new());
                        }
                        _ => {}
                    }
                }
            });

        container(box_view)
            .style(move |s| {
                let shown = goto_open.get();
                s.absolute()
                 .inset(0)
                 .items_start()
                 .justify_center()
                 .padding_top(60.0)
                 .background(floem::peniko::Color::from_rgba8(0, 0, 0, 140))
                 .z_index(50)
                 .apply_if(!shown, |s| s.display(floem::style::Display::None))
            })
            .on_click_stop(move |_| {
                goto_open.set(false);
                goto_query.set(String::new());
            })
    };

    stack((tab_bar, find_bar, editor_row, goto_overlay))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let bg = if t.is_cosmic() { p.glass_bg } else { p.bg_deep };
            s.flex_col().flex_grow(1.0).min_width(0.0).height_full().background(bg)
        })
        .on_event_stop(EventListener::KeyDown, move |event| {
            if let Event::KeyDown(e) = event {
                let ctrl  = e.modifiers.contains(Modifiers::CONTROL);
                let shift = e.modifiers.contains(Modifiers::SHIFT);
                if ctrl && !shift {
                    if let Key::Character(ref ch) = e.key.logical_key {
                        match ch.as_str() {
                            "s" => save_fn_key(),
                            "=" | "+" => font_size.update(|s| *s = (*s + 1).min(32)),
                            "-"       => font_size.update(|s| { if *s > 8 { *s -= 1; } }),
                            "0"       => font_size.set(14),
                            "f"       => {
                                find_open.set(true);
                                replace_open.set(false);
                                find_cur_match.set(0);
                            }
                            "h"       => {
                                find_open.set(true);
                                replace_open.set(true);
                                find_cur_match.set(0);
                            }
                            "g"       => {
                                goto_open.set(true);
                                goto_query.set(String::new());
                            }
                            _ => {}
                        }
                    }
                }
            }
        })
}

// ── Tab bar ───────────────────────────────────────────────────────────────────

fn tab_bar_view(
    tabs: RwSignal<Vec<TabState>>,
    active_idx: RwSignal<Option<usize>>,
    theme: RwSignal<PhazeTheme>,
    _save_fn: Rc<dyn Fn()>,
    diagnostics: RwSignal<Vec<crate::lsp_bridge::DiagEntry>>,
) -> impl IntoView {
    dyn_stack(
        move || tabs.get().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, _)| *i,
        move |(i, tab)| {
            let is_active = active_idx.get() == Some(i);
            let is_hovered = create_rw_signal(false);
            let name = tab.name.clone();
            let dirty = tab.dirty;
            let tab_path = tab.path.clone();

            // Compute this tab's highest-severity diagnostic.
            let diag_color = move || -> Option<floem::peniko::Color> {
                let p = theme.get().palette;
                let diags = diagnostics.get();
                let has_err = diags.iter().any(|d| d.path == tab_path && d.severity == crate::lsp_bridge::DiagSeverity::Error);
                let has_warn = diags.iter().any(|d| d.path == tab_path && d.severity == crate::lsp_bridge::DiagSeverity::Warning);
                if has_err { Some(p.error) }
                else if has_warn { Some(p.warning) }
                else { None }
            };

            container(
                stack((
                    // Dirty indicator
                    container(label(|| "●"))
                        .style(move |s| {
                            s.font_size(8.0)
                             .color(theme.get().palette.accent)
                             .margin_right(4.0)
                             .apply_if(!dirty.get(), |s| s.display(floem::style::Display::None))
                        }),
                    // Diagnostic dot (error=red, warning=yellow)
                    container(label(|| "●"))
                        .style(move |s| {
                            let color = diag_color();
                            s.font_size(8.0)
                             .margin_right(4.0)
                             .apply_if(color.is_none(), |s| s.display(floem::style::Display::None))
                             .apply_if(color.is_some(), move |s| s.color(color.unwrap_or(floem::peniko::Color::TRANSPARENT)))
                        }),
                    label(move || name.clone())
                        .style(move |s| {
                            let t = theme.get();
                            s.font_size(13.0)
                             .color(if is_active { t.palette.text_primary } else { t.palette.text_muted })
                        }),
                    phaze_icon(icons::CLOSE, 10.0, move |p| p.text_muted, theme)
                    .style(move |s: floem::style::Style| {
                        s.margin_left(8.0).width(16.0).height(16.0).items_center()
                         .justify_center().border_radius(3.0)
                         .cursor(floem::style::CursorStyle::Pointer)
                         .hover(|s| s.background(theme.get().palette.bg_elevated))
                    })
                    .on_click_stop(move |_| {
                        tabs.update(|list| { if i < list.len() { list.remove(i); } });
                        active_idx.update(|cur| {
                            let len = tabs.get().len();
                            if len == 0 { *cur = None; }
                            else { *cur = Some(cur.unwrap_or(0).min(len - 1)); }
                        });
                    }),
                ))
                .style(|s| s.items_center()),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let bg = if is_active { p.bg_surface }
                         else if is_hovered.get() { p.bg_elevated }
                         else { p.bg_panel };
                s.height_full().padding_horiz(16.0).background(bg)
                 .border_right(1.0).border_color(p.border)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .apply_if(is_active, |s| {
                     s.border_top(1.0).border_color(p.accent)
                      .box_shadow_v_offset(-1.0).box_shadow_blur(12.0).box_shadow_color(p.glow)
                      .border_bottom(2.0).border_color(p.accent)
                 })
                 .items_center()
            })
            .on_click_stop(move |_| active_idx.set(Some(i)))
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| is_hovered.set(true))
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| is_hovered.set(false))
        },
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let bar_bg = if t.is_cosmic() { p.glass_bg } else { p.bg_deep };
        s.height(35.0).width_full().background(bar_bg)
         .border_bottom(1.0).border_color(p.border).min_width(0.0)
    })
}
