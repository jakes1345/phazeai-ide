use egui::{self, Color32, RichText};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::themes::ThemeColors;

// ── Git Status ────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub enum GitFileState {
    Modified,
    Added,
    Deleted,
    Untracked,
    Renamed,
    Conflicted,
}

impl GitFileState {
    fn label(&self) -> &'static str {
        match self {
            GitFileState::Modified => "M",
            GitFileState::Added => "A",
            GitFileState::Deleted => "D",
            GitFileState::Untracked => "?",
            GitFileState::Renamed => "R",
            GitFileState::Conflicted => "!",
        }
    }

    fn color(&self, theme: &ThemeColors) -> Color32 {
        match self {
            GitFileState::Modified => theme.warning,
            GitFileState::Added => theme.success,
            GitFileState::Deleted => theme.error,
            GitFileState::Untracked => theme.text_muted,
            GitFileState::Renamed => theme.accent,
            GitFileState::Conflicted => Color32::from_rgb(255, 80, 80),
        }
    }
}

// ── File Entry ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct FileEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    depth: usize,
    expanded: bool,
    children_loaded: bool,
    git_state: Option<GitFileState>,
}

fn load_entries_for(dir: &Path, depth: usize, git_status: &HashMap<PathBuf, GitFileState>) -> Vec<FileEntry> {
    let mut entries: Vec<FileEntry> = Vec::new();

    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if name.starts_with('.') || name == "node_modules" || name == "target"
                || name == "__pycache__" || name == "_archive" || name == "dist" || name == "build"
            {
                continue;
            }

            let is_dir = path.is_dir();
            let git_state = git_status.get(&path).cloned();

            entries.push(FileEntry { name, path, is_dir, depth, expanded: false, children_loaded: false, git_state });
        }
    }

    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    entries
}

// ── Explorer Panel ────────────────────────────────────────────────────────

pub struct ExplorerPanel {
    root: Option<PathBuf>,
    entries: Vec<FileEntry>,
    selected: Option<PathBuf>,
    pub file_to_open: Option<PathBuf>,
    /// git status: path → state (relative to git root)
    git_status: HashMap<PathBuf, GitFileState>,
    /// When did we last refresh git status
    last_git_refresh: Option<Instant>,
    /// Git root (may differ from explorer root)
    git_root: Option<PathBuf>,
}

impl ExplorerPanel {
    pub fn new() -> Self {
        Self {
            root: None,
            entries: Vec::new(),
            selected: None,
            file_to_open: None,
            git_status: HashMap::new(),
            last_git_refresh: None,
            git_root: None,
        }
    }

    pub fn root(&self) -> Option<&PathBuf> {
        self.root.as_ref()
    }

    pub fn set_root(&mut self, root: PathBuf) {
        self.entries.clear();
        self.git_root = find_git_root(&root);
        self.refresh_git_status();
        let entries = load_entries_for(&root, 0, &self.git_status);
        self.entries = entries;
        self.root = Some(root);
    }

    /// Run `git status --porcelain` and update the internal map.
    pub fn refresh_git_status(&mut self) {
        self.last_git_refresh = Some(Instant::now());
        self.git_status.clear();

        let git_root = match &self.git_root {
            Some(r) => r.clone(),
            None => return,
        };

        let output = std::process::Command::new("git")
            .args(["status", "--porcelain", "-u"])
            .current_dir(&git_root)
            .output();

        let output = match output {
            Ok(o) if o.status.success() => o,
            _ => return,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.len() < 3 { continue; }
            let xy = &line[..2];
            let path_str = line[3..].trim();
            // Handle renames: "old -> new" format
            let file_path = if path_str.contains(" -> ") {
                path_str.split(" -> ").last().unwrap_or(path_str)
            } else {
                path_str
            };

            let state = parse_xy(xy);
            let abs_path = git_root.join(file_path);
            self.git_status.insert(abs_path, state);
        }

        // Propagate "has changes" up to parent directories too (for dir coloring)
        let paths: Vec<PathBuf> = self.git_status.keys().cloned().collect();
        for path in paths {
            let mut parent = path.parent();
            while let Some(p) = parent {
                if p == git_root { break; }
                self.git_status.entry(p.to_path_buf()).or_insert(GitFileState::Modified);
                parent = p.parent();
            }
        }

        // Update git_state on existing entries
        for entry in &mut self.entries {
            entry.git_state = self.git_status.get(&entry.path).cloned();
        }
    }

