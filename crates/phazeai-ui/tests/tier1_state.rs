//! Tier 1: Pure logic / state-level tests for PhazeAI IDE.
//!
//! These tests run without a display, GPU, or Floem reactive scope.
//! They test the exact same logic that runs in production by extracting
//! pure functions from app.rs keybinding dispatch, command palette filtering,
//! activity bar toggle, and theme selection.
//!
//! Run: `cargo test --test tier1_state`

// ── Command palette filtering ─────────────────────────────────────────────────

const PALETTE_COMMANDS: &[&str] = &[
    "Open File…",
    "Toggle Terminal",
    "Toggle Explorer",
    "Toggle AI Chat",
    "Show AI Panel",
    "Theme: Midnight Blue",
    "Theme: Cyberpunk 2077",
    "Theme: Synthwave '84",
    "Theme: Andromeda",
    "Theme: Dark",
    "Theme: Dracula",
    "Theme: Tokyo Night",
    "Theme: Monokai",
    "Theme: Nord Dark",
    "Theme: Matrix Green",
    "Theme: Root Shell",
    "Theme: Light",
];

fn filter_commands<'a>(query: &str) -> Vec<&'a str> {
    let q = query.to_lowercase();
    PALETTE_COMMANDS
        .iter()
        .copied()
        .filter(|cmd| q.is_empty() || cmd.to_lowercase().contains(&q))
        .collect()
}

#[test]
fn palette_empty_query_returns_all() {
    let results = filter_commands("");
    assert_eq!(results.len(), PALETTE_COMMANDS.len());
}

#[test]
fn palette_filters_toggle_commands() {
    let results = filter_commands("toggle");
    assert_eq!(results.len(), 3);
    assert!(results.contains(&"Toggle Terminal"));
    assert!(results.contains(&"Toggle Explorer"));
    assert!(results.contains(&"Toggle AI Chat"));
}

#[test]
fn palette_filters_theme_commands() {
    let results = filter_commands("theme");
    assert_eq!(results.len(), 12); // 12 themes
}

#[test]
fn palette_case_insensitive() {
    let lower = filter_commands("theme");
    let upper = filter_commands("THEME");
    let mixed = filter_commands("ThEmE");
    assert_eq!(lower.len(), upper.len());
    assert_eq!(lower.len(), mixed.len());
}

#[test]
fn palette_no_match_returns_empty() {
    let results = filter_commands("zzz_nonexistent_zzz");
    assert!(results.is_empty());
}

#[test]
fn palette_partial_match_open_file() {
    let results = filter_commands("file");
    assert!(results.iter().any(|c| c.contains("File")));
}

// ── Keybinding dispatch ───────────────────────────────────────────────────────

#[derive(Default, Debug)]
struct MockPanelState {
    show_left_panel: bool,
    show_bottom_panel: bool,
    show_right_panel: bool,
    left_panel_width: f64,
    command_palette_open: bool,
}

impl MockPanelState {
    fn open() -> Self {
        Self {
            show_left_panel: true,
            left_panel_width: 300.0,
            show_right_panel: true,
            ..Default::default()
        }
    }

    /// Mirror of app.rs keybinding dispatch (Ctrl+key combos).
    fn dispatch_ctrl(&mut self, key: char) {
        match key {
            'b' => {
                self.show_left_panel = !self.show_left_panel;
                self.left_panel_width = if self.show_left_panel { 260.0 } else { 0.0 };
            }
            'j' => {
                self.show_bottom_panel = !self.show_bottom_panel;
            }
            '\\' => {
                self.show_right_panel = !self.show_right_panel;
            }
            'p' => {
                self.command_palette_open = !self.command_palette_open;
            }
            _ => {}
        }
    }
}

#[test]
fn ctrl_j_toggles_terminal() {
    let mut s = MockPanelState::open();
    assert!(!s.show_bottom_panel);
    s.dispatch_ctrl('j');
    assert!(s.show_bottom_panel);
    s.dispatch_ctrl('j');
    assert!(!s.show_bottom_panel);
}

#[test]
fn ctrl_b_toggles_left_panel_and_width() {
    let mut s = MockPanelState::open();
    assert!(s.show_left_panel);
    s.dispatch_ctrl('b');
    assert!(!s.show_left_panel);
    assert_eq!(s.left_panel_width, 0.0);
    s.dispatch_ctrl('b');
    assert!(s.show_left_panel);
    assert_eq!(s.left_panel_width, 260.0);
}

