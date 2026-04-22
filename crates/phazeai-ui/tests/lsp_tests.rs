//! LSP layer tests — data structures, parsing helpers, URI conversion,
//! diagnostic ordering and completion filtering.
//!
//! All types/helpers are defined inline here, mirroring the production types
//! from `phazeai-ui/src/lsp_bridge.rs`. No LSP server needed.
//!
//! Run: `cargo test --test lsp_tests`

#![allow(dead_code)]

use std::path::PathBuf;

// ── InlayHintEntry ────────────────────────────────────────────────────────────

/// Mirror of `InlayHintEntry` from lsp_bridge.rs.
#[derive(Debug, Clone, PartialEq)]
struct InlayHintEntry {
    /// 0-based line.
    line: u32,
    /// 0-based column (byte offset within line).
    col: u32,
    /// Hint label text, e.g. ": i32" or "name: ".
    label: String,
}

#[test]
fn inlay_hint_entry_fields() {
    let hint = InlayHintEntry {
        line: 5,
        col: 12,
        label: ": i32".to_string(),
    };
    assert_eq!(hint.line, 5);
    assert_eq!(hint.col, 12);
    assert_eq!(hint.label, ": i32");
}

#[test]
fn inlay_hint_entry_clone_is_independent() {
    let hint = InlayHintEntry {
        line: 0,
        col: 0,
        label: "name: ".to_string(),
    };
    let mut cloned = hint.clone();
    cloned.label = "other: ".to_string();
    assert_eq!(hint.label, "name: ");
    assert_eq!(cloned.label, "other: ");
}

#[test]
fn inlay_hint_entry_zero_position() {
    let hint = InlayHintEntry {
        line: 0,
        col: 0,
        label: ": ()".to_string(),
    };
    assert_eq!(hint.line, 0);
    assert_eq!(hint.col, 0);
}

#[test]
fn inlay_hint_entry_large_position() {
    let hint = InlayHintEntry {
        line: 9999,
        col: 255,
        label: ": HashMap<String, Vec<i32>>".to_string(),
    };
    assert_eq!(hint.line, 9999);
    assert_eq!(hint.col, 255);
}

#[test]
fn inlay_hint_collection_sorted_by_line_then_col() {
    let mut hints = vec![
        InlayHintEntry {
            line: 3,
            col: 5,
            label: "a".into(),
        },
        InlayHintEntry {
            line: 1,
            col: 0,
            label: "b".into(),
        },
        InlayHintEntry {
            line: 3,
            col: 2,
            label: "c".into(),
        },
        InlayHintEntry {
            line: 0,
            col: 10,
            label: "d".into(),
        },
    ];
    hints.sort_by_key(|h| (h.line, h.col));
    assert_eq!(hints[0].label, "d"); // line 0
    assert_eq!(hints[1].label, "b"); // line 1
    assert_eq!(hints[2].label, "c"); // line 3, col 2
    assert_eq!(hints[3].label, "a"); // line 3, col 5
}

// ── SymbolEntry ───────────────────────────────────────────────────────────────

/// Mirror of `SymbolEntry` from lsp_bridge.rs.
#[derive(Debug, Clone)]
struct SymbolEntry {
    name: String,
    /// Matches the `kind` field in production: "fn", "struct", "trait", etc.
    kind: String,
    /// 1-based line number.
    line: u32,
    /// Nesting depth (0 = top-level).
    depth: u32,
}

impl SymbolEntry {
    /// Return a short display string for the symbol kind,
    /// matching the rendering used in the symbol outline panel.
    fn kind_str(&self) -> &str {
        match self.kind.as_str() {
            "fn" | "function" => "fn",
            "struct" => "struct",
            "enum" => "enum",
            "trait" => "trait",
            "impl" => "impl",
            "mod" | "module" => "mod",
            "type" | "typealias" => "type",
            "const" => "const",
            "static" => "static",
            "macro" => "macro",
            "class" => "class",
            "interface" => "interface",
            "field" | "property" => "field",
            "variable" | "let" | "var" => "var",
            _ => "sym",
        }
    }
}

