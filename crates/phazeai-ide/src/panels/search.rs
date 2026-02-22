use egui::{self, FontId, RichText};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::themes::ThemeColors;

// ── Search Result ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub file: PathBuf,
    pub line_number: usize,
    pub line_text: String,
    pub match_start: usize,
    pub match_len: usize,
}

// ── Search State ──────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
pub enum SearchMode {
    Text,
    Regex,
}

// ── Search Panel ──────────────────────────────────────────────────────────

pub struct SearchPanel {
    pub query: String,
    pub replacement: String,
    pub show_replace: bool,
    pub mode: SearchMode,
    pub case_sensitive: bool,
    pub include_glob: String,
    pub results: Arc<Mutex<Vec<SearchResult>>>,
    pub is_searching: Arc<Mutex<bool>>,
    pub result_count: usize,
    pub search_root: Option<PathBuf>,
    /// File + line to open (consumed by app.rs)
    pub file_to_open: Option<(PathBuf, usize)>,
    last_query: String,
    scroll_to_top: bool,
    /// Status after a replace-all operation
    replace_status: Option<String>,
}

impl Default for SearchPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchPanel {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            replacement: String::new(),
            show_replace: false,
            mode: SearchMode::Text,
            case_sensitive: false,
            include_glob: String::new(),
            results: Arc::new(Mutex::new(Vec::new())),
            is_searching: Arc::new(Mutex::new(false)),
            result_count: 0,
            search_root: None,
            file_to_open: None,
            last_query: String::new(),
            scroll_to_top: false,
            replace_status: None,
        }
    }

    pub fn set_root(&mut self, root: PathBuf) {
        self.search_root = Some(root);
    }

    pub fn run_search(&mut self) {
        let query = self.query.trim().to_string();
        if query.is_empty() {
            if let Ok(mut r) = self.results.lock() {
                r.clear();
            }
            self.result_count = 0;
            return;
        }

        let root = match &self.search_root {
            Some(r) => r.clone(),
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };

        let results = Arc::clone(&self.results);
        let searching = Arc::clone(&self.is_searching);
        let case_sensitive = self.case_sensitive;
        let use_regex = self.mode == SearchMode::Regex;
        let include_glob = self.include_glob.clone();

        // Clear old results
        if let Ok(mut r) = results.lock() {
            r.clear();
        }
        *searching.lock().unwrap() = true;
        self.scroll_to_top = true;

        std::thread::spawn(move || {
            let found = run_ripgrep(&query, &root, case_sensitive, use_regex, &include_glob);
            if let Ok(mut r) = results.lock() {
                *r = found;
            }
            *searching.lock().unwrap() = false;
        });
    }

    /// Replace all occurrences across files. Returns (files_changed, replacements_made).
    pub fn replace_all_in_files(&mut self) -> (usize, usize) {
        let results = match self.results.lock() {
            Ok(r) => r.clone(),
            Err(_) => return (0, 0),
        };
        if results.is_empty() || self.query.is_empty() {
            return (0, 0);
        }

        let query = self.query.clone();
        let replacement = self.replacement.clone();
        let case_sensitive = self.case_sensitive;
        let use_regex = self.mode == SearchMode::Regex;

        // Collect unique files
        let files: Vec<PathBuf> = {
            let mut seen = HashSet::new();
            results
                .iter()
                .filter(|r| seen.insert(r.file.clone()))
                .map(|r| r.file.clone())
                .collect()
        };

        let mut files_changed = 0;
        let mut total_replacements = 0;

        for file in &files {
            let content = match std::fs::read_to_string(file) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let (new_content, n) = if use_regex {
                // Regex replace using simple line-by-line approach
                let mut changed = 0;
                let lines: Vec<String> = content
                    .lines()
                    .map(|l| {
                        if case_sensitive {
                            if l.contains(&query) {
                                changed += l.matches(&query).count();
                                l.replace(&query, &replacement)
                            } else {
                                l.to_string()
                            }
                        } else {
                            let lower = l.to_lowercase();
                            let ql = query.to_lowercase();
                            if lower.contains(&ql) {
                                changed += lower.matches(&ql).count();
                                // Case-insensitive replace (simple)
                                replace_case_insensitive(l, &query, &replacement)
                            } else {
                                l.to_string()
                            }
                        }
                    })
                    .collect();
                let nc = if content.ends_with('\n') {
                    format!("{}\n", lines.join("\n"))
                } else {
                    lines.join("\n")
                };
                (nc, changed)
            } else {
                let mut changed = 0;
                let new = if case_sensitive {
                    changed += content.matches(&query).count();
                    content.replace(&query, &replacement)
                } else {
                    changed += content
                        .to_lowercase()
                        .matches(&query.to_lowercase())
                        .count();
                    replace_case_insensitive(&content, &query, &replacement)
                };
                (new, changed)
            };

            if n > 0 && std::fs::write(file, new_content).is_ok() {
                files_changed += 1;
                total_replacements += n;
            }
        }

        // Clear results after replacing
        if let Ok(mut r) = self.results.lock() {
            r.clear();
        }
        self.result_count = 0;

        (files_changed, total_replacements)
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        let is_searching = *self.is_searching.lock().unwrap_or_else(|e| e.into_inner());
        let result_count = self.results.lock().map(|r| r.len()).unwrap_or(0);
        self.result_count = result_count;

        // Header
        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "SEARCH");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let replace_label = if self.show_replace {
                    egui::RichText::new("↔").color(theme.accent)
                } else {
                    egui::RichText::new("↔").color(theme.text_muted)
                };
                if ui
                    .button(replace_label)
                    .on_hover_text("Toggle replace")
                    .clicked()
                {
                    self.show_replace = !self.show_replace;
                }
            });
        });
        ui.separator();

        // Query bar
        ui.horizontal(|ui| {
            let search_resp = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search in workspace...")
                    .desired_width(ui.available_width() - 60.0)
                    .font(FontId::monospace(13.0)),
            );

            let enter_pressed =
                search_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if ui
                .add_enabled(!is_searching, egui::Button::new("🔍"))
                .clicked()
                || (enter_pressed && !is_searching)
            {
                self.last_query = self.query.clone();
                self.run_search();
            }
        });

        // Replace bar
        if self.show_replace {
            let mut do_replace_all = false;
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.replacement)
                        .hint_text("Replace with...")
                        .desired_width(ui.available_width() - 120.0)
                        .font(FontId::monospace(13.0)),
                );
                let result_count = self.result_count;
                let enabled = result_count > 0 && !self.query.is_empty();
                if ui
                    .add_enabled(enabled, egui::Button::new("Replace All"))
                    .clicked()
                {
                    do_replace_all = true;
                }
            });

            if do_replace_all {
                let (files, replacements) = self.replace_all_in_files();
                self.replace_status = Some(format!(
                    "Replaced {} occurrence{} in {} file{}",
                    replacements,
                    if replacements == 1 { "" } else { "s" },
                    files,
                    if files == 1 { "" } else { "s" },
                ));
            }

            if let Some(ref status) = self.replace_status.clone() {
                ui.colored_label(theme.success, status);
            }
        }

        // Options row
        ui.horizontal(|ui| {
            ui.add_space(4.0);

            // Case sensitive toggle
            let cs_label = if self.case_sensitive {
                RichText::new("Aa").color(theme.accent).size(11.0)
            } else {
                RichText::new("Aa").color(theme.text_muted).size(11.0)
            };
            if ui.button(cs_label).clicked() {
                self.case_sensitive = !self.case_sensitive;
            }
            ui.label(RichText::new("case").color(theme.text_muted).size(10.0));

            ui.add_space(8.0);

            // Regex toggle
            let rx_label = if self.mode == SearchMode::Regex {
                RichText::new(".*").color(theme.accent).size(11.0)
            } else {
                RichText::new(".*").color(theme.text_muted).size(11.0)
            };
            if ui.button(rx_label).clicked() {
                self.mode = if self.mode == SearchMode::Regex {
                    SearchMode::Text
                } else {
                    SearchMode::Regex
                };
            }
            ui.label(RichText::new("regex").color(theme.text_muted).size(10.0));
        });

        // Include glob filter
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.colored_label(theme.text_muted, RichText::new("files:").size(11.0));
            ui.add(
                egui::TextEdit::singleline(&mut self.include_glob)
                    .hint_text("*.rs, src/**")
                    .desired_width(ui.available_width() - 8.0)
                    .font(FontId::monospace(11.0)),
            );
        });

        ui.separator();

        // Status line
        if is_searching {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.colored_label(theme.text_muted, "Searching...");
            });
        } else if !self.last_query.is_empty() {
            let count_color = if result_count == 0 {
                theme.error
            } else {
                theme.success
            };
            ui.colored_label(
                count_color,
                RichText::new(format!(
                    "{} result{} for \"{}\"",
                    result_count,
                    if result_count == 1 { "" } else { "s" },
                    self.last_query
                ))
                .size(11.0),
            );
        }

        // Results list
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let results = match self.results.lock() {
                    Ok(r) => r.clone(),
                    Err(_) => return,
                };

                let mut current_file: Option<PathBuf> = None;
                let mut file_to_open: Option<(PathBuf, usize)> = None;

                for result in &results {
                    // File header when file changes
                    if current_file.as_ref() != Some(&result.file) {
                        current_file = Some(result.file.clone());
                        ui.add_space(4.0);

                        let rel_path = result
                            .file
                            .strip_prefix(self.search_root.as_deref().unwrap_or(&result.file))
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| result.file.display().to_string());

                        ui.horizontal(|ui| {
                            ui.colored_label(
                                theme.accent,
                                RichText::new(&rel_path).size(12.0).strong(),
                            );
                        });
                    }

                    // Result line
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);

                        // Line number
                        ui.colored_label(
                            theme.text_muted,
                            RichText::new(format!("{:4}:", result.line_number))
                                .monospace()
                                .size(11.0),
                        );

                        // Line text with match highlighted
                        let text = result.line_text.trim();
                        let text = if text.len() > 120 { &text[..120] } else { text };

                        let resp = ui.add(
                            egui::Label::new(
                                RichText::new(text)
                                    .monospace()
                                    .size(11.0)
                                    .color(theme.text_secondary),
                            )
                            .sense(egui::Sense::click()),
                        );

                        if resp.clicked() {
                            file_to_open =
                                Some((result.file.clone(), result.line_number.saturating_sub(1)));
                        }

                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                    });
                }

                if let Some(target) = file_to_open {
                    self.file_to_open = Some(target);
                }

                if results.is_empty() && !is_searching && !self.last_query.is_empty() {
                    ui.add_space(20.0);
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(theme.text_muted, "No results found");
                    });
                }
            });
    }
}

