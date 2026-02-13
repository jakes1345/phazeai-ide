use egui;
use std::path::{Path, PathBuf};

use crate::themes::ThemeColors;

#[derive(Clone)]
struct FileEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    depth: usize,
    expanded: bool,
    children_loaded: bool,
}

pub struct ExplorerPanel {
    root: Option<PathBuf>,
    entries: Vec<FileEntry>,
    selected: Option<PathBuf>,
    pub file_to_open: Option<PathBuf>,
}

impl ExplorerPanel {
    pub fn new() -> Self {
        Self {
            root: None,
            entries: Vec::new(),
            selected: None,
            file_to_open: None,
        }
    }

    pub fn set_root(&mut self, root: PathBuf) {
        self.entries.clear();
        self.load_directory(&root, 0);
        self.root = Some(root);
    }

    fn load_directory(&mut self, dir: &Path, depth: usize) {
        let mut entries: Vec<FileEntry> = Vec::new();

        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip hidden files and common unneeded dirs
                if name.starts_with('.') || name == "node_modules" || name == "target" || name == "__pycache__" || name == "_archive" {
                    continue;
                }

                let is_dir = path.is_dir();
                entries.push(FileEntry {
                    name,
                    path,
                    is_dir,
                    depth,
                    expanded: false,
                    children_loaded: false,
                });
            }
        }

        // Sort: directories first, then alphabetical
        entries.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        self.entries.extend(entries);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "EXPLORER");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("refresh").clicked() {
                    if let Some(root) = self.root.clone() {
                        self.set_root(root);
                    }
                }
            });
        });
        ui.separator();

        if self.root.is_none() {
            ui.centered_and_justified(|ui| {
                ui.colored_label(theme.text_muted, "No folder open");
            });
            return;
        }

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut i = 0;
                let mut toggle_idx: Option<usize> = None;
                let mut open_file: Option<PathBuf> = None;

                while i < self.entries.len() {
                    let entry = &self.entries[i];
                    let indent = entry.depth as f32 * 16.0;
                    let is_selected = self.selected.as_ref() == Some(&entry.path);

                    ui.horizontal(|ui| {
                        ui.add_space(indent);

                        let icon = if entry.is_dir {
                            if entry.expanded { "v " } else { "> " }
                        } else {
                            file_icon(&entry.name)
                        };

                        let text_color = if is_selected {
                            theme.accent
                        } else if entry.is_dir {
                            theme.text
                        } else {
                            theme.text_secondary
                        };

                        let label = format!("{}{}", icon, entry.name);
                        let response = ui.selectable_label(is_selected, egui::RichText::new(&label).color(text_color));

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

                        // Load children
                        let mut children: Vec<FileEntry> = Vec::new();
                        if let Ok(read_dir) = std::fs::read_dir(&path) {
                            for dir_entry in read_dir.flatten() {
                                let child_path = dir_entry.path();
                                let name = dir_entry.file_name().to_string_lossy().to_string();

                                if name.starts_with('.')
                                    || name == "node_modules"
                                    || name == "target"
                                    || name == "__pycache__"
                                    || name == "_archive"
                                {
                                    continue;
                                }

                                children.push(FileEntry {
                                    name,
                                    path: child_path.clone(),
                                    is_dir: child_path.is_dir(),
                                    depth,
                                    expanded: false,
                                    children_loaded: false,
                                });
                            }
                        }

                        children.sort_by(|a, b| {
                            match (a.is_dir, b.is_dir) {
                                (true, false) => std::cmp::Ordering::Less,
                                (false, true) => std::cmp::Ordering::Greater,
                                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                            }
                        });

                        // Insert children right after the parent
                        let insert_pos = idx + 1;
                        for (j, child) in children.into_iter().enumerate() {
                            self.entries.insert(insert_pos + j, child);
                        }
                    } else if !self.entries[idx].expanded {
                        // Collapse: remove all children (entries with depth > parent's depth)
                        let parent_depth = self.entries[idx].depth;
                        let mut remove_count = 0;
                        let start = idx + 1;
                        while start + remove_count < self.entries.len()
                            && self.entries[start + remove_count].depth > parent_depth
                        {
                            remove_count += 1;
                        }
                        self.entries.drain(start..start + remove_count);
                        self.entries[idx].children_loaded = false;
                    }
                }

                if let Some(path) = open_file {
                    self.file_to_open = Some(path);
                }
            });
    }
}

fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => "# ",
        "py" => "# ",
        "js" | "jsx" | "ts" | "tsx" => "# ",
        "html" => "# ",
        "css" | "scss" => "# ",
        "json" => "{ ",
        "toml" | "yaml" | "yml" => "@ ",
        "md" => "# ",
        "txt" => "= ",
        "sh" | "bash" | "zsh" => "$ ",
        "lock" => "# ",
        _ => "  ",
    }
}