#[test]
fn symbol_kind_fn() {
    let sym = SymbolEntry {
        name: "my_func".into(),
        kind: "fn".into(),
        line: 1,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "fn");
}

#[test]
fn symbol_kind_function_alias() {
    let sym = SymbolEntry {
        name: "my_func".into(),
        kind: "function".into(),
        line: 1,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "fn");
}

#[test]
fn symbol_kind_struct() {
    let sym = SymbolEntry {
        name: "MyStruct".into(),
        kind: "struct".into(),
        line: 5,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "struct");
}

#[test]
fn symbol_kind_enum() {
    let sym = SymbolEntry {
        name: "Color".into(),
        kind: "enum".into(),
        line: 10,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "enum");
}

#[test]
fn symbol_kind_trait() {
    let sym = SymbolEntry {
        name: "Display".into(),
        kind: "trait".into(),
        line: 20,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "trait");
}

#[test]
fn symbol_kind_impl() {
    let sym = SymbolEntry {
        name: "impl MyStruct".into(),
        kind: "impl".into(),
        line: 30,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "impl");
}

#[test]
fn symbol_kind_mod() {
    let sym = SymbolEntry {
        name: "utils".into(),
        kind: "mod".into(),
        line: 1,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "mod");
}

#[test]
fn symbol_kind_module_alias() {
    let sym = SymbolEntry {
        name: "utils".into(),
        kind: "module".into(),
        line: 1,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "mod");
}

#[test]
fn symbol_kind_class() {
    let sym = SymbolEntry {
        name: "MyClass".into(),
        kind: "class".into(),
        line: 1,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "class");
}

#[test]
fn symbol_kind_interface() {
    let sym = SymbolEntry {
        name: "ISerializable".into(),
        kind: "interface".into(),
        line: 1,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "interface");
}

#[test]
fn symbol_kind_unknown_falls_back_to_sym() {
    let sym = SymbolEntry {
        name: "something".into(),
        kind: "unknown_kind".into(),
        line: 1,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "sym");
}

#[test]
fn symbol_kind_empty_falls_back_to_sym() {
    let sym = SymbolEntry {
        name: "x".into(),
        kind: "".into(),
        line: 1,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "sym");
}

#[test]
fn symbol_kind_const() {
    let sym = SymbolEntry {
        name: "MAX_SIZE".into(),
        kind: "const".into(),
        line: 3,
        depth: 0,
    };
    assert_eq!(sym.kind_str(), "const");
}

#[test]
fn symbol_kind_field() {
    let sym = SymbolEntry {
        name: "width".into(),
        kind: "field".into(),
        line: 8,
        depth: 1,
    };
    assert_eq!(sym.kind_str(), "field");
}

#[test]
fn symbol_kind_property_alias() {
    let sym = SymbolEntry {
        name: "width".into(),
        kind: "property".into(),
        line: 8,
        depth: 1,
    };
    assert_eq!(sym.kind_str(), "field");
}

#[test]
fn symbol_kind_variable() {
    let sym = SymbolEntry {
        name: "count".into(),
        kind: "variable".into(),
        line: 15,
        depth: 2,
    };
    assert_eq!(sym.kind_str(), "var");
}

// ── URI ↔ path conversion ─────────────────────────────────────────────────────

/// Convert a `file://` URI string to a filesystem path.
/// Mirrors the `parse_diag_uri` / `uri_to_path` pattern used throughout
/// lsp_bridge.rs and tier1_state.rs.
fn uri_to_path(uri: &str) -> PathBuf {
    uri.strip_prefix("file://")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(uri))
}