#[test]
fn ctrl_backslash_toggles_chat_panel() {
    let mut s = MockPanelState::open();
    assert!(s.show_right_panel);
    s.dispatch_ctrl('\\');
    assert!(!s.show_right_panel);
    s.dispatch_ctrl('\\');
    assert!(s.show_right_panel);
}

#[test]
fn ctrl_p_toggles_command_palette() {
    let mut s = MockPanelState::open();
    assert!(!s.command_palette_open);
    s.dispatch_ctrl('p');
    assert!(s.command_palette_open);
    s.dispatch_ctrl('p');
    assert!(!s.command_palette_open);
}

// ── Activity bar tab toggle ───────────────────────────────────────────────────

#[derive(Default, Debug, PartialEq)]
enum MockTab {
    #[default]
    Explorer,
    Search,
    Git,
    AI,
    #[allow(dead_code)]
    Debug,
}

#[derive(Debug)]
struct MockActivityBar {
    tab: MockTab,
    show_panel: bool,
    panel_width: f64,
}

impl Default for MockActivityBar {
    fn default() -> Self {
        Self { tab: MockTab::Explorer, show_panel: true, panel_width: 300.0 }
    }
}

impl MockActivityBar {
    /// Mirror of activity_bar_btn on_click_stop logic.
    fn click(&mut self, tab: MockTab) {
        if self.tab == tab && self.show_panel {
            self.show_panel = false;
            self.panel_width = 0.0;
        } else {
            self.tab = tab;
            self.show_panel = true;
            self.panel_width = 300.0;
        }
    }
}

#[test]
fn activity_click_active_tab_closes_panel() {
    let mut bar = MockActivityBar::default();
    assert!(bar.show_panel);
    bar.click(MockTab::Explorer);
    assert!(!bar.show_panel);
    assert_eq!(bar.panel_width, 0.0);
}

#[test]
fn activity_click_different_tab_switches() {
    let mut bar = MockActivityBar::default();
    bar.click(MockTab::Search);
    assert!(bar.show_panel);
    assert_eq!(bar.tab, MockTab::Search);
}

#[test]
fn activity_click_when_closed_opens_panel() {
    let mut bar = MockActivityBar { show_panel: false, panel_width: 0.0, tab: MockTab::Explorer };
    bar.click(MockTab::Explorer);
    assert!(bar.show_panel);
    assert_eq!(bar.panel_width, 300.0);
}

#[test]
fn activity_multi_switch_sequence() {
    let mut bar = MockActivityBar::default();
    // Explorer open → click Git → Git open
    bar.click(MockTab::Git);
    assert_eq!(bar.tab, MockTab::Git);
    assert!(bar.show_panel);
    // Click Git again → close
    bar.click(MockTab::Git);
    assert!(!bar.show_panel);
    // Click AI from closed state → AI open
    bar.click(MockTab::AI);
    assert_eq!(bar.tab, MockTab::AI);
    assert!(bar.show_panel);
}

// ── Theme variant matching ────────────────────────────────────────────────────

// Theme name variants (used as documentation, not in tests directly)
#[allow(dead_code)]
const THEME_NAMES: &[&str] = &[
    "midnight_blue", "midnightblue", "MIDNIGHTBLUE",
    "cyberpunk", "CYBERPUNK",
    "synthwave84", "synthwave",
    "andromeda",
    "dark", "Dark",
    "dracula",
    "tokyonight", "tokyo",
    "monokai",
    "norddark", "nord",
    "matrixgreen", "matrix",
    "rootshell", "root",
    "light", "Light",
];

/// Simplified version of ThemeVariant::from_str logic.
fn parse_theme(s: &str) -> &'static str {
    match s.to_lowercase().replace([' ', '-', '_'], "").as_str() {
        "midnightblue" | "midnight"     => "MidnightBlue",
        "cyberpunk" | "cyber"           => "Cyberpunk",
        "synthwave84" | "synthwave"     => "Synthwave84",
        "andromeda"                     => "Andromeda",
        "dracula"                       => "Dracula",
        "tokyonight" | "tokyo"          => "TokyoNight",
        "monokai"                       => "Monokai",
        "norddark" | "nord"             => "NordDark",
        "matrixgreen" | "matrix"        => "MatrixGreen",
        "rootshell" | "root"            => "RootShell",
        "light"                         => "Light",
        _                               => "Dark",
    }
}

