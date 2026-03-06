/// Repo Map Generator — tree-sitter based project symbol extraction.
///
/// Gives the AI a bird's-eye view of the entire codebase by extracting
/// function signatures, struct/enum definitions, trait impls, and module
/// structure. Inspired by Aider's repo map but implemented in Rust using
/// tree-sitter for zero-dependency parsing.
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A single symbol extracted from a source file
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Const,
    TypeAlias,
    Macro,
    Module,
    Class,
    Interface,
    Method,
}

impl SymbolKind {
    fn icon(&self) -> &'static str {
        match self {
            SymbolKind::Function | SymbolKind::Method => "fn",
            SymbolKind::Struct | SymbolKind::Class => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait | SymbolKind::Interface => "trait",
            SymbolKind::Impl => "impl",
            SymbolKind::Const => "const",
            SymbolKind::TypeAlias => "type",
            SymbolKind::Macro => "macro",
            SymbolKind::Module => "mod",
        }
    }
}

/// File-level symbol summary
#[derive(Debug, Clone)]
pub struct FileSymbols {
    pub path: PathBuf,
    pub symbols: Vec<Symbol>,
}

/// Generates a text-based repo map from the project's source files.
pub struct RepoMapGenerator {
    root: PathBuf,
    max_files: usize,
    max_tokens: usize,
}

impl RepoMapGenerator {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_files: 500,
            max_tokens: 4096,
        }
    }

    pub fn with_max_files(mut self, max: usize) -> Self {
        self.max_files = max;
        self
    }

    pub fn with_max_tokens(mut self, max: usize) -> Self {
        self.max_tokens = max;
        self
    }

    /// Generate the full repo map as a string suitable for LLM context.
    pub fn generate(&self) -> String {
        let file_symbols = self.collect_all_symbols();

        if file_symbols.is_empty() {
            return String::from("(empty project — no source files found)");
        }

        self.format_map(&file_symbols)
    }

    /// Collect symbols from all source files in the project.
    fn collect_all_symbols(&self) -> Vec<FileSymbols> {
        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .max_depth(Some(15))
            .build();

        let mut results = Vec::new();
        let mut file_count = 0;

        for entry in walker.flatten() {
            if file_count >= self.max_files {
                break;
            }

            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(ext) => ext,
                None => continue,
            };

            // Only process supported languages
            if !matches!(
                ext,
                "rs" | "py"
                    | "js"
                    | "jsx"
                    | "ts"
                    | "tsx"
                    | "go"
                    | "c"
                    | "h"
                    | "cpp"
                    | "hpp"
                    | "java"
                    | "rb"
                    | "lua"
            ) {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Skip very small files (likely auto-generated or empty)
            if content.len() < 20 {
                continue;
            }

            let symbols = extract_symbols_regex(&content, ext);

            if !symbols.is_empty() {
                let rel_path = path.strip_prefix(&self.root).unwrap_or(path).to_path_buf();
                results.push(FileSymbols {
                    path: rel_path,
                    symbols,
                });
                file_count += 1;
            }
        }

        // Sort by path for consistent output
        results.sort_by(|a, b| a.path.cmp(&b.path));
        results
    }

    /// Format the collected symbols into a compact text map.
    fn format_map(&self, file_symbols: &[FileSymbols]) -> String {
        let mut output = String::new();
        let mut total_chars = 0;
        let char_budget = self.max_tokens * 4; // rough chars-per-token estimate

        // Group by directory for better readability
        let mut by_dir: BTreeMap<String, Vec<&FileSymbols>> = BTreeMap::new();
        for fs in file_symbols {
            let dir = fs
                .path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            by_dir.entry(dir).or_default().push(fs);
        }

        for (dir, files) in &by_dir {
            if total_chars >= char_budget {
                output.push_str("\n... (truncated — repo too large for context window)\n");
                break;
            }

            if !dir.is_empty() {
                output.push_str(&format!("\n📁 {dir}/\n"));
            }

            for fs in files {
                if total_chars >= char_budget {
                    break;
                }

                let filename = fs
                    .path
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();
                output.push_str(&format!("  📄 {filename}\n"));
                total_chars += filename.len() + 6;

                for sym in &fs.symbols {
                    let line = format!(
                        "    {} {} (L{})\n",
                        sym.kind.icon(),
                        sym.signature,
                        sym.line
                    );
                    total_chars += line.len();
                    if total_chars >= char_budget {
                        output.push_str("    ... (truncated)\n");
                        break;
                    }
                    output.push_str(&line);
                }
            }
        }

        output
    }
}