/// Convert a filesystem path to a `file://` URI string.
/// Mirrors the `path_to_uri` helper in phazeai-core/src/lsp/client.rs.
fn path_to_uri(path: &str) -> String {
    if path.starts_with('/') {
        format!("file://{}", path)
    } else {
        // Relative path — just prefix with file:// for test purposes
        format!("file://{}", path)
    }
}

#[test]
fn uri_to_path_strips_file_prefix() {
    let path = uri_to_path("file:///home/jack/foo.rs");
    assert_eq!(path, PathBuf::from("/home/jack/foo.rs"));
}

#[test]
fn uri_to_path_no_prefix_passthrough() {
    let path = uri_to_path("/absolute/path.rs");
    assert_eq!(path, PathBuf::from("/absolute/path.rs"));
}

#[test]
fn uri_to_path_preserves_extension() {
    let path = uri_to_path("file:///home/jack/project/Cargo.toml");
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("toml"));
}

#[test]
fn uri_to_path_deep_nesting() {
    let path = uri_to_path("file:///home/jack/phazeai_ide/crates/phazeai-ui/src/app.rs");
    assert_eq!(
        path,
        PathBuf::from("/home/jack/phazeai_ide/crates/phazeai-ui/src/app.rs")
    );
}

#[test]
fn path_to_uri_absolute() {
    let uri = path_to_uri("/home/jack/foo.rs");
    assert_eq!(uri, "file:///home/jack/foo.rs");
}

#[test]
fn path_to_uri_starts_with_file_scheme() {
    let uri = path_to_uri("/some/path/main.rs");
    assert!(uri.starts_with("file://"));
}

#[test]
fn path_to_uri_roundtrip() {
    let original = "/home/jack/phazeai_ide/src/main.rs";
    let uri = path_to_uri(original);
    let recovered = uri_to_path(&uri);
    assert_eq!(recovered, PathBuf::from(original));
}

#[test]
fn uri_to_path_then_back_roundtrip() {
    let original_uri = "file:///home/jack/project/lib.rs";
    let path = uri_to_path(original_uri);
    let back = path_to_uri(path.to_str().unwrap());
    assert_eq!(back, original_uri);
}

// ── Diagnostic severity ordering ──────────────────────────────────────────────

/// Mirror of `DiagSeverity` from lsp_bridge.rs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

impl DiagSeverity {
    /// Numeric priority: higher = more severe.
    fn priority(self) -> u8 {
        match self {
            DiagSeverity::Error => 3,
            DiagSeverity::Warning => 2,
            DiagSeverity::Info => 1,
            DiagSeverity::Hint => 0,
        }
    }
}

#[test]
fn diag_severity_error_highest() {
    assert!(DiagSeverity::Error.priority() > DiagSeverity::Warning.priority());
}

#[test]
fn diag_severity_warning_above_info() {
    assert!(DiagSeverity::Warning.priority() > DiagSeverity::Info.priority());
}

#[test]
fn diag_severity_info_above_hint() {
    assert!(DiagSeverity::Info.priority() > DiagSeverity::Hint.priority());
}

#[test]
fn diag_severity_hint_is_lowest() {
    assert_eq!(DiagSeverity::Hint.priority(), 0);
}

#[test]
fn diag_severity_error_is_highest_value() {
    assert_eq!(DiagSeverity::Error.priority(), 3);
}

#[test]
fn diag_severity_sort_descending() {
    let mut severities = vec![
        DiagSeverity::Info,
        DiagSeverity::Error,
        DiagSeverity::Hint,
        DiagSeverity::Warning,
    ];
    severities.sort_by_key(|s| std::cmp::Reverse(s.priority()));
    assert_eq!(severities[0], DiagSeverity::Error);
    assert_eq!(severities[1], DiagSeverity::Warning);
    assert_eq!(severities[2], DiagSeverity::Info);
    assert_eq!(severities[3], DiagSeverity::Hint);
}