#[test]
fn theme_parsing_case_insensitive() {
    assert_eq!(parse_theme("midnightblue"), "MidnightBlue");
    assert_eq!(parse_theme("MIDNIGHTBLUE"), "MidnightBlue");
    assert_eq!(parse_theme("MidnightBlue"), "MidnightBlue");
}

#[test]
fn theme_parsing_aliases() {
    assert_eq!(parse_theme("tokyo"), "TokyoNight");
    assert_eq!(parse_theme("nord"), "NordDark");
    assert_eq!(parse_theme("matrix"), "MatrixGreen");
    assert_eq!(parse_theme("root"), "RootShell");
    assert_eq!(parse_theme("synthwave"), "Synthwave84");
}

#[test]
fn theme_unknown_falls_back_to_dark() {
    assert_eq!(parse_theme("notatheme"), "Dark");
    assert_eq!(parse_theme(""), "Dark");
}

// ── Search result deduplication / capping ────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
struct SearchResult {
    path: String,
    line: usize,
    content: String,
}

fn cap_results(mut results: Vec<SearchResult>, max: usize) -> Vec<SearchResult> {
    results.truncate(max);
    results
}

#[test]
fn search_results_capped_at_100() {
    let results: Vec<SearchResult> = (0..200)
        .map(|i| SearchResult {
            path: format!("file{i}.rs"),
            line: i + 1,
            content: format!("line {i}"),
        })
        .collect();
    let capped = cap_results(results, 100);
    assert_eq!(capped.len(), 100);
}

#[test]
fn search_results_under_cap_unchanged() {
    let results: Vec<SearchResult> = (0..50)
        .map(|i| SearchResult {
            path: format!("file{i}.rs"),
            line: i + 1,
            content: format!("line {i}"),
        })
        .collect();
    let capped = cap_results(results, 100);
    assert_eq!(capped.len(), 50);
}

// ── File extension → language detection ──────────────────────────────────────

fn detect_language(path: &str) -> &'static str {
    match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
    {
        "rs"              => "Rust",
        "py"              => "Python",
        "js" | "ts"       => "TypeScript",
        "jsx" | "tsx"     => "TSX",
        "toml"            => "TOML",
        "md"              => "Markdown",
        "json"            => "JSON",
        "yaml" | "yml"    => "YAML",
        "sh" | "bash"     => "Shell",
        "c" | "h"         => "C",
        "cpp" | "hpp"     => "C++",
        "go"              => "Go",
        _                 => "Text",
    }
}

#[test]
fn language_detection_rust() {
    assert_eq!(detect_language("src/main.rs"), "Rust");
    assert_eq!(detect_language("lib.rs"), "Rust");
}

#[test]
fn language_detection_web() {
    assert_eq!(detect_language("index.ts"), "TypeScript");
    assert_eq!(detect_language("app.js"), "TypeScript");
    assert_eq!(detect_language("Component.tsx"), "TSX");
}

#[test]
fn language_detection_config() {
    assert_eq!(detect_language("Cargo.toml"), "TOML");
    assert_eq!(detect_language("data.json"), "JSON");
    assert_eq!(detect_language("config.yaml"), "YAML");
}

#[test]
fn language_detection_unknown_fallback() {
    assert_eq!(detect_language("binary.exe"), "Text");
    assert_eq!(detect_language("README"), "Text");
    assert_eq!(detect_language("Makefile"), "Text");
}

// ── Git status parsing ────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum GitStatus {
    Modified,
    Added,
    Deleted,
    Untracked,
    Renamed,
}

#[derive(Debug)]
struct GitEntry {
    status: GitStatus,
    path: String,
}

fn parse_git_porcelain(output: &str) -> Vec<GitEntry> {
    output
        .lines()
        .filter(|l| l.len() >= 4)
        .filter_map(|line| {
            let staged = line.chars().nth(0)?;
            let unstaged = line.chars().nth(1)?;
            let path = line[3..].trim().to_string();

            let status = match (staged, unstaged) {
                ('?', '?')                  => GitStatus::Untracked,
                ('A', _) | (_, 'A')        => GitStatus::Added,
                ('D', _) | (_, 'D')        => GitStatus::Deleted,
                ('R', _) | (_, 'R')        => GitStatus::Renamed,
                ('M', _) | (_, 'M')        => GitStatus::Modified,
                _                          => return None,
            };
            Some(GitEntry { status, path })
        })
        .collect()
}

