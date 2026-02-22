use crate::themes::ThemeColors;
use egui::{self, RichText};

#[derive(Clone)]
pub struct OutlineSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub indent: usize,
}

#[derive(Clone, PartialEq)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Class,
    Constant,
    Type,
}

impl SymbolKind {
    pub fn icon(&self) -> &'static str {
        match self {
            SymbolKind::Function => "ƒ",
            SymbolKind::Struct => "◻",
            SymbolKind::Enum => "≡",
            SymbolKind::Trait => "τ",
            SymbolKind::Impl => "⊕",
            SymbolKind::Module => "◈",
            SymbolKind::Class => "⬡",
            SymbolKind::Constant => "π",
            SymbolKind::Type => "Τ",
        }
    }

    pub fn color(&self, theme: &ThemeColors) -> egui::Color32 {
        match self {
            SymbolKind::Function => egui::Color32::from_rgb(100, 180, 255),
            SymbolKind::Struct => egui::Color32::from_rgb(100, 220, 140),
            SymbolKind::Enum => egui::Color32::from_rgb(220, 160, 80),
            SymbolKind::Trait => egui::Color32::from_rgb(180, 120, 220),
            SymbolKind::Impl => egui::Color32::from_rgb(120, 200, 200),
            SymbolKind::Module => theme.text_secondary,
            SymbolKind::Class => egui::Color32::from_rgb(100, 220, 140),
            SymbolKind::Constant => egui::Color32::from_rgb(220, 100, 100),
            SymbolKind::Type => egui::Color32::from_rgb(180, 180, 80),
        }
    }
}

pub struct OutlinePanel {
    pub symbols: Vec<OutlineSymbol>,
    pub filter: String,
    /// Set by show() when user clicks a symbol; caller reads and clears it.
    pub jump_to_line: Option<usize>,
}

impl Default for OutlinePanel {
    fn default() -> Self {
        Self::new()
    }
}

impl OutlinePanel {
    pub fn new() -> Self {
        Self {
            symbols: Vec::new(),
            filter: String::new(),
            jump_to_line: None,
        }
    }

    /// Re-extract symbols from file content whenever the file changes.
    pub fn update(&mut self, content: &str, language: &str) {
        self.symbols = extract_symbols(content, language);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.vertical(|ui| {
            // Header
            ui.horizontal(|ui| {
                ui.colored_label(theme.text, RichText::new("Outline").strong());
            });
            ui.add_space(4.0);

            // Filter bar
            let filter_edit = egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter symbols…")
                .desired_width(ui.available_width())
                .frame(true);
            ui.add(filter_edit);
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);

            if self.symbols.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.colored_label(theme.text_muted, "No symbols found.");
                    ui.add_space(4.0);
                    ui.colored_label(
                        theme.text_muted,
                        RichText::new("Open a file to see its outline.").small(),
                    );
                });
                return;
            }

            let query = self.filter.to_lowercase();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for sym in &self.symbols {
                        if !query.is_empty() && !sym.name.to_lowercase().contains(&query) {
                            continue;
                        }

                        let indent_px = sym.indent as f32 * 12.0;
                        ui.horizontal(|ui| {
                            ui.add_space(indent_px);
                            let icon_color = sym.kind.color(theme);
                            ui.colored_label(icon_color, sym.kind.icon());
                            ui.add_space(4.0);
                            let label = egui::Label::new(
                                RichText::new(&sym.name).color(theme.text).size(13.0),
                            )
                            .sense(egui::Sense::click());
                            let resp = ui.add(label);
                            if resp.clicked() {
                                self.jump_to_line = Some(sym.line);
                            }
                            if resp.hovered() {
                                ui.colored_label(
                                    theme.text_muted,
                                    RichText::new(format!(":{}", sym.line + 1)).small(),
                                );
                            }
                        });
                    }
                });
        });
    }
}

/// Extract symbols from source code using pattern matching.
/// Returns a list of symbols in document order.
pub fn extract_symbols(content: &str, language: &str) -> Vec<OutlineSymbol> {
    match language {
        "rs" => extract_rust(content),
        "py" => extract_python(content),
        "js" | "ts" | "jsx" | "tsx" => extract_js_ts(content),
        "go" => extract_go(content),
        "c" | "cpp" | "h" | "hpp" => extract_c(content),
        _ => Vec::new(),
    }
}

fn extract_rust(content: &str) -> Vec<OutlineSymbol> {
    let mut symbols = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) / 4;

        // Strip visibility modifiers
        let stripped = trimmed
            .trim_start_matches("pub(crate) ")
            .trim_start_matches("pub(super) ")
            .trim_start_matches("pub ")
            .trim_start_matches("async ");

        if let Some(rest) = stripped.strip_prefix("fn ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = stripped.strip_prefix("struct ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Struct,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = stripped.strip_prefix("enum ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Enum,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = stripped.strip_prefix("trait ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Trait,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = stripped.strip_prefix("impl ") {
            // impl Foo or impl Bar for Foo
            let name = if let Some(for_pos) = rest.find(" for ") {
                format!(
                    "impl {} for {}",
                    rest[for_pos + 5..].split('{').next().unwrap_or("").trim(),
                    rest[..for_pos].trim()
                )
            } else {
                format!(
                    "impl {}",
                    rest.split('{')
                        .next()
                        .unwrap_or("")
                        .split('<')
                        .next()
                        .unwrap_or("")
                        .trim()
                )
            };
            symbols.push(OutlineSymbol {
                name,
                kind: SymbolKind::Impl,
                line: line_idx,
                indent,
            });
        } else if let Some(rest) = stripped.strip_prefix("mod ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Module,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = stripped.strip_prefix("type ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Type,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = stripped.strip_prefix("const ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Constant,
                    line: line_idx,
                    indent,
                });
            }
        }
    }
    symbols
}