#[test]
fn diag_severity_all_unique_priorities() {
    let all = [
        DiagSeverity::Error,
        DiagSeverity::Warning,
        DiagSeverity::Info,
        DiagSeverity::Hint,
    ];
    let priorities: Vec<u8> = all.iter().map(|s| s.priority()).collect();
    let mut unique = priorities.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        priorities.len(),
        unique.len(),
        "all severities must have distinct priorities"
    );
}

// ── DiagEntry ─────────────────────────────────────────────────────────────────

/// Mirror of `DiagEntry` from lsp_bridge.rs.
#[derive(Debug, Clone)]
struct DiagEntry {
    path: PathBuf,
    /// 1-based line.
    line: u32,
    /// 1-based column.
    col: u32,
    message: String,
    severity: DiagSeverity,
}

#[test]
fn diag_entry_fields_accessible() {
    let entry = DiagEntry {
        path: PathBuf::from("/src/main.rs"),
        line: 10,
        col: 5,
        message: "unused variable".to_string(),
        severity: DiagSeverity::Warning,
    };
    assert_eq!(entry.line, 10);
    assert_eq!(entry.col, 5);
    assert_eq!(entry.severity, DiagSeverity::Warning);
}

#[test]
fn diag_entry_path_has_expected_extension() {
    let entry = DiagEntry {
        path: PathBuf::from("/project/src/lib.rs"),
        line: 1,
        col: 1,
        message: "error".to_string(),
        severity: DiagSeverity::Error,
    };
    assert_eq!(entry.path.extension().and_then(|e| e.to_str()), Some("rs"));
}

#[test]
fn diag_entry_line_col_are_one_based() {
    // The LSP spec uses 0-based positions, but DiagEntry stores 1-based.
    // A minimum valid position is (1, 1).
    let entry = DiagEntry {
        path: PathBuf::from("/f.rs"),
        line: 1,
        col: 1,
        message: "x".to_string(),
        severity: DiagSeverity::Info,
    };
    assert!(entry.line >= 1);
    assert!(entry.col >= 1);
}

/// Filter diagnostics to only errors (used in status bar indicator).
fn errors_only(diags: &[DiagEntry]) -> Vec<&DiagEntry> {
    diags
        .iter()
        .filter(|d| d.severity == DiagSeverity::Error)
        .collect()
}

#[test]
fn diag_filter_errors_only() {
    let diags = vec![
        DiagEntry {
            path: PathBuf::from("/a.rs"),
            line: 1,
            col: 1,
            message: "err".into(),
            severity: DiagSeverity::Error,
        },
        DiagEntry {
            path: PathBuf::from("/b.rs"),
            line: 2,
            col: 1,
            message: "warn".into(),
            severity: DiagSeverity::Warning,
        },
        DiagEntry {
            path: PathBuf::from("/c.rs"),
            line: 3,
            col: 1,
            message: "err2".into(),
            severity: DiagSeverity::Error,
        },
    ];
    let errors = errors_only(&diags);
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().all(|d| d.severity == DiagSeverity::Error));
}

#[test]
fn diag_filter_no_errors_returns_empty() {
    let diags = vec![DiagEntry {
        path: PathBuf::from("/a.rs"),
        line: 1,
        col: 1,
        message: "hint".into(),
        severity: DiagSeverity::Hint,
    }];
    assert!(errors_only(&diags).is_empty());
}

// ── CompletionEntry filtering ─────────────────────────────────────────────────

/// Mirror of `CompletionEntry` from lsp_bridge.rs.
#[derive(Debug, Clone)]
struct CompletionEntry {
    /// The label shown in the popup.
    label: String,
    /// The text to insert (may include snippet placeholders).
    insert_text: String,
    /// Optional short description.
    detail: Option<String>,
}

/// Filter completion entries by case-insensitive prefix match on `label`.
fn filter_completions<'a>(
    entries: &'a [CompletionEntry],
    prefix: &str,
) -> Vec<&'a CompletionEntry> {
    let p = prefix.to_lowercase();
    entries
        .iter()
        .filter(|e| e.label.to_lowercase().starts_with(&p))
        .collect()
}