/// Extract symbols from source code using regex patterns.
/// This is language-aware and handles Rust, Python, JS/TS, Go, C/C++, Java, Ruby.
fn extract_symbols_regex(content: &str, extension: &str) -> Vec<Symbol> {
    match extension {
        "rs" => extract_rust_symbols(content),
        "py" => extract_python_symbols(content),
        "js" | "jsx" | "ts" | "tsx" => extract_js_symbols(content),
        "go" => extract_go_symbols(content),
        "c" | "h" | "cpp" | "hpp" => extract_c_symbols(content),
        "java" => extract_java_symbols(content),
        "rb" => extract_ruby_symbols(content),
        _ => Vec::new(),
    }
}

fn extract_rust_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.is_empty() {
            continue;
        }

        // pub fn / fn
        if let Some((fn_name, sig)) = extract_rust_fn(trimmed) {
            let kind = if trimmed.contains("fn ") && content[..content.lines().take(line_num).map(|l| l.len() + 1).sum::<usize>().saturating_sub(1)].lines().rev().take(5).any(|l| l.trim().starts_with("impl ")) {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            symbols.push(Symbol {
                name: fn_name,
                kind,
                line: line_num + 1,
                signature: sig,
            });
        }
        // pub struct / struct
        else if (trimmed.starts_with("pub struct ") || trimmed.starts_with("struct "))
            && !trimmed.contains("//")
        {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "pub" && *w != "struct")
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Struct,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
        // pub enum / enum
        else if (trimmed.starts_with("pub enum ") || trimmed.starts_with("enum "))
            && !trimmed.contains("//")
        {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "pub" && *w != "enum")
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Enum,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
        // pub trait / trait
        else if (trimmed.starts_with("pub trait ") || trimmed.starts_with("trait "))
            && !trimmed.contains("//")
        {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "pub" && *w != "trait")
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Trait,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
        // impl
        else if trimmed.starts_with("impl ") && !trimmed.starts_with("impl<") || trimmed.starts_with("impl<") {
            let sig = trimmed
                .trim_end_matches('{')
                .trim_end()
                .to_string();
            // Extract the type being implemented
            let name = sig
                .replace("impl ", "")
                .split(|c: char| c == '<' || c == ' ')
                .next()
                .unwrap_or("")
                .to_string();
            if !name.is_empty() && name != "impl" {
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Impl,
                    line: line_num + 1,
                    signature: sig,
                });
            }
        }
        // pub const / const
        else if (trimmed.starts_with("pub const ") || trimmed.starts_with("const "))
            && trimmed.contains(':')
        {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "pub" && *w != "const")
                .unwrap_or("")
                .trim_end_matches(':')
                .to_string();
            if !name.is_empty() && name.chars().all(|c| c.is_uppercase() || c == '_') {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Const,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
        // pub type / type
        else if (trimmed.starts_with("pub type ") || trimmed.starts_with("type "))
            && trimmed.contains('=')
        {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "pub" && *w != "type")
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::TypeAlias,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
        // macro_rules!
        else if trimmed.starts_with("macro_rules!") {
            let name = trimmed
                .strip_prefix("macro_rules!")
                .unwrap_or("")
                .trim()
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Macro,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
        // pub mod / mod
        else if (trimmed.starts_with("pub mod ") || trimmed.starts_with("mod "))
            && trimmed.ends_with(';')
        {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "pub" && *w != "mod")
                .unwrap_or("")
                .trim_end_matches(';')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Module,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
    }

    symbols
}

