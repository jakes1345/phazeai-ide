use std::ops::Range;
use std::path::Path;
use tree_sitter::{Parser, Query, QueryCursor};
use streaming_iterator::StreamingIterator;

/// Reparent any symbol whose byte range is fully inside another symbol's byte range
/// as a child of the outer one. Functions nested inside impl/class become `Method`.
/// Input is expected to be in document order (tree-sitter `matches` already yields it
/// that way), but we sort defensively. Top-level symbols are returned in `out`.
fn nest_by_range(items: Vec<(CodeSymbol, Range<usize>)>) -> Vec<CodeSymbol> {
    let mut items = items;
    // Largest container first, so we attach inner symbols to the right parent.
    items.sort_by_key(|(_, r)| (r.start, std::cmp::Reverse(r.end)));

    let mut roots: Vec<(CodeSymbol, Range<usize>)> = Vec::new();
    for (mut sym, range) in items {
        if let Some(idx) = find_innermost_container_idx(&roots, &range) {
            if matches!(sym.kind, SymbolKind::Function) {
                sym.kind = SymbolKind::Method;
            }
            roots[idx].0.children.push(sym);
        } else {
            roots.push((sym, range));
        }
    }
    roots.into_iter().map(|(s, _)| s).collect()
}

/// Find the index of the innermost already-collected symbol whose byte range
/// strictly contains `target`. Returns None if `target` has no enclosing symbol
/// among `roots`. Index-based to avoid borrow-checker conflicts when later
/// pushing as a child.
fn find_innermost_container_idx(
    roots: &[(CodeSymbol, Range<usize>)],
    target: &Range<usize>,
) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None; // (idx, range_len)
    for (i, (_sym, range)) in roots.iter().enumerate() {
        if range.start <= target.start && range.end >= target.end && *range != *target {
            let len = range.end - range.start;
            match best {
                Some((_, blen)) if blen <= len => {}
                _ => best = Some((i, len)),
            }
        }
    }
    best.map(|(i, _)| i)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
    pub children: Vec<CodeSymbol>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Class,
    Enum,
    Trait,
    Interface,
    Module,
    Variable,
    Constant,
    Unknown,
}

pub fn extract_symbols(path: &Path, source: &str) -> Vec<CodeSymbol> {
    let mut symbols = Vec::new();
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");

    match extension {
        "rs" => extract_rust_symbols_ts(source, &mut symbols),
        "py" => extract_python_symbols_ts(source, &mut symbols),
        _ => {}
    }

    symbols
}

fn extract_rust_symbols_ts(source: &str, symbols: &mut Vec<CodeSymbol>) {
    let mut parser = Parser::new();
    let language = tree_sitter_rust::LANGUAGE;
    if parser.set_language(&language.into()).is_err() {
        tracing::warn!(target: "phazeai_core::outline", "failed to load Rust grammar; skipping outline");
        return;
    }

    let Some(tree) = parser.parse(source, None) else {
        tracing::debug!(target: "phazeai_core::outline", "tree-sitter Rust parse returned None");
        return;
    };

    // Each pattern uses a unique outer capture name so we can map capture-name → kind
    // without depending on capture-index ordering (which is global per query).
    let query_scm = r#"
        (function_item name: (identifier) @name) @func
        (struct_item name: (type_identifier) @name) @struct
        (enum_item name: (type_identifier) @name) @enum
        (trait_item name: (type_identifier) @name) @trait
        (impl_item type: (type_identifier) @name) @impl
        (mod_item name: (identifier) @name) @mod
    "#;

    let query = match Query::new(&language.into(), query_scm) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!(target: "phazeai_core::outline", error = %e, "failed to compile Rust outline query");
            return;
        }
    };
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut collected: Vec<(CodeSymbol, Range<usize>)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut name_node = None;
        let mut outer_node = None;
        let mut kind = SymbolKind::Unknown;
        for cap in m.captures {
            let cname = capture_names.get(cap.index as usize).copied().unwrap_or("");
            match cname {
                "name" => name_node = Some(cap.node),
                "func" => {
                    outer_node = Some(cap.node);
                    kind = SymbolKind::Function;
                }
                "struct" => {
                    outer_node = Some(cap.node);
                    kind = SymbolKind::Struct;
                }
                "enum" => {
                    outer_node = Some(cap.node);
                    kind = SymbolKind::Enum;
                }
                "trait" => {
                    outer_node = Some(cap.node);
                    kind = SymbolKind::Trait;
                }
                "impl" => {
                    // `impl Foo` and `impl Trait for Foo` should both yield a
                    // top-level symbol named after the type. We use SymbolKind::Struct
                    // so existing repo_map output stays consistent with the type kind.
                    outer_node = Some(cap.node);
                    kind = SymbolKind::Struct;
                }
                "mod" => {
                    outer_node = Some(cap.node);
                    kind = SymbolKind::Module;
                }
                _ => {}
            }
        }
        let (Some(name_node), Some(outer)) = (name_node, outer_node) else {
            continue;
        };

        let name = source[name_node.byte_range()].to_string();
        let start_line = outer.start_position().row + 1;
        let end_line = outer.end_position().row + 1;
        let signature = source[outer.byte_range()]
            .lines()
            .next()
            .unwrap_or("")
            .trim_end_matches('{')
            .trim()
            .to_string();

        collected.push((
            CodeSymbol {
                name,
                kind,
                start_line,
                end_line,
                signature,
                children: vec![],
            },
            outer.byte_range(),
        ));
    }

    // For `impl Foo { ... }` blocks the same `Foo` may also be matched as a struct
    // declaration elsewhere, but here we only see the impl-`@name` capture which is
    // the type. We still want one symbol per impl block, with its functions attached
    // as Method children. nest_by_range handles that purely from byte ranges.
    symbols.extend(nest_by_range(collected));
}

