use regex::Regex;

#[derive(Debug, Clone)]
pub struct CodeAnalysis {
    pub issues: Vec<Issue>,
    pub suggestions: Vec<String>,
    pub metrics: CodeMetrics,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub line: usize,
    pub column: usize,
    pub severity: Severity,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct CodeMetrics {
    pub lines_of_code: usize,
    pub complexity_score: f32,
    pub function_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    Other,
}

impl Language {
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Self::Rust,
            "py" => Self::Python,
            "js" | "jsx" => Self::JavaScript,
            "ts" | "tsx" => Self::TypeScript,
            "go" => Self::Go,
            _ => Self::Other,
        }
    }
}

pub struct Linter;

impl Linter {
    pub fn analyze(code: &str, language: Language) -> CodeAnalysis {
        let mut issues = Vec::new();
        let mut suggestions = Vec::new();

        // Language-specific analysis
        match language {
            Language::Rust => Self::analyze_rust(code, &mut issues, &mut suggestions),
            Language::Python => Self::analyze_python(code, &mut issues),
            Language::JavaScript | Language::TypeScript => {
                Self::analyze_javascript(code, &mut issues)
            }
            _ => {}
        }

        // Generic analysis for all languages
        Self::analyze_generic(code, &mut issues);

        let metrics = CodeMetrics {
            lines_of_code: code.lines().filter(|l| !l.trim().is_empty()).count(),
            complexity_score: Self::calculate_complexity(code),
            function_count: Self::count_functions(code),
        };

        CodeAnalysis {
            issues,
            suggestions,
            metrics,
        }
    }

    fn analyze_rust(code: &str, issues: &mut Vec<Issue>, suggestions: &mut Vec<String>) {
        let unwrap_re = Regex::new(r"\.unwrap\(\)").unwrap();
        let clone_re = Regex::new(r"\.clone\(\)").unwrap();

        for (line_num, line) in code.lines().enumerate() {
            if unwrap_re.is_match(line) {
                issues.push(Issue {
                    line: line_num + 1,
                    column: line.find(".unwrap").unwrap_or(0),
                    severity: Severity::Warning,
                    message: "Direct unwrap() call without error handling".into(),
                    suggestion: Some("Use ? operator, unwrap_or(), or unwrap_or_else()".into()),
                });
            }

            if clone_re.is_match(line) && !line.contains('&') {
                issues.push(Issue {
                    line: line_num + 1,
                    column: line.find(".clone").unwrap_or(0),
                    severity: Severity::Info,
                    message: "Consider using reference instead of clone()".into(),
                    suggestion: Some("Use & instead of .clone() to avoid allocations".into()),
                });
            }
        }

        suggestions.push("Consider adding documentation comments for public functions".into());
    }

    fn analyze_python(code: &str, issues: &mut Vec<Issue>) {
        let bare_except_re = Regex::new(r"except\s*:").unwrap();

        for (line_num, line) in code.lines().enumerate() {
            if bare_except_re.is_match(line) {
                issues.push(Issue {
                    line: line_num + 1,
                    column: line.find("except").unwrap_or(0),
                    severity: Severity::Warning,
                    message: "Bare except clause catches all exceptions".into(),
                    suggestion: Some("Specify the exception types to catch".into()),
                });
            }
        }
    }

    fn analyze_javascript(code: &str, issues: &mut Vec<Issue>) {
        let var_re = Regex::new(r"\bvar\s+").unwrap();

        for (line_num, line) in code.lines().enumerate() {
            if var_re.is_match(line) {
                issues.push(Issue {
                    line: line_num + 1,
                    column: line.find("var").unwrap_or(0),
                    severity: Severity::Warning,
                    message: "Using 'var' instead of 'let' or 'const'".into(),
                    suggestion: Some(
                        "Use 'let' for reassigned variables, 'const' for constants".into(),
                    ),
                });
            }
        }
    }

    fn analyze_generic(code: &str, issues: &mut Vec<Issue>) {
        let todo_re = Regex::new(r"(?i)\b(TODO|FIXME|HACK)\b").unwrap();

        for (line_num, line) in code.lines().enumerate() {
            if todo_re.is_match(line) {
                issues.push(Issue {
                    line: line_num + 1,
                    column: todo_re.find(line).map(|m| m.start()).unwrap_or(0),
                    severity: Severity::Info,
                    message: "TODO/FIXME comment found".into(),
                    suggestion: None,
                });
            }

            if line.len() > 120 {
                issues.push(Issue {
                    line: line_num + 1,
                    column: 120,
                    severity: Severity::Info,
                    message: format!("Line is too long ({} characters)", line.len()),
                    suggestion: Some("Break long lines for readability".into()),
                });
            }
        }
    }

    fn calculate_complexity(code: &str) -> f32 {
        let mut complexity = 1.0f32;
        let keywords = ["if ", "else ", "for ", "while ", "match ", "&&", "||"];
        for line in code.lines() {
            for kw in &keywords {
                if line.contains(kw) {
                    complexity += 1.0;
                }
            }
        }
        complexity.min(10.0)
    }

    fn count_functions(code: &str) -> usize {
        code.matches("fn ").count()
            + code.matches("function ").count()
            + code.matches("def ").count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_unwrap_detection() {
        let code = "let x = some_option.unwrap();";
        let analysis = Linter::analyze(code, Language::Rust);
        assert!(!analysis.issues.is_empty());
        assert_eq!(analysis.issues[0].severity, Severity::Warning);
    }

    #[test]
    fn test_python_bare_except() {
        let code = "try:\n    pass\nexcept:\n    pass";
        let analysis = Linter::analyze(code, Language::Python);
        assert!(analysis
            .issues
            .iter()
            .any(|i| i.message.contains("Bare except")));
    }

    #[test]
    fn test_metrics() {
        let code = "fn foo() {\n    if true {\n        bar()\n    }\n}\n";
        let analysis = Linter::analyze(code, Language::Rust);
        assert_eq!(analysis.metrics.function_count, 1);
        assert!(analysis.metrics.complexity_score > 1.0);
    }
}
