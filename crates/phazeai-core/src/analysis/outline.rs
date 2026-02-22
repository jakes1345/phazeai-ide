/// Tree-sitter based code outline extractor.
/// Extracts function, class, struct, and method symbols from source code.
/// Inspired by Zed's outline view and Aider's repo map.
use std::path::Path;

/// A symbol extracted from source code
#[derive(Debug, Clone)]
pub struct CodeSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
    pub children: Vec<CodeSymbol>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
    Module,
    Constant,
    Variable,
    Import,
    Type,
    Unknown,
}

impl SymbolKind {
    pub fn icon(&self) -> &str {
        match self {
            SymbolKind::Function | SymbolKind::Method => "ƒ",
            SymbolKind::Class => "C",
            SymbolKind::Struct => "S",
            SymbolKind::Enum => "E",
            SymbolKind::Interface | SymbolKind::Trait => "I",
            SymbolKind::Module => "M",
            SymbolKind::Constant => "K",
            SymbolKind::Variable => "V",
            SymbolKind::Import => "⬇",
            SymbolKind::Type => "T",
            SymbolKind::Unknown => "?",
        }
    }
}

/// Extract symbols from a file using tree-sitter.
/// This doesn't require compiled tree-sitter grammars — it uses
/// a generic heuristic approach that works across languages.
pub fn extract_symbols_generic(source: &str, extension: &str) -> Vec<CodeSymbol> {
    let mut symbols = Vec::new();

    // Use regex-based extraction as a fast fallback that works everywhere
    // without requiring compiled tree-sitter grammars
    match extension {
        "rs" => extract_rust_symbols(source, &mut symbols),
        "py" => extract_python_symbols(source, &mut symbols),
        "js" | "jsx" | "ts" | "tsx" | "mjs" => extract_js_symbols(source, &mut symbols),
        "go" => extract_go_symbols(source, &mut symbols),
        "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" => extract_c_symbols(source, &mut symbols),
        "java" => extract_java_symbols(source, &mut symbols),
        _ => extract_generic_symbols(source, &mut symbols),
    }

    symbols
}

/// Generate a compact repo map string from symbols (like Aider does).
/// This gives the LLM a high-level view of a file without sending all the code.
pub fn symbols_to_repo_map(path: &Path, symbols: &[CodeSymbol]) -> String {
    let mut map = String::new();
    let filename = path.file_name().unwrap_or_default().to_string_lossy();
    map.push_str(&format!("{}:\n", filename));

    for sym in symbols {
        let indent = "  ";
        map.push_str(&format!(
            "{}{} {} {}\n",
            indent,
            sym.kind.icon(),
            sym.name,
            if sym.signature.is_empty() {
                String::new()
            } else {
                format!("| {}", sym.signature)
            }
        ));

        for child in &sym.children {
            map.push_str(&format!(
                "    {} {} {}\n",
                child.kind.icon(),
                child.name,
                if child.signature.is_empty() {
                    String::new()
                } else {
                    format!("| {}", child.signature)
                }
            ));
        }
    }
    map
}

/// Generate a full repo map for a directory (like Aider's repomap.py)
pub fn generate_repo_map(root: &Path) -> String {
    let mut map = String::new();
    let walker = ignore::Walk::new(root);

    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        // Only process source code files
        if !is_source_file(ext) {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(path) {
            let symbols = extract_symbols_generic(&content, ext);
            if !symbols.is_empty() {
                let relative = path.strip_prefix(root).unwrap_or(path);
                map.push_str(&symbols_to_repo_map(relative, &symbols));
                map.push('\n');
            }
        }
    }

    map
}

fn is_source_file(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "c"
            | "cpp"
            | "cc"
            | "cxx"
            | "h"
            | "hpp"
            | "java"
            | "rb"
            | "lua"
            | "sh"
            | "bash"
            | "mjs"
            | "mts"
    )
}

// ── Language-specific extractors ──────────────────────────────

