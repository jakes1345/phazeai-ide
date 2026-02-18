mod linter;
pub mod outline;

pub use linter::{CodeAnalysis, CodeMetrics, Issue, Severity, Linter};
pub use outline::{CodeSymbol, SymbolKind, extract_symbols_generic, symbols_to_repo_map, generate_repo_map};