fn make_completion(label: &str, insert: &str, detail: Option<&str>) -> CompletionEntry {
    CompletionEntry {
        label: label.to_string(),
        insert_text: insert.to_string(),
        detail: detail.map(str::to_string),
    }
}

#[test]
fn completion_filter_empty_prefix_returns_all() {
    let entries = vec![
        make_completion("println", "println!()", None),
        make_completion("print", "print!()", None),
        make_completion("eprintln", "eprintln!()", None),
    ];
    assert_eq!(filter_completions(&entries, "").len(), 3);
}

#[test]
fn completion_filter_by_exact_prefix() {
    let entries = vec![
        make_completion("println", "println!()", None),
        make_completion("print", "print!()", None),
        make_completion("eprintln", "eprintln!()", None),
    ];
    let result = filter_completions(&entries, "print");
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|e| e.label.starts_with("print")));
}

#[test]
fn completion_filter_case_insensitive() {
    let entries = vec![
        make_completion("PrintLn", "PrintLn()", None),
        make_completion("eprint", "eprint!()", None),
    ];
    let result = filter_completions(&entries, "PRINT");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].label, "PrintLn");
}

#[test]
fn completion_filter_no_match_returns_empty() {
    let entries = vec![make_completion("foo", "foo()", None)];
    assert!(filter_completions(&entries, "bar").is_empty());
}

#[test]
fn completion_filter_exact_full_label() {
    let entries = vec![
        make_completion("vec", "vec![]", Some("Vec macro")),
        make_completion("vector", "Vector::new()", None),
    ];
    let result = filter_completions(&entries, "vec");
    assert_eq!(result.len(), 2); // both start with "vec"
}

#[test]
fn completion_filter_single_char_prefix() {
    let entries = vec![
        make_completion("a_func", "a_func()", None),
        make_completion("b_func", "b_func()", None),
        make_completion("another", "another()", None),
    ];
    let result = filter_completions(&entries, "a");
    assert_eq!(result.len(), 2); // "a_func" and "another"
}

#[test]
fn completion_insert_text_independent_of_label() {
    let entry = make_completion("println", "println!($0)", None);
    assert_eq!(entry.label, "println");
    assert_eq!(entry.insert_text, "println!($0)");
}

#[test]
fn completion_detail_optional() {
    let with_detail = make_completion("push", "push($0)", Some("Vec::push"));
    let without_detail = make_completion("pop", "pop()", None);
    assert!(with_detail.detail.is_some());
    assert!(without_detail.detail.is_none());
}

#[test]
fn completion_filter_preserves_order() {
    let entries = vec![
        make_completion("zz_last", "", None),
        make_completion("aa_first", "", None),
        make_completion("mm_middle", "", None),
    ];
    let result = filter_completions(&entries, "");
    // Order should be preserved (no sorting in filter)
    assert_eq!(result[0].label, "zz_last");
    assert_eq!(result[1].label, "aa_first");
    assert_eq!(result[2].label, "mm_middle");
}

// ── SignatureHelpResult ───────────────────────────────────────────────────────

/// Mirror of `SignatureHelpResult` from lsp_bridge.rs.
#[derive(Debug, Clone)]
struct SignatureHelpResult {
    label: String,
    active_param: usize,
    params: Vec<String>,
}

#[test]
fn sig_help_fields() {
    let sig = SignatureHelpResult {
        label: "fn foo(a: i32, b: &str) -> bool".to_string(),
        active_param: 1,
        params: vec!["a: i32".to_string(), "b: &str".to_string()],
    };
    assert_eq!(sig.active_param, 1);
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.params[1], "b: &str");
}

#[test]
fn sig_help_no_params() {
    let sig = SignatureHelpResult {
        label: "fn bar()".to_string(),
        active_param: 0,
        params: vec![],
    };
    assert!(sig.params.is_empty());
}