fn extract_rust_fn(line: &str) -> Option<(String, String)> {
    // Match: pub fn name(, pub async fn name(, fn name(, async fn name(, pub(crate) fn name(
    let fn_idx = line.find("fn ")?;
    let after_fn = &line[fn_idx + 3..];
    let paren_idx = after_fn.find('(')?;
    let name = after_fn[..paren_idx].trim();
    if name.is_empty() || name.contains(' ') {
        return None;
    }

    // Build the signature: name(params) -> return
    let rest = &line[fn_idx..];
    // Truncate at the opening brace or where clause
    let sig = rest
        .split('{')
        .next()
        .unwrap_or(rest)
        .split("where")
        .next()
        .unwrap_or(rest)
        .trim()
        .to_string();

    Some((name.to_string(), sig))
}

fn extract_python_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("def ") {
            if let Some(paren) = trimmed.find('(') {
                let name = trimmed[4..paren].trim().to_string();
                let sig = trimmed.trim_end_matches(':').to_string();
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_num + 1,
                    signature: sig,
                });
            }
        } else if trimmed.starts_with("class ") {
            let name = trimmed[6..]
                .split(|c: char| c == '(' || c == ':')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Class,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
    }
    symbols
}

fn extract_js_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // function declarations
        if trimmed.starts_with("function ")
            || trimmed.starts_with("export function ")
            || trimmed.starts_with("async function ")
            || trimmed.starts_with("export async function ")
            || trimmed.starts_with("export default function ")
        {
            if let Some(paren) = trimmed.find('(') {
                let before_paren = &trimmed[..paren];
                let name = before_paren
                    .split_whitespace()
                    .last()
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty() && name != "function" {
                    symbols.push(Symbol {
                        name,
                        kind: SymbolKind::Function,
                        line: line_num + 1,
                        signature: trimmed
                            .split('{')
                            .next()
                            .unwrap_or(trimmed)
                            .trim()
                            .to_string(),
                    });
                }
            }
        }
        // class declarations
        else if trimmed.starts_with("class ") || trimmed.starts_with("export class ") {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "export" && *w != "class" && *w != "default")
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Class,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
        // interface/type declarations (TypeScript)
        else if trimmed.starts_with("interface ")
            || trimmed.starts_with("export interface ")
            || (trimmed.starts_with("type ") && trimmed.contains('='))
            || (trimmed.starts_with("export type ") && trimmed.contains('='))
        {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "export" && *w != "interface" && *w != "type")
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Interface,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
        // const arrow functions: const name = (
        else if (trimmed.starts_with("const ") || trimmed.starts_with("export const "))
            && (trimmed.contains("= (") || trimmed.contains("= async (") || trimmed.contains("=>"))
        {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "export" && *w != "const")
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_num + 1,
                    signature: trimmed
                        .split("=>")
                        .next()
                        .unwrap_or(trimmed)
                        .trim()
                        .to_string(),
                });
            }
        }
    }
    symbols
}

fn extract_go_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("func ") {
            let sig = trimmed.split('{').next().unwrap_or(trimmed).trim().to_string();
            let name = if trimmed.contains("(") && trimmed[5..].starts_with('(') {
                // Method: func (r *Receiver) Name(
                let after_close = trimmed.find(") ").map(|i| &trimmed[i + 2..]).unwrap_or("");
                after_close
                    .split('(')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            } else {
                trimmed[5..]
                    .split('(')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };
            if !name.is_empty() {
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_num + 1,
                    signature: sig,
                });
            }
        } else if trimmed.starts_with("type ") && trimmed.contains("struct") {
            let name = trimmed
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Struct,
                    line: line_num + 1,
                    signature: name,
                });
            }
        } else if trimmed.starts_with("type ") && trimmed.contains("interface") {
            let name = trimmed
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Interface,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
    }
    symbols
}

