use std::path::Path;
use tree_sitter::{Parser, Query, QueryCursor};

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
    parser
        .set_language(&language.into())
        .expect("Error loading Rust grammar");

    let tree = parser.parse(source, None).unwrap();
    let query_scm = r#"
        (function_item name: (_) @name) @func
        (struct_item name: (_) @name) @struct
        (enum_item name: (_) @name) @enum
        (trait_item name: (_) @name) @trait
        (impl_item type: (_) @name) @impl
        (mod_item name: (_) @name) @mod
    "#;

    let query = Query::new(&language.into(), query_scm).unwrap();
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let captures = cursor.captures(&query, tree.root_node(), source.as_bytes());

    for (m, _) in captures {
        let mut node = None;
        let mut name_node = None;

        for capture in m.captures {
            let name = capture_names[capture.index as usize];
            if name == "name" {
                name_node = Some(capture.node);
            } else {
                node = Some(capture.node);
            }
        }

        let (node, name_node) = match (node, name_node) {
            (Some(n), Some(nn)) => (n, nn),
            _ => continue,
        };
        let name = source[name_node.byte_range()].to_string();

        let kind = match m.pattern_index {
            0 => SymbolKind::Function,
            1 => SymbolKind::Struct,
            2 => SymbolKind::Enum,
            3 => SymbolKind::Trait,
            4 => SymbolKind::Struct, // impl block
            5 => SymbolKind::Module,
            _ => SymbolKind::Unknown,
        };

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        let signature = source[node.byte_range()]
            .lines()
            .next()
            .unwrap_or("")
            .trim_end_matches('{')
            .trim()
            .to_string();

        symbols.push(CodeSymbol {
            name,
            kind,
            start_line,
            end_line,
            signature,
            children: vec![],
        });
    }
}

fn extract_python_symbols_ts(source: &str, symbols: &mut Vec<CodeSymbol>) {
    let mut parser = Parser::new();
    let language = tree_sitter_python::LANGUAGE;
    parser
        .set_language(&language.into())
        .expect("Error loading Python grammar");

    let tree = parser.parse(source, None).unwrap();
    let query_scm = r#"
        (function_definition name: (_) @name) @func
        (class_definition name: (_) @name) @class
    "#;

    let query = Query::new(&language.into(), query_scm).unwrap();
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let captures = cursor.captures(&query, tree.root_node(), source.as_bytes());

    for (m, _) in captures {
        let mut node = None;
        let mut name_node = None;

        for capture in m.captures {
            let name = capture_names[capture.index as usize];
            if name == "name" {
                name_node = Some(capture.node);
            } else {
                node = Some(capture.node);
            }
        }

        let (node, name_node) = match (node, name_node) {
            (Some(n), Some(nn)) => (n, nn),
            _ => continue,
        };
        let name = source[name_node.byte_range()].to_string();

        let kind = match m.pattern_index {
            0 => SymbolKind::Function,
            1 => SymbolKind::Class,
            _ => SymbolKind::Unknown,
        };

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        let signature = source[node.byte_range()]
            .lines()
            .next()
            .unwrap_or("")
            .trim_end_matches(':')
            .trim()
            .to_string();

        symbols.push(CodeSymbol {
            name,
            kind,
            start_line,
            end_line,
            signature,
            children: vec![],
        });
    }
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

pub fn symbols_to_repo_map(_path: &Path, symbols: &[CodeSymbol]) -> String {
    let mut out = String::new();
    for sym in symbols {
        let kind_str = match sym.kind {
            SymbolKind::Function => "func",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Module => "mod",
            _ => "sym",
        };
        out.push_str(&format!(
            "  {} {} (L{}-L{})\n",
            kind_str, sym.name, sym.start_line, sym.end_line
        ));
    }
    out
}

pub fn generate_repo_map(root: &Path) -> String {
    let mut out = String::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .build();

    for entry in walker.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let path = entry.path();
            let symbols = if let Ok(content) = std::fs::read_to_string(path) {
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                extract_symbols_generic(&content, ext)
            } else {
                continue;
            };

            if !symbols.is_empty() {
                out.push_str(&format!(
                    "{}:\n",
                    path.strip_prefix(root).unwrap_or(path).display()
                ));
                out.push_str(&symbols_to_repo_map(path, &symbols));
                out.push('\n');
            }
        }
    }
    out
}