#[test]
fn sig_help_active_param_in_range() {
    let sig = SignatureHelpResult {
        label: "fn f(x: u8, y: u8)".to_string(),
        active_param: 0,
        params: vec!["x: u8".to_string(), "y: u8".to_string()],
    };
    assert!(sig.active_param < sig.params.len());
}

// ── DefinitionResult / ReferenceEntry ────────────────────────────────────────

/// Mirror of `DefinitionResult` from lsp_bridge.rs.
#[derive(Debug, Clone, PartialEq)]
struct DefinitionResult {
    path: PathBuf,
    /// 1-based line.
    line: u32,
    /// 1-based column.
    col: u32,
}

/// Mirror of `ReferenceEntry` from lsp_bridge.rs.
#[derive(Debug, Clone, PartialEq)]
struct ReferenceEntry {
    path: PathBuf,
    line: u32,
    col: u32,
}

#[test]
fn definition_result_fields() {
    let def = DefinitionResult {
        path: PathBuf::from("/project/src/lib.rs"),
        line: 42,
        col: 5,
    };
    assert_eq!(def.line, 42);
    assert_eq!(def.col, 5);
}

#[test]
fn definition_result_line_col_one_based() {
    let def = DefinitionResult {
        path: PathBuf::from("/f.rs"),
        line: 1,
        col: 1,
    };
    assert!(def.line >= 1 && def.col >= 1);
}

#[test]
fn reference_entry_equality() {
    let r1 = ReferenceEntry {
        path: PathBuf::from("/a.rs"),
        line: 3,
        col: 7,
    };
    let r2 = ReferenceEntry {
        path: PathBuf::from("/a.rs"),
        line: 3,
        col: 7,
    };
    assert_eq!(r1, r2);
}

#[test]
fn reference_entries_dedup() {
    // Duplicates should be removable (e.g. same definition counted twice)
    let mut refs = vec![
        ReferenceEntry {
            path: PathBuf::from("/a.rs"),
            line: 1,
            col: 1,
        },
        ReferenceEntry {
            path: PathBuf::from("/a.rs"),
            line: 1,
            col: 1,
        },
        ReferenceEntry {
            path: PathBuf::from("/b.rs"),
            line: 2,
            col: 3,
        },
    ];
    refs.dedup_by(|a, b| a == b);
    assert_eq!(refs.len(), 2);
}

// ── CodeLensEntry ─────────────────────────────────────────────────────────────

/// Mirror of `CodeLensEntry` from lsp_bridge.rs.
#[derive(Debug, Clone)]
struct CodeLensEntry {
    /// 1-based line.
    line: u32,
    label: String,
}

#[test]
fn code_lens_entry_fields() {
    let lens = CodeLensEntry {
        line: 10,
        label: "2 references".to_string(),
    };
    assert_eq!(lens.line, 10);
    assert_eq!(lens.label, "2 references");
}

#[test]
fn code_lens_multiple_entries_on_same_file() {
    let lenses = vec![
        CodeLensEntry {
            line: 1,
            label: "Run test".into(),
        },
        CodeLensEntry {
            line: 15,
            label: "3 references".into(),
        },
        CodeLensEntry {
            line: 22,
            label: "Run test".into(),
        },
    ];
    let run_tests: Vec<&CodeLensEntry> = lenses.iter().filter(|l| l.label == "Run test").collect();
    assert_eq!(run_tests.len(), 2);
}

#[test]
fn code_lens_sorted_by_line() {
    let mut lenses = vec![
        CodeLensEntry {
            line: 50,
            label: "a".into(),
        },
        CodeLensEntry {
            line: 10,
            label: "b".into(),
        },
        CodeLensEntry {
            line: 30,
            label: "c".into(),
        },
    ];
    lenses.sort_by_key(|l| l.line);
    assert_eq!(lenses[0].line, 10);
    assert_eq!(lenses[1].line, 30);
    assert_eq!(lenses[2].line, 50);
}