fn extract_c_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        // Function declarations (heuristic: type name(
        if trimmed.contains('(')
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("/*")
            && !trimmed.starts_with('#')
            && !trimmed.starts_with("if ")
            && !trimmed.starts_with("for ")
            && !trimmed.starts_with("while ")
            && !trimmed.starts_with("switch ")
            && !trimmed.starts_with("return ")
        {
            let paren_idx = match trimmed.find('(') {
                Some(i) => i,
                None => continue,
            };
            let before_paren = trimmed[..paren_idx].trim();
            let parts: Vec<&str> = before_paren.split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts.last().unwrap_or(&"").trim_start_matches('*').to_string();
                if !name.is_empty()
                    && name.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false)
                {
                    symbols.push(Symbol {
                        name,
                        kind: SymbolKind::Function,
                        line: line_num + 1,
                        signature: trimmed
                            .split('{')
                            .next()
                            .unwrap_or(trimmed)
                            .trim()
                            .to_string(),
                    });
                }
            }
        }
        // struct
        else if trimmed.starts_with("struct ") || trimmed.starts_with("typedef struct") {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "struct" && *w != "typedef")
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Struct,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
    }
    symbols
}

fn extract_java_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if (trimmed.contains("class ") && trimmed.contains('{'))
            || trimmed.starts_with("public class ")
            || trimmed.starts_with("class ")
        {
            let name = trimmed
                .split_whitespace()
                .find(|w| {
                    *w != "public"
                        && *w != "private"
                        && *w != "protected"
                        && *w != "abstract"
                        && *w != "final"
                        && *w != "static"
                        && *w != "class"
                })
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Class,
                    line: line_num + 1,
                    signature: name,
                });
            }
        } else if trimmed.contains("interface ") {
            let name = trimmed
                .split_whitespace()
                .find(|w| *w != "public" && *w != "interface")
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Interface,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
    }
    symbols
}

fn extract_ruby_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("def ") {
            let name = trimmed[4..]
                .split(|c: char| c == '(' || c == ' ')
                .next()
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_num + 1,
                    signature: trimmed.to_string(),
                });
            }
        } else if trimmed.starts_with("class ") {
            let name = trimmed[6..]
                .split(|c: char| c == '<' || c == ' ')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Class,
                    line: line_num + 1,
                    signature: name,
                });
            }
        } else if trimmed.starts_with("module ") {
            let name = trimmed[7..].trim().to_string();
            if !name.is_empty() {
                symbols.push(Symbol {
                    name: name.clone(),
                    kind: SymbolKind::Module,
                    line: line_num + 1,
                    signature: name,
                });
            }
        }
    }
    symbols
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rust_symbols() {
        let code = r#"
pub struct MyStruct {
    field: u32,
}

pub enum MyEnum {
    A,
    B,
}

pub trait MyTrait {
    fn do_thing(&self);
}

impl MyStruct {
    pub fn new() -> Self {
        Self { field: 0 }
    }

    pub async fn process(&self, input: &str) -> Result<String, Error> {
        Ok(input.to_string())
    }
}

pub fn standalone_function(x: i32, y: i32) -> i32 {
    x + y
}

const MAX_ITEMS: usize = 100;
"#;
        let symbols = extract_rust_symbols(code);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MyStruct"));
        assert!(names.contains(&"MyEnum"));
        assert!(names.contains(&"MyTrait"));
        assert!(names.contains(&"standalone_function"));
        assert!(names.contains(&"MAX_ITEMS"));
    }

    #[test]
    fn test_extract_python_symbols() {
        let code = r#"
class UserManager:
    def __init__(self):
        pass

    def create_user(self, name):
        pass

def standalone():
    pass
"#;
        let symbols = extract_python_symbols(code);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserManager"));
        assert!(names.contains(&"create_user"));
        assert!(names.contains(&"standalone"));
    }
}
