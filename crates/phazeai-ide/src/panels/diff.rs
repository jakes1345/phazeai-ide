use egui::{self, Color32, FontId, RichText, ScrollArea};
use similar::{ChangeTag, TextDiff};
use std::path::PathBuf;
use std::process::Command;

use crate::themes::ThemeColors;

// ── Diff Line ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct DiffLine {
    pub tag: LineTag,
    pub old_num: Option<usize>,
    pub new_num: Option<usize>,
    pub content: String,
}

#[derive(Clone, PartialEq)]
pub enum LineTag {
    Context,
    Added,
    Removed,
    Hunk,
}

// ── Diff Viewer Panel ──────────────────────────────────────────────────────

pub struct DiffPanel {
    /// Currently selected file to diff (None = show staged overview)
    pub selected_file: Option<PathBuf>,
    /// Parsed diff lines for the selected file
    lines: Vec<DiffLine>,
    /// Git root
    pub git_root: Option<PathBuf>,
    /// All changed files (from `git status`)
    pub changed_files: Vec<(PathBuf, String)>, // (path, status_code)
    last_refresh: Option<std::time::Instant>,
    /// Staged files ready for commit
    pub staged_files: Vec<PathBuf>,
    /// Commit message being composed
    pub commit_message: String,
    /// Result of last commit
    pub commit_status: Option<String>,
    pub visible: bool,
}

impl Default for DiffPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl DiffPanel {
    pub fn new() -> Self {
        Self {
            selected_file: None,
            lines: Vec::new(),
            git_root: None,
            changed_files: Vec::new(),
            last_refresh: None,
            staged_files: Vec::new(),
            commit_message: String::new(),
            commit_status: None,
            visible: false,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            self.refresh();
        }
    }

    pub fn set_git_root(&mut self, root: PathBuf) {
        self.git_root = Some(root);
        self.refresh();
    }

    /// Refresh changed files list from `git status`.
    pub fn refresh(&mut self) {
        self.last_refresh = Some(std::time::Instant::now());
        self.changed_files.clear();
        self.commit_status = None;

        let git_root = match &self.git_root {
            Some(r) => r.clone(),
            None => return,
        };

        let output = Command::new("git")
            .args(["status", "--porcelain", "-u"])
            .current_dir(&git_root)
            .output();

        let output = match output {
            Ok(o) if o.status.success() => o,
            _ => return,
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.len() < 3 {
                continue;
            }
            let xy = &line[..2];
            let path_str = line[3..].trim();
            let path = if path_str.contains(" -> ") {
                path_str.split(" -> ").last().unwrap_or(path_str)
            } else {
                path_str
            };
            self.changed_files
                .push((git_root.join(path), xy.to_string()));
        }
    }

    /// Load diff for a specific file using `git diff`.
    pub fn load_diff_for(&mut self, path: &PathBuf) {
        self.selected_file = Some(path.clone());
        self.lines.clear();

        let git_root = match &self.git_root {
            Some(r) => r.clone(),
            None => return,
        };

        // Try unstaged diff first, then staged
        let output = Command::new("git")
            .args(["diff", "--unified=3", "--", path.to_str().unwrap_or("")])
            .current_dir(&git_root)
            .output()
            .ok();

        let diff_text = output
            .filter(|o| !o.stdout.is_empty())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .or_else(|| {
                Command::new("git")
                    .args([
                        "diff",
                        "HEAD",
                        "--unified=3",
                        "--",
                        path.to_str().unwrap_or(""),
                    ])
                    .current_dir(&git_root)
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            });

        if let Some(text) = diff_text {
            self.lines = parse_unified_diff(&text);
        } else {
            // Fallback: compute diff in-memory (for new/untracked files)
            if let Ok(new_content) = std::fs::read_to_string(path) {
                let diff = TextDiff::from_lines("", &new_content);
                let mut old_num = 0usize;
                let mut new_num = 0usize;
                for change in diff.iter_all_changes() {
                    let (tag, content) = match change.tag() {
                        ChangeTag::Delete => (LineTag::Removed, change.value().to_string()),
                        ChangeTag::Insert => (LineTag::Added, change.value().to_string()),
                        ChangeTag::Equal => (LineTag::Context, change.value().to_string()),
                    };
                    let old = if tag == LineTag::Removed || tag == LineTag::Context {
                        old_num += 1;
                        Some(old_num)
                    } else {
                        None
                    };
                    let new = if tag == LineTag::Added || tag == LineTag::Context {
                        new_num += 1;
                        Some(new_num)
                    } else {
                        None
                    };
                    self.lines.push(DiffLine {
                        tag,
                        old_num: old,
                        new_num: new,
                        content,
                    });
                }
            }
        }
    }