// ── ripgrep runner ────────────────────────────────────────────────────────

fn run_ripgrep(
    query: &str,
    root: &PathBuf,
    case_sensitive: bool,
    use_regex: bool,
    include_glob: &str,
) -> Vec<SearchResult> {
    let mut args: Vec<String> = vec![
        "--line-number".into(),
        "--column".into(),
        "--color=never".into(),
        "--max-count=500".into(),   // cap results per file
        "--max-filesize=1M".into(), // skip huge files
    ];

    if !case_sensitive {
        args.push("--ignore-case".into());
    }
    if !use_regex {
        args.push("--fixed-strings".into());
    }
    if !include_glob.is_empty() {
        for glob in include_glob.split(',') {
            let g = glob.trim();
            if !g.is_empty() {
                args.push(format!("--glob={}", g));
            }
        }
    }

    args.push(query.to_string());
    args.push(root.to_string_lossy().to_string());

    let output = match std::process::Command::new("rg").args(&args).output() {
        Ok(o) => o,
        Err(_) => {
            // Fall back to grep if rg not available
            return run_grep_fallback(query, root, case_sensitive);
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_rg_output(&stdout, root)
}

fn parse_rg_output(output: &str, _root: &PathBuf) -> Vec<SearchResult> {
    let mut results = Vec::new();

    for line in output.lines() {
        // ripgrep output format: file:line:col:text
        let mut parts = line.splitn(4, ':');
        let file_str = parts.next().unwrap_or("");
        let line_num: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let col: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        let text = parts.next().unwrap_or("").to_string();

        if file_str.is_empty() || line_num == 0 {
            continue;
        }

        let file = PathBuf::from(file_str);
        results.push(SearchResult {
            file,
            line_number: line_num,
            line_text: text,
            match_start: col.saturating_sub(1),
            match_len: 1, // will be refined if needed
        });

        if results.len() >= 2000 {
            break;
        }
    }

    results
}

/// Case-insensitive string replacement preserving original case where possible.
fn replace_case_insensitive(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let lower_haystack = haystack.to_lowercase();
    let lower_needle = needle.to_lowercase();
    let mut result = String::with_capacity(haystack.len());
    let mut pos = 0;
    while let Some(found) = lower_haystack[pos..].find(&lower_needle) {
        let actual_pos = pos + found;
        result.push_str(&haystack[pos..actual_pos]);
        result.push_str(replacement);
        pos = actual_pos + needle.len();
    }
    result.push_str(&haystack[pos..]);
    result
}

fn run_grep_fallback(query: &str, root: &PathBuf, case_sensitive: bool) -> Vec<SearchResult> {
    let mut args = vec!["-r", "-n", "--include=*"];
    if !case_sensitive {
        args.push("-i");
    }

    let output = match std::process::Command::new("grep")
        .args(&args)
        .arg(query)
        .arg(root)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines().take(1000) {
        let mut parts = line.splitn(3, ':');
        let file_str = parts.next().unwrap_or("");
        let line_num: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let text = parts.next().unwrap_or("").to_string();

        if file_str.is_empty() {
            continue;
        }

        results.push(SearchResult {
            file: PathBuf::from(file_str),
            line_number: line_num,
            line_text: text,
            match_start: 0,
            match_len: 1,
        });
    }

    results
}