fn extract_python_symbols_ts(source: &str, symbols: &mut Vec<CodeSymbol>) {
    let mut parser = Parser::new();
    let language = tree_sitter_python::LANGUAGE;
    if parser.set_language(&language.into()).is_err() {
        tracing::warn!(target: "phazeai_core::outline", "failed to load Python grammar; skipping outline");
        return;
    }
    let Some(tree) = parser.parse(source, None) else {
        tracing::debug!(target: "phazeai_core::outline", "tree-sitter Python parse returned None");
        return;
    };

    let query_scm = r#"
        (function_definition name: (identifier) @name) @func
        (class_definition name: (identifier) @name) @class
    "#;

    let query = match Query::new(&language.into(), query_scm) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!(target: "phazeai_core::outline", error = %e, "failed to compile Python outline query");
            return;
        }
    };
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut collected: Vec<(CodeSymbol, Range<usize>)> = Vec::new();

    while let Some(m) = matches.next() {
        let mut name_node = None;
        let mut outer_node = None;
        let mut kind = SymbolKind::Unknown;
        for cap in m.captures {
            let cname = capture_names.get(cap.index as usize).copied().unwrap_or("");
            match cname {
                "name" => name_node = Some(cap.node),
                "func" => {
                    outer_node = Some(cap.node);
                    kind = SymbolKind::Function;
                }
                "class" => {
                    outer_node = Some(cap.node);
                    kind = SymbolKind::Class;
                }
                _ => {}
            }
        }
        let (Some(name_node), Some(outer)) = (name_node, outer_node) else {
            continue;
        };

        let name = source[name_node.byte_range()].to_string();
        let start_line = outer.start_position().row + 1;
        let end_line = outer.end_position().row + 1;
        let signature = source[outer.byte_range()]
            .lines()
            .next()
            .unwrap_or("")
            .trim_end_matches(':')
            .trim()
            .to_string();

        collected.push((
            CodeSymbol {
                name,
                kind,
                start_line,
                end_line,
                signature,
                children: vec![],
            },
            outer.byte_range(),
        ));
    }

    symbols.extend(nest_by_range(collected));
}

pub fn extract_symbols_generic(source: &str, extension: &str) -> Vec<CodeSymbol> {
    let mut symbols = Vec::new();
    match extension {
        "rs" => extract_rust_symbols_ts(source, &mut symbols),
        "py" => extract_python_symbols_ts(source, &mut symbols),
        _ => {}
    }
    symbols
}

pub fn symbols_to_repo_map(path: &Path, symbols: &[CodeSymbol]) -> String {
    let mut out = String::new();
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        out.push_str(name);
        out.push('\n');
    }
    fn write_sym(out: &mut String, sym: &CodeSymbol, indent: usize) {
        let kind_str = match sym.kind {
            SymbolKind::Function => "func",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Module => "mod",
            _ => "sym",
        };
        let pad = "  ".repeat(indent + 1);
        out.push_str(&format!(
            "{pad}{kind_str} {} (L{}-L{})\n",
            sym.name, sym.start_line, sym.end_line
        ));
        for child in &sym.children {
            write_sym(out, child, indent + 1);
        }
    }
    for sym in symbols {
        write_sym(&mut out, sym, 0);
    }
    out
}

pub fn generate_repo_map(root: &Path) -> String {
    let mut out = String::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .build();

    for result in walker {
        if let Ok(entry) = result {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                let path = entry.path();
                let symbols = if let Ok(content) = std::fs::read_to_string(path) {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    extract_symbols_generic(&content, ext)
                } else {
                    continue;
                };

                if !symbols.is_empty() {
                    out.push_str(&format!("{}:\n", path.strip_prefix(root).unwrap_or(path).display()));
                    out.push_str(&symbols_to_repo_map(path, &symbols));
                    out.push_str("\n");
                }
            }
        }
    }
    out
}