    fn maybe_refresh_git(&mut self) {
        let needs = match self.last_git_refresh {
            None => true,
            Some(t) => t.elapsed().as_secs() >= 3,
        };
        if needs {
            self.refresh_git_status();
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        self.maybe_refresh_git();

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "EXPLORER");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⟳").clicked() {
                    if let Some(root) = self.root.clone() {
                        self.set_root(root);
                    }
                }
            });
        });

        // Git branch indicator
        if let Some(ref git_root) = self.git_root.clone() {
            if let Ok(output) = std::process::Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(git_root)
                .output()
            {
                let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !branch.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.colored_label(theme.accent, format!("⎇ {}", branch));
                    });
                }
            }
        }

        ui.separator();

        if self.root.is_none() {
            ui.centered_and_justified(|ui| {
                ui.colored_label(theme.text_muted, "No folder open\nFile → Open Folder");
            });
            return;
        }

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut toggle_idx: Option<usize> = None;
                let mut open_file: Option<PathBuf> = None;

                let mut i = 0;
                while i < self.entries.len() {
                    let entry = &self.entries[i];
                    let indent = entry.depth as f32 * 14.0;
                    let is_selected = self.selected.as_ref() == Some(&entry.path);

                    let icon = if entry.is_dir {
                        if entry.expanded { "▾ " } else { "▸ " }
                    } else {
                        file_icon(&entry.name)
                    };

                    let base_color = if is_selected {
                        theme.accent
                    } else if entry.is_dir {
                        // Directory with changes shows a muted warning color
                        if entry.git_state.is_some() { theme.warning } else { theme.text }
                    } else {
                        match &entry.git_state {
                            Some(GitFileState::Added) => theme.success,
                            Some(GitFileState::Modified) => theme.warning,
                            Some(GitFileState::Deleted) => theme.error,
                            Some(GitFileState::Untracked) => theme.text_muted,
                            Some(GitFileState::Conflicted) => Color32::from_rgb(255, 80, 80),
                            _ => theme.text_secondary,
                        }
                    };

                    let git_badge = entry.git_state.as_ref().filter(|_| !entry.is_dir);

                    ui.horizontal(|ui| {
                        ui.add_space(indent);

                        let label_text = RichText::new(format!("{}{}", icon, entry.name))
                            .color(base_color)
                            .size(13.0);

                        let response = ui.selectable_label(is_selected, label_text);

                        // Git status badge (files only)
                        if let Some(state) = git_badge {
                            ui.add_space(4.0);
                            ui.colored_label(state.color(theme), state.label());
                        }

                        if response.clicked() {
                            if entry.is_dir {
                                toggle_idx = Some(i);
                            } else {
                                self.selected = Some(entry.path.clone());
                                open_file = Some(entry.path.clone());
                            }
                        }
                    });

                    i += 1;
                }

                // Handle directory toggle
                if let Some(idx) = toggle_idx {
                    let entry = &mut self.entries[idx];
                    entry.expanded = !entry.expanded;

                    if entry.expanded && !entry.children_loaded {
                        let path = entry.path.clone();
                        let depth = entry.depth + 1;
                        entry.children_loaded = true;

                        let children = load_entries_for(&path, depth, &self.git_status);
                        let insert_pos = idx + 1;
                        for (j, child) in children.into_iter().enumerate() {
                            self.entries.insert(insert_pos + j, child);
                        }
                    } else if !self.entries[idx].expanded {
                        let parent_depth = self.entries[idx].depth;
                        let start = idx + 1;
                        let mut count = 0;
                        while start + count < self.entries.len()
                            && self.entries[start + count].depth > parent_depth
                        {
                            count += 1;
                        }
                        self.entries.drain(start..start + count);
                        self.entries[idx].children_loaded = false;
                    }
                }

                if let Some(path) = open_file {
                    self.file_to_open = Some(path);
                }
            });
    }
}

fn parse_xy(xy: &str) -> GitFileState {
    let xy = xy.trim();
    match xy {
        "??" => GitFileState::Untracked,
        s if s.contains('U') || s == "AA" || s == "DD" => GitFileState::Conflicted,
        s if s.starts_with('A') || s.ends_with('A') => GitFileState::Added,
        s if s.starts_with('D') || s.ends_with('D') => GitFileState::Deleted,
        s if s.starts_with('R') || s.ends_with('R') => GitFileState::Renamed,
        _ => GitFileState::Modified,
    }
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => "🦀 ",
        "py" => "🐍 ",
        "js" | "jsx" => "📜 ",
        "ts" | "tsx" => "📘 ",
        "html" => "🌐 ",
        "css" | "scss" | "sass" => "🎨 ",
        "json" => "📦 ",
        "toml" | "yaml" | "yml" => "⚙ ",
        "md" => "📝 ",
        "txt" => "📄 ",
        "sh" | "bash" | "zsh" | "fish" => "🐚 ",
        "lock" => "🔒 ",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" => "🖼 ",
        "pdf" => "📕 ",
        "zip" | "tar" | "gz" | "xz" => "📦 ",
        "sql" => "🗃 ",
        "go" => "🔵 ",
        "c" | "cpp" | "h" | "hpp" => "⚡ ",
        "java" | "kt" => "☕ ",
        "rb" => "💎 ",
        "ex" | "exs" => "💜 ",
        "lua" => "🌙 ",
        "nix" => "❄ ",
        "dockerfile" | "Dockerfile" => "🐳 ",
        _ => "  ",
    }
}
