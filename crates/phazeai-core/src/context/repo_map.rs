/// Repo Map Generator — tree-sitter based project symbol extraction.
///
/// Gives the AI a bird's-eye view of the entire codebase by extracting
/// function signatures, struct/enum definitions, trait impls, and module
/// structure. Inspired by Aider's repo map but implemented in Rust using
/// tree-sitter for zero-dependency parsing.
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::analysis::{extract_symbols_generic, symbols_to_repo_map, CodeSymbol};

/// File-level symbol summary
#[derive(Debug, Clone)]
struct FileSymbols {
    path: PathBuf,
    symbols: Vec<CodeSymbol>,
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

            let symbols = extract_symbols_generic(&content, ext);

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

                let sym_map = symbols_to_repo_map(&fs.path, &fs.symbols);
                total_chars += sym_map.len();
                if total_chars >= char_budget {
                    output.push_str("    ... (truncated)\n");
                    break;
                }
                // Indent the symbol map lines (skip the first line which is the filename)
                for line in sym_map.lines().skip(1) {
                    output.push_str("  ");
                    output.push_str(line);
                    output.push('\n');
                }
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_symbols_uses_outline() {
        // Verify that extract_symbols_generic is used and returns symbols for Rust code
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

pub fn standalone_function(x: i32, y: i32) -> i32 {
    x + y
}
"#;
        let symbols = extract_symbols_generic(code, "rs");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MyStruct"));
        assert!(names.contains(&"MyEnum"));
        assert!(names.contains(&"MyTrait"));
        assert!(names.contains(&"standalone_function"));
    }

    #[test]
    fn test_collect_symbols_python() {
        let code = r#"
class UserManager:
    def __init__(self):
        pass

    def create_user(self, name):
        pass

def standalone():
    pass
"#;
        let symbols = extract_symbols_generic(code, "py");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserManager"));
        assert!(names.contains(&"standalone"));
    }
}