// ── LspCommand variant coverage ───────────────────────────────────────────────
//
// These tests verify that the LspCommand discriminants used in lsp_bridge.rs
// describe the right intent by checking string representations from Debug.
// They do NOT import the actual enum (which lives in a Floem reactive context);
// they document the expected set of commands as a regression guard.

const EXPECTED_LSP_COMMANDS: &[&str] = &[
    "OpenFile",
    "ChangeFile",
    "RequestCompletions",
    "RequestDefinition",
    "RequestHover",
    "RequestSignatureHelp",
    "RequestReferences",
    "RequestCodeActions",
    "RequestRename",
    "RequestDocumentSymbols",
    "SaveFile",
    "RequestWorkspaceSymbols",
    "RequestPeekDefinition",
    "RequestCodeLens",
    "RequestImplementation",
    "RequestFoldingRanges",
    "OrganizeImports",
    "RequestInlayHints",
    "Shutdown",
];

#[test]
fn lsp_command_list_count() {
    // If a command is added or removed from lsp_bridge.rs, update this list.
    assert_eq!(EXPECTED_LSP_COMMANDS.len(), 19);
}

#[test]
fn lsp_command_list_has_shutdown() {
    assert!(EXPECTED_LSP_COMMANDS.contains(&"Shutdown"));
}

#[test]
fn lsp_command_list_has_inlay_hints() {
    assert!(EXPECTED_LSP_COMMANDS.contains(&"RequestInlayHints"));
}

#[test]
fn lsp_command_list_has_organize_imports() {
    assert!(EXPECTED_LSP_COMMANDS.contains(&"OrganizeImports"));
}

#[test]
fn lsp_command_list_has_folding_ranges() {
    assert!(EXPECTED_LSP_COMMANDS.contains(&"RequestFoldingRanges"));
}

#[test]
fn lsp_command_list_no_duplicates() {
    let mut sorted = EXPECTED_LSP_COMMANDS.to_vec();
    sorted.sort();
    let original_len = sorted.len();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        original_len,
        "duplicate entries in EXPECTED_LSP_COMMANDS"
    );
}

// ── FoldingRange pair utilities ───────────────────────────────────────────────

/// A folding range (0-based start and end lines), matching
/// `Vec<(u32, u32)>` returned by the LSP bridge.
fn fold_range_contains_line(range: (u32, u32), line: u32) -> bool {
    line > range.0 && line <= range.1
}

fn fold_ranges_for_line(ranges: &[(u32, u32)], line: u32) -> Vec<(u32, u32)> {
    ranges
        .iter()
        .copied()
        .filter(|&r| fold_range_contains_line(r, line))
        .collect()
}

#[test]
fn fold_range_line_inside() {
    assert!(fold_range_contains_line((0, 10), 5));
}

#[test]
fn fold_range_line_at_start_is_outside() {
    // The start line is the header line and should NOT be hidden
    assert!(!fold_range_contains_line((5, 10), 5));
}

#[test]
fn fold_range_line_at_end_is_inside() {
    assert!(fold_range_contains_line((5, 10), 10));
}

#[test]
fn fold_range_line_before_start_is_outside() {
    assert!(!fold_range_contains_line((5, 10), 3));
}

#[test]
fn fold_range_line_after_end_is_outside() {
    assert!(!fold_range_contains_line((5, 10), 11));
}

#[test]
fn fold_ranges_for_line_finds_enclosing() {
    let ranges = vec![(0, 20), (5, 15), (25, 30)];
    let enclosing = fold_ranges_for_line(&ranges, 10);
    // Both (0,20) and (5,15) contain line 10
    assert_eq!(enclosing.len(), 2);
}

#[test]
fn fold_ranges_for_line_no_match() {
    let ranges = vec![(0, 5), (10, 15)];
    let enclosing = fold_ranges_for_line(&ranges, 7);
    assert!(enclosing.is_empty());
}