fn extract_python(content: &str) -> Vec<OutlineSymbol> {
    let mut symbols = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) / 4;

        if let Some(rest) = trimmed.strip_prefix("def ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = trimmed.strip_prefix("async def ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = trimmed.strip_prefix("class ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Class,
                    line: line_idx,
                    indent,
                });
            }
        }
    }
    symbols
}

fn extract_js_ts(content: &str) -> Vec<OutlineSymbol> {
    let mut symbols = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) / 2;

        // function foo() / async function foo()
        let stripped = trimmed
            .trim_start_matches("export default ")
            .trim_start_matches("export ");
        let stripped = stripped.trim_start_matches("async ");
        if let Some(rest) = stripped.strip_prefix("function ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = stripped.strip_prefix("class ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Class,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = stripped.strip_prefix("interface ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Trait,
                    line: line_idx,
                    indent,
                });
            }
        } else if let Some(rest) = stripped.strip_prefix("type ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Type,
                    line: line_idx,
                    indent,
                });
            }
        }
        // const foo = () => / const foo = function
        else if let Some(rest) = stripped.strip_prefix("const ") {
            if let Some(name) = extract_identifier(rest) {
                if rest.contains("=>") || rest.contains("function") {
                    symbols.push(OutlineSymbol {
                        name,
                        kind: SymbolKind::Function,
                        line: line_idx,
                        indent,
                    });
                } else {
                    symbols.push(OutlineSymbol {
                        name,
                        kind: SymbolKind::Constant,
                        line: line_idx,
                        indent,
                    });
                }
            }
        }
    }
    symbols
}

fn extract_go(content: &str) -> Vec<OutlineSymbol> {
    let mut symbols = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) / 4;

        if let Some(rest) = trimmed.strip_prefix("func ") {
            // func (r Receiver) Name(...) or func Name(...)
            let name = if rest.starts_with('(') {
                // method with receiver — extract method name
                let after_paren = rest.find(')').and_then(|i| rest.get(i + 2..));
                after_paren
                    .and_then(extract_identifier)
                    .unwrap_or_else(|| "func".to_string())
            } else {
                extract_identifier(rest).unwrap_or_else(|| "func".to_string())
            };
            symbols.push(OutlineSymbol {
                name,
                kind: SymbolKind::Function,
                line: line_idx,
                indent,
            });
        } else if let Some(rest) = trimmed.strip_prefix("type ") {
            if let Some(name) = extract_identifier(rest) {
                let kind = if rest.contains("struct") {
                    SymbolKind::Struct
                } else if rest.contains("interface") {
                    SymbolKind::Trait
                } else {
                    SymbolKind::Type
                };
                symbols.push(OutlineSymbol {
                    name,
                    kind,
                    line: line_idx,
                    indent,
                });
            }
        }
    }
    symbols
}

fn extract_c(content: &str) -> Vec<OutlineSymbol> {
    let mut symbols = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) / 4;

        // struct Foo {
        if let Some(rest) = trimmed.strip_prefix("struct ") {
            if let Some(name) = extract_identifier(rest) {
                symbols.push(OutlineSymbol {
                    name,
                    kind: SymbolKind::Struct,
                    line: line_idx,
                    indent,
                });
            }
        }
        // typedef struct Foo / typedef enum Foo
        else if trimmed.starts_with("typedef") {
            if trimmed.contains("struct") {
                if let Some(rest) = trimmed.split("struct").nth(1) {
                    if let Some(name) = extract_identifier(rest.trim()) {
                        symbols.push(OutlineSymbol {
                            name,
                            kind: SymbolKind::Struct,
                            line: line_idx,
                            indent,
                        });
                    }
                }
            }
        }
        // Simple function-like: return_type name(
        else if !trimmed.starts_with("//")
            && !trimmed.starts_with("*")
            && trimmed.contains('(')
            && !trimmed.contains(';')
        {
            let before_paren = trimmed.split('(').next().unwrap_or("");
            let last_word = before_paren.split_whitespace().last().unwrap_or("");
            if !last_word.is_empty()
                && last_word.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !is_c_keyword(last_word)
            {
                symbols.push(OutlineSymbol {
                    name: last_word.to_string(),
                    kind: SymbolKind::Function,
                    line: line_idx,
                    indent,
                });
            }
        }
    }
    symbols
}

fn is_c_keyword(s: &str) -> bool {
    matches!(
        s,
        "if" | "else"
            | "for"
            | "while"
            | "switch"
            | "return"
            | "void"
            | "int"
            | "char"
            | "long"
            | "unsigned"
            | "static"
            | "extern"
            | "inline"
    )
}

/// Extract the first identifier from a string (stops at whitespace, `<`, `(`, `{`).
fn extract_identifier(s: &str) -> Option<String> {
    let ident: String = s
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if ident.is_empty() {
        None
    } else {
        Some(ident)
    }
}