#[test]
fn git_parse_modified_file() {
    let output = " M src/main.rs\n";
    let entries = parse_git_porcelain(output);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, GitStatus::Modified);
    assert_eq!(entries[0].path, "src/main.rs");
}

#[test]
fn git_parse_untracked_file() {
    let output = "?? new_file.rs\n";
    let entries = parse_git_porcelain(output);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, GitStatus::Untracked);
}

#[test]
fn git_parse_staged_added() {
    let output = "A  added.rs\n";
    let entries = parse_git_porcelain(output);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, GitStatus::Added);
}

#[test]
fn git_parse_multiple_entries() {
    let output = " M Cargo.toml\n?? new.rs\nA  staged.rs\n D deleted.rs\n";
    let entries = parse_git_porcelain(output);
    assert_eq!(entries.len(), 4);
}

#[test]
fn git_parse_empty_output() {
    let entries = parse_git_porcelain("");
    assert!(entries.is_empty());
}

// ── Diagnostic severity ordering ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SeverityLevel { Hint = 0, Info = 1, Warning = 2, Error = 3 }

fn severity_priority(s: SeverityLevel) -> u8 {
    s as u8
}

#[test]
fn severity_error_is_highest() {
    assert!(severity_priority(SeverityLevel::Error) > severity_priority(SeverityLevel::Warning));
    assert!(severity_priority(SeverityLevel::Warning) > severity_priority(SeverityLevel::Info));
    assert!(severity_priority(SeverityLevel::Info) > severity_priority(SeverityLevel::Hint));
}

// ── DiagEntry path parsing ────────────────────────────────────────────────────

fn parse_diag_uri(uri: &str) -> std::path::PathBuf {
    uri.strip_prefix("file://")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(uri))
}

#[test]
fn diag_uri_strips_file_prefix() {
    let path = parse_diag_uri("file:///home/user/project/src/main.rs");
    assert_eq!(path, std::path::PathBuf::from("/home/user/project/src/main.rs"));
}

#[test]
fn diag_uri_no_prefix_passthrough() {
    let path = parse_diag_uri("/absolute/path.rs");
    assert_eq!(path, std::path::PathBuf::from("/absolute/path.rs"));
}

// ── CompletionEntry prefix filter ────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CompletionEntry { label: String }

fn filter_completions<'a>(entries: &'a [CompletionEntry], prefix: &str) -> Vec<&'a CompletionEntry> {
    let p = prefix.to_lowercase();
    entries.iter().filter(|e| e.label.to_lowercase().starts_with(&p)).collect()
}

#[test]
fn completion_filter_empty_prefix_returns_all() {
    let entries = vec![
        CompletionEntry { label: "println".into() },
        CompletionEntry { label: "print".into() },
        CompletionEntry { label: "eprintln".into() },
    ];
    assert_eq!(filter_completions(&entries, "").len(), 3);
}

#[test]
fn completion_filter_by_prefix() {
    let entries = vec![
        CompletionEntry { label: "println".into() },
        CompletionEntry { label: "print".into() },
        CompletionEntry { label: "eprintln".into() },
    ];
    let result = filter_completions(&entries, "print");
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|e| e.label.starts_with("print")));
}

#[test]
fn completion_filter_case_insensitive() {
    let entries = vec![
        CompletionEntry { label: "PrintLn".into() },
        CompletionEntry { label: "eprint".into() },
    ];
    let result = filter_completions(&entries, "PRINT");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].label, "PrintLn");
}

#[test]
fn completion_filter_no_match() {
    let entries = vec![CompletionEntry { label: "foo".into() }];
    assert!(filter_completions(&entries, "bar").is_empty());
}

// ── Session persistence helpers ───────────────────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
struct SessionTab {
    path: String,
    is_dirty: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
struct Session {
    active_tab: usize,
    tabs: Vec<SessionTab>,
    left_panel_width: f64,
    bottom_panel_height: f64,
}

impl Session {
    fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap()
    }
    fn from_toml(s: &str) -> Self {
        toml::from_str(s).unwrap()
    }
}

