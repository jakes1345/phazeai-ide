mod linter;
pub mod outline;

pub use linter::{CodeAnalysis, CodeMetrics, Issue, Linter, Severity};
pub use outline::{
    extract_symbols_generic, generate_repo_map, symbols_to_repo_map, CodeSymbol, SymbolKind,
};