fn extract_rust_symbols(source: &str, symbols: &mut Vec<CodeSymbol>) {
    let mut current_impl: Option<String> = None;
    let mut impl_methods: Vec<CodeSymbol> = Vec::new();

    for (line_num, line) in source.lines().enumerate() {
        let trimmed = line.trim();

        // Detect `impl` blocks
        if trimmed.starts_with("impl ") || trimmed.starts_with("impl<") {
            // Save previous impl block
            if let Some(ref impl_name) = current_impl {
                if !impl_methods.is_empty() {
                    symbols.push(CodeSymbol {
                        name: impl_name.clone(),
                        kind: SymbolKind::Struct,
                        start_line: line_num,
                        end_line: line_num,
                        signature: String::new(),
                        children: std::mem::take(&mut impl_methods),
                    });
                }
            }
            current_impl = extract_impl_name(trimmed);
            continue;
        }

        // Functions and methods
        if trimmed.starts_with("pub fn ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("pub async fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.starts_with("pub(crate) fn ")
        {
            let sig = trimmed.trim_end_matches('{').trim().to_string();
            let name = extract_fn_name(trimmed);
            let sym = CodeSymbol {
                name,
                kind: if current_impl.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                },
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: sig,
                children: vec![],
            };
            if current_impl.is_some() {
                impl_methods.push(sym);
            } else {
                symbols.push(sym);
            }
        }

        // Structs
        if trimmed.starts_with("pub struct ") || trimmed.starts_with("struct ") {
            symbols.push(CodeSymbol {
                name: extract_word_after(trimmed, "struct"),
                kind: SymbolKind::Struct,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }

        // Enums
        if trimmed.starts_with("pub enum ") || trimmed.starts_with("enum ") {
            symbols.push(CodeSymbol {
                name: extract_word_after(trimmed, "enum"),
                kind: SymbolKind::Enum,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }

        // Traits
        if trimmed.starts_with("pub trait ") || trimmed.starts_with("trait ") {
            symbols.push(CodeSymbol {
                name: extract_word_after(trimmed, "trait"),
                kind: SymbolKind::Trait,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }

        // Modules
        if (trimmed.starts_with("pub mod ") || trimmed.starts_with("mod "))
            && !trimmed.contains("//")
        {
            symbols.push(CodeSymbol {
                name: extract_word_after(trimmed, "mod")
                    .trim_end_matches(';')
                    .to_string(),
                kind: SymbolKind::Module,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }
    }

    // Don't forget the last impl block
    if let Some(impl_name) = current_impl {
        if !impl_methods.is_empty() {
            symbols.push(CodeSymbol {
                name: impl_name,
                kind: SymbolKind::Struct,
                start_line: 0,
                end_line: 0,
                signature: String::new(),
                children: impl_methods,
            });
        }
    }
}

fn extract_python_symbols(source: &str, symbols: &mut Vec<CodeSymbol>) {
    let mut current_class: Option<String> = None;
    let mut class_methods: Vec<CodeSymbol> = Vec::new();
    let mut class_indent: usize = 0;

    for (line_num, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();

        // Class definitions
        if trimmed.starts_with("class ") {
            // Save previous class
            if let Some(ref class_name) = current_class {
                symbols.push(CodeSymbol {
                    name: class_name.clone(),
                    kind: SymbolKind::Class,
                    start_line: line_num + 1,
                    end_line: line_num + 1,
                    signature: String::new(),
                    children: std::mem::take(&mut class_methods),
                });
            }
            current_class = Some(
                extract_word_after(trimmed, "class")
                    .trim_end_matches(':')
                    .trim_end_matches('(')
                    .to_string(),
            );
            class_indent = indent;
            continue;
        }

        // Functions/methods
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let name = extract_word_after(trimmed, "def");
            let name = name.trim_end_matches('(').trim_end_matches(':').to_string();

            let sig = trimmed.trim_end_matches(':').to_string();

            let sym = CodeSymbol {
                name,
                kind: if current_class.is_some() && indent > class_indent {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                },
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: sig,
                children: vec![],
            };

            if current_class.is_some() && indent > class_indent {
                class_methods.push(sym);
            } else {
                // We left the class
                if let Some(ref class_name) = current_class {
                    symbols.push(CodeSymbol {
                        name: class_name.clone(),
                        kind: SymbolKind::Class,
                        start_line: line_num + 1,
                        end_line: line_num + 1,
                        signature: String::new(),
                        children: std::mem::take(&mut class_methods),
                    });
                    current_class = None;
                }
                symbols.push(sym);
            }
        }
    }

    // Last class
    if let Some(class_name) = current_class {
        symbols.push(CodeSymbol {
            name: class_name,
            kind: SymbolKind::Class,
            start_line: 0,
            end_line: 0,
            signature: String::new(),
            children: class_methods,
        });
    }
}

fn extract_js_symbols(source: &str, symbols: &mut Vec<CodeSymbol>) {
    for (line_num, line) in source.lines().enumerate() {
        let trimmed = line.trim();

        // function declarations
        if trimmed.starts_with("function ")
            || trimmed.starts_with("export function ")
            || trimmed.starts_with("export default function ")
            || trimmed.starts_with("async function ")
            || trimmed.starts_with("export async function ")
        {
            let name = extract_word_after(trimmed, "function")
                .trim_end_matches('(')
                .to_string();
            symbols.push(CodeSymbol {
                name,
                kind: SymbolKind::Function,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: trimmed.trim_end_matches('{').trim().to_string(),
                children: vec![],
            });
        }

        // Arrow functions as const
        if (trimmed.starts_with("const ") || trimmed.starts_with("export const "))
            && trimmed.contains("=>")
        {
            let name = extract_word_after(trimmed, "const")
                .trim_end_matches('=')
                .trim()
                .to_string();
            symbols.push(CodeSymbol {
                name,
                kind: SymbolKind::Function,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: trimmed.trim_end_matches('{').trim().to_string(),
                children: vec![],
            });
        }

        // class
        if trimmed.starts_with("class ") || trimmed.starts_with("export class ") {
            let name = extract_word_after(trimmed, "class")
                .trim_end_matches('{')
                .trim()
                .to_string();
            symbols.push(CodeSymbol {
                name,
                kind: SymbolKind::Class,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }

        // interface (TypeScript)
        if trimmed.starts_with("interface ") || trimmed.starts_with("export interface ") {
            let name = extract_word_after(trimmed, "interface")
                .trim_end_matches('{')
                .trim()
                .to_string();
            symbols.push(CodeSymbol {
                name,
                kind: SymbolKind::Interface,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }

        // type (TypeScript)
        if trimmed.starts_with("type ") || trimmed.starts_with("export type ") {
            let name = extract_word_after(trimmed, "type")
                .trim_end_matches('=')
                .trim()
                .to_string();
            symbols.push(CodeSymbol {
                name,
                kind: SymbolKind::Type,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }
    }
}

fn extract_go_symbols(source: &str, symbols: &mut Vec<CodeSymbol>) {
    for (line_num, line) in source.lines().enumerate() {
        let trimmed = line.trim();

        if trimmed.starts_with("func ") {
            let sig = trimmed.trim_end_matches('{').trim().to_string();
            let name = extract_word_after(trimmed, "func")
                .trim_start_matches('(')
                .to_string();
            symbols.push(CodeSymbol {
                name,
                kind: SymbolKind::Function,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: sig,
                children: vec![],
            });
        }

        if trimmed.starts_with("type ") && trimmed.contains("struct") {
            symbols.push(CodeSymbol {
                name: extract_word_after(trimmed, "type"),
                kind: SymbolKind::Struct,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }

        if trimmed.starts_with("type ") && trimmed.contains("interface") {
            symbols.push(CodeSymbol {
                name: extract_word_after(trimmed, "type"),
                kind: SymbolKind::Interface,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }
    }
}

fn extract_c_symbols(source: &str, symbols: &mut Vec<CodeSymbol>) {
    for (line_num, line) in source.lines().enumerate() {
        let trimmed = line.trim();

        // Function-like patterns in C/C++
        // Look for: type name(params) {
        if !trimmed.starts_with("//")
            && !trimmed.starts_with("/*")
            && !trimmed.starts_with("*")
            && !trimmed.starts_with("#")
            && trimmed.contains('(')
            && !trimmed.contains(';')
            && (trimmed.ends_with('{') || trimmed.ends_with(')'))
        {
            if let Some(paren_pos) = trimmed.find('(') {
                let before_paren = &trimmed[..paren_pos];
                let parts: Vec<&str> = before_paren.split_whitespace().collect();
                if parts.len() >= 2 {
                    let name = parts.last().unwrap().trim_start_matches('*').to_string();
                    symbols.push(CodeSymbol {
                        name,
                        kind: SymbolKind::Function,
                        start_line: line_num + 1,
                        end_line: line_num + 1,
                        signature: trimmed.trim_end_matches('{').trim().to_string(),
                        children: vec![],
                    });
                }
            }
        }

        // struct
        if trimmed.starts_with("struct ") || trimmed.starts_with("typedef struct") {
            let name = extract_word_after(trimmed, "struct")
                .trim_end_matches('{')
                .trim()
                .to_string();
            if !name.is_empty() {
                symbols.push(CodeSymbol {
                    name,
                    kind: SymbolKind::Struct,
                    start_line: line_num + 1,
                    end_line: line_num + 1,
                    signature: String::new(),
                    children: vec![],
                });
            }
        }

        // class (C++)
        if trimmed.starts_with("class ") {
            let name = extract_word_after(trimmed, "class")
                .trim_end_matches('{')
                .trim_end_matches(':')
                .trim()
                .to_string();
            symbols.push(CodeSymbol {
                name,
                kind: SymbolKind::Class,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: String::new(),
                children: vec![],
            });
        }
    }
}

fn extract_java_symbols(source: &str, symbols: &mut Vec<CodeSymbol>) {
    for (line_num, line) in source.lines().enumerate() {
        let trimmed = line.trim();

        if (trimmed.contains("class ") || trimmed.contains("interface "))
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("*")
        {
            if trimmed.contains("class ") {
                symbols.push(CodeSymbol {
                    name: extract_word_after(trimmed, "class")
                        .trim_end_matches('{')
                        .trim()
                        .to_string(),
                    kind: SymbolKind::Class,
                    start_line: line_num + 1,
                    end_line: line_num + 1,
                    signature: String::new(),
                    children: vec![],
                });
            }
            if trimmed.contains("interface ") {
                symbols.push(CodeSymbol {
                    name: extract_word_after(trimmed, "interface")
                        .trim_end_matches('{')
                        .trim()
                        .to_string(),
                    kind: SymbolKind::Interface,
                    start_line: line_num + 1,
                    end_line: line_num + 1,
                    signature: String::new(),
                    children: vec![],
                });
            }
        }
    }
}

fn extract_generic_symbols(source: &str, symbols: &mut Vec<CodeSymbol>) {
    // Very generic: just look for function-like patterns
    for (line_num, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("function ")
            || trimmed.starts_with("def ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("func ")
            || trimmed.starts_with("sub ")
        {
            symbols.push(CodeSymbol {
                name: trimmed
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("unknown")
                    .trim_end_matches('(')
                    .to_string(),
                kind: SymbolKind::Function,
                start_line: line_num + 1,
                end_line: line_num + 1,
                signature: trimmed.to_string(),
                children: vec![],
            });
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────

fn extract_fn_name(line: &str) -> String {
    let after_fn = line.split("fn ").nth(1).unwrap_or("");
    after_fn
        .split('(')
        .next()
        .unwrap_or("")
        .split('<')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

fn extract_impl_name(line: &str) -> Option<String> {
    // Handle: impl Foo, impl<T> Foo<T>, impl Trait for Foo
    let after_impl = line.split("impl").nth(1)?;
    let cleaned = after_impl.trim();
    // Skip generic params
    let cleaned = if cleaned.starts_with('<') {
        // Find closing >
        cleaned.split('>').nth(1).unwrap_or(cleaned).trim()
    } else {
        cleaned
    };

    // Handle "Trait for Type"
    if cleaned.contains(" for ") {
        cleaned.split(" for ").nth(1)?.split('{').next()
    } else {
        cleaned.split('{').next()
    }
    .map(|s| s.split('<').next().unwrap_or(s).trim().to_string())
    .filter(|s| !s.is_empty())
}

fn extract_word_after(line: &str, keyword: &str) -> String {
    line.split(keyword)
        .nth(1)
        .unwrap_or("")
        .trim()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_function_extraction() {
        let src = r#"
pub fn hello_world() -> String {
    "hello".to_string()
}

fn private_fn(x: i32) -> i32 {
    x + 1
}
"#;
        let symbols = extract_symbols_generic(src, "rs");
        assert!(symbols.iter().any(|s| s.name == "hello_world"));
        assert!(symbols.iter().any(|s| s.name == "private_fn"));
    }

    #[test]
    fn test_python_class_extraction() {
        let src = r#"
class MyClass:
    def __init__(self):
        pass

    def method(self, x):
        return x
"#;
        let symbols = extract_symbols_generic(src, "py");
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_repo_map_generation() {
        let src = "pub fn foo() {}\npub struct Bar {}\n";
        let symbols = extract_symbols_generic(src, "rs");
        let map = symbols_to_repo_map(Path::new("src/main.rs"), &symbols);
        assert!(map.contains("foo"));
        assert!(map.contains("Bar"));
    }
}