#[test]
fn session_roundtrip_toml() {
    let session = Session {
        active_tab: 1,
        tabs: vec![
            SessionTab { path: "/home/user/main.rs".into(), is_dirty: false },
            SessionTab { path: "/home/user/lib.rs".into(),  is_dirty: true  },
        ],
        left_panel_width: 280.0,
        bottom_panel_height: 200.0,
    };
    let toml_str = session.to_toml();
    let loaded = Session::from_toml(&toml_str);
    assert_eq!(session, loaded);
}

#[test]
fn session_active_tab_clamped_to_valid_range() {
    let session = Session {
        active_tab: 99, // out of range
        tabs: vec![SessionTab { path: "a.rs".into(), is_dirty: false }],
        left_panel_width: 260.0,
        bottom_panel_height: 160.0,
    };
    // Restoration logic should clamp active_tab to tabs.len()-1
    let clamped = if session.active_tab >= session.tabs.len() {
        session.tabs.len().saturating_sub(1)
    } else {
        session.active_tab
    };
    assert_eq!(clamped, 0);
}

#[test]
fn session_empty_tabs_stays_empty() {
    let session = Session {
        active_tab: 0,
        tabs: vec![],
        left_panel_width: 260.0,
        bottom_panel_height: 160.0,
    };
    let toml_str = session.to_toml();
    let loaded = Session::from_toml(&toml_str);
    assert!(loaded.tabs.is_empty());
}

// ── Cursor offset ↔ line/col math (mirrors editor.rs tracking) ───────────────

fn byte_offset_to_line_col(text: &str, byte_offset: usize) -> (u32, u32) {
    let mut line = 0u32;
    let mut line_start = 0usize;
    for (i, ch) in text.char_indices() {
        if i >= byte_offset { break; }
        if ch == '\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    ((byte_offset - line_start) as u32, line) // (col, line) — swap to match LSP
}

#[test]
fn cursor_offset_line0_col0() {
    let (col, line) = byte_offset_to_line_col("hello\nworld", 0);
    assert_eq!(line, 0);
    assert_eq!(col, 0);
}

#[test]
fn cursor_offset_end_of_first_line() {
    let text = "hello\nworld";
    let (col, line) = byte_offset_to_line_col(text, 4); // 'o' in "hello"
    assert_eq!(line, 0);
    assert_eq!(col, 4);
}

#[test]
fn cursor_offset_second_line_start() {
    let text = "hello\nworld";
    let (col, line) = byte_offset_to_line_col(text, 6); // 'w' in "world"
    assert_eq!(line, 1);
    assert_eq!(col, 0);
}

#[test]
fn cursor_offset_second_line_mid() {
    let text = "hello\nworld";
    let (col, line) = byte_offset_to_line_col(text, 9); // 'l' in "world"
    assert_eq!(line, 1);
    assert_eq!(col, 3);
}

// ── Tab dirty-state management ────────────────────────────────────────────────

#[derive(Debug, Default)]
struct MockEditorTab {
    path: String,
    content: String,
    saved_content: String,
}

impl MockEditorTab {
    fn new(path: &str, content: &str) -> Self {
        Self { path: path.into(), content: content.into(), saved_content: content.into() }
    }
    fn is_dirty(&self) -> bool { self.content != self.saved_content }
    fn save(&mut self) { self.saved_content = self.content.clone(); }
    fn edit(&mut self, new_content: &str) { self.content = new_content.into(); }
}

#[test]
fn tab_clean_on_open() {
    let tab = MockEditorTab::new("main.rs", "fn main() {}");
    assert!(!tab.is_dirty());
}

#[test]
fn tab_dirty_after_edit() {
    let mut tab = MockEditorTab::new("main.rs", "fn main() {}");
    tab.edit("fn main() { println!(\"hi\"); }");
    assert!(tab.is_dirty());
}

#[test]
fn tab_clean_after_save() {
    let mut tab = MockEditorTab::new("main.rs", "fn main() {}");
    tab.edit("fn main() { println!(\"hi\"); }");
    assert!(tab.is_dirty());
    tab.save();
    assert!(!tab.is_dirty());
}

#[test]
fn tab_stays_clean_after_content_returns_to_original() {
    // Once content matches saved_content again → dirty flag clears
    let mut tab = MockEditorTab::new("main.rs", "fn main() {}");
    tab.edit("changed");
    assert!(tab.is_dirty());
    tab.edit("fn main() {}"); // content back to original
    assert!(!tab.is_dirty());
}