    /// Stage a file (`git add`).
    pub fn stage_file(&mut self, path: &PathBuf) -> bool {
        let git_root = match &self.git_root {
            Some(r) => r.clone(),
            None => return false,
        };
        let ok = Command::new("git")
            .args(["add", "--", path.to_str().unwrap_or("")])
            .current_dir(&git_root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            if !self.staged_files.contains(path) {
                self.staged_files.push(path.clone());
            }
            self.refresh();
        }
        ok
    }

    /// Unstage a file (`git restore --staged`).
    pub fn unstage_file(&mut self, path: &PathBuf) -> bool {
        let git_root = match &self.git_root {
            Some(r) => r.clone(),
            None => return false,
        };
        let ok = Command::new("git")
            .args(["restore", "--staged", "--", path.to_str().unwrap_or("")])
            .current_dir(&git_root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            self.staged_files.retain(|p| p != path);
            self.refresh();
        }
        ok
    }

    /// Commit staged changes.
    pub fn commit(&mut self) {
        if self.commit_message.trim().is_empty() {
            return;
        }
        let git_root = match &self.git_root {
            Some(r) => r.clone(),
            None => return,
        };

        let result = Command::new("git")
            .args(["commit", "-m", &self.commit_message])
            .current_dir(&git_root)
            .output();

        match result {
            Ok(o) if o.status.success() => {
                self.commit_status = Some("✓ Committed successfully".to_string());
                self.commit_message.clear();
                self.staged_files.clear();
                self.refresh();
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                self.commit_status = Some(format!("✗ {}", stderr.trim()));
            }
            Err(e) => {
                self.commit_status = Some(format!("✗ {}", e));
            }
        }
    }

    /// Show the sidebar portion (file list + commit panel).
    /// Call this from within the left side panel.
    pub fn show(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        // Refresh every 5 seconds
        if self
            .last_refresh
            .map(|t| t.elapsed().as_secs() >= 5)
            .unwrap_or(true)
        {
            self.refresh();
        }

        ui.horizontal(|ui| {
            ui.colored_label(theme.text_secondary, "GIT");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⟳").clicked() {
                    self.refresh();
                }
            });
        });
        ui.separator();

        if self.git_root.is_none() {
            ui.colored_label(theme.text_muted, "No git repository");
            return;
        }

        self.show_file_list(ui, theme);
        ui.separator();
        self.show_commit_panel(ui, theme);
    }

    /// Show the diff view portion. Call this from the central panel.
    pub fn show_diff_central(&self, ui: &mut egui::Ui, theme: &ThemeColors) {
        self.show_diff_view(ui, theme);
    }

    /// Returns true if a file is currently selected for diffing.
    pub fn has_diff_selected(&self) -> bool {
        self.selected_file.is_some() && !self.lines.is_empty()
    }

    fn show_file_list(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.colored_label(theme.text_muted, RichText::new("CHANGES").size(11.0));
        ScrollArea::vertical()
            .id_source("diff_file_list")
            .show(ui, |ui| {
                let files = self.changed_files.clone();
                let selected = self.selected_file.clone();

                for (path, status) in &files {
                    let is_selected = selected.as_ref() == Some(path);
                    let is_staged = self.staged_files.contains(path);

                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());

                    let status_color = match status.trim() {
                        "M " | " M" | "MM" => theme.warning,
                        "A " | " A" => theme.success,
                        "D " | " D" => theme.error,
                        "??" => theme.text_muted,
                        _ => theme.text_secondary,
                    };

                    ui.horizontal(|ui| {
                        // Stage/unstage checkbox
                        let mut staged = is_staged;
                        if ui.checkbox(&mut staged, "").changed() {
                            if staged {
                                self.stage_file(path);
                            } else {
                                self.unstage_file(path);
                            }
                        }

                        let label = RichText::new(format!("{} {}", status.trim(), &name))
                            .size(12.0)
                            .color(if is_selected {
                                theme.accent
                            } else {
                                status_color
                            });
                        if ui.selectable_label(is_selected, label).clicked() {
                            let p = path.clone();
                            self.load_diff_for(&p);
                        }
                    });
                }
            });
    }

    fn show_commit_panel(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.add_space(4.0);
        ui.colored_label(theme.text_muted, RichText::new("COMMIT").size(11.0));
        ui.add_space(4.0);

        let staged_count = self.staged_files.len();
        if staged_count > 0 {
            ui.colored_label(
                theme.success,
                RichText::new(format!(
                    "{staged_count} file{} staged",
                    if staged_count == 1 { "" } else { "s" }
                ))
                .size(11.0),
            );
        } else {
            ui.colored_label(
                theme.text_muted,
                RichText::new("No files staged").size(11.0),
            );
        }

        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::multiline(&mut self.commit_message)
                .hint_text("Commit message...")
                .desired_width(ui.available_width())
                .desired_rows(3)
                .font(FontId::monospace(11.0)),
        );
        ui.add_space(4.0);

        let can_commit = staged_count > 0 && !self.commit_message.trim().is_empty();
        if ui
            .add_enabled(
                can_commit,
                egui::Button::new(RichText::new("Commit").size(12.0)),
            )
            .clicked()
        {
            self.commit();
        }

        if let Some(ref status) = self.commit_status.clone() {
            let color = if status.starts_with('✓') {
                theme.success
            } else {
                theme.error
            };
            ui.colored_label(color, RichText::new(status).size(11.0));
        }
    }

    fn show_diff_view(&self, ui: &mut egui::Ui, theme: &ThemeColors) {
        if let Some(ref path) = self.selected_file {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            ui.colored_label(
                theme.text_secondary,
                RichText::new(format!("diff: {name}")).size(12.0),
            );
            ui.separator();
        }

        if self.lines.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.colored_label(theme.text_muted, "Select a file to view its diff");
            });
            return;
        }

        ScrollArea::both().show(ui, |ui| {
            for line in &self.lines {
                let (bg, fg, prefix) = match line.tag {
                    LineTag::Added => (
                        Color32::from_rgba_premultiplied(0, 80, 0, 40),
                        Color32::from_rgb(80, 200, 80),
                        "+",
                    ),
                    LineTag::Removed => (
                        Color32::from_rgba_premultiplied(80, 0, 0, 40),
                        Color32::from_rgb(200, 80, 80),
                        "-",
                    ),
                    LineTag::Context => (Color32::TRANSPARENT, theme.text_secondary, " "),
                    LineTag::Hunk => (
                        Color32::from_rgba_premultiplied(0, 40, 80, 60),
                        theme.accent,
                        "",
                    ),
                };

                let content = line.content.trim_end_matches('\n');
                let text = if line.tag == LineTag::Hunk {
                    format!("  {content}")
                } else {
                    let old = line
                        .old_num
                        .map(|n| format!("{:4}", n))
                        .unwrap_or_else(|| "    ".to_string());
                    let new = line
                        .new_num
                        .map(|n| format!("{:4}", n))
                        .unwrap_or_else(|| "    ".to_string());
                    format!("{old} {new} {prefix} {content}")
                };

                let resp = ui.add(
                    egui::Label::new(
                        RichText::new(&text)
                            .monospace()
                            .size(11.5)
                            .color(fg)
                            .background_color(bg),
                    )
                    .wrap(),
                );
                let _ = resp;
            }
        });
    }
}

// ── Unified Diff Parser ────────────────────────────────────────────────────

fn parse_unified_diff(text: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let mut old_num: usize = 0;
    let mut new_num: usize = 0;

    for line in text.lines() {
        if line.starts_with("@@") {
            // Parse hunk header: @@ -O,o +N,n @@
            if let Some(plus) = line.find('+') {
                let after = &line[plus + 1..];
                let end = after
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(after.len());
                new_num = after[..end].parse::<usize>().unwrap_or(1).saturating_sub(1);
            }
            if let Some(minus) = line.find('-') {
                let after = &line[minus + 1..];
                let end = after
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(after.len());
                old_num = after[..end].parse::<usize>().unwrap_or(1).saturating_sub(1);
            }
            lines.push(DiffLine {
                tag: LineTag::Hunk,
                old_num: None,
                new_num: None,
                content: line.to_string(),
            });
        } else if line.starts_with('+') && !line.starts_with("+++") {
            new_num += 1;
            lines.push(DiffLine {
                tag: LineTag::Added,
                old_num: None,
                new_num: Some(new_num),
                content: line.strip_prefix('+').unwrap_or("").to_string(),
            });
        } else if line.starts_with('-') && !line.starts_with("---") {
            old_num += 1;
            lines.push(DiffLine {
                tag: LineTag::Removed,
                old_num: Some(old_num),
                new_num: None,
                content: line.strip_prefix('-').unwrap_or("").to_string(),
            });
        } else if line.starts_with(' ') {
            old_num += 1;
            new_num += 1;
            lines.push(DiffLine {
                tag: LineTag::Context,
                old_num: Some(old_num),
                new_num: Some(new_num),
                content: line.strip_prefix(' ').unwrap_or("").to_string(),
            });
        }
        // Skip diff headers (---, +++, diff --, index ...)
    }

    lines
}
