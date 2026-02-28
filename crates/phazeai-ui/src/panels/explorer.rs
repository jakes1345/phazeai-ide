use std::{collections::HashMap, path::PathBuf, sync::mpsc::channel};

use floem::{
    action::show_context_menu,
    event::{Event, EventListener},
    ext_event::create_ext_action,
    keyboard::{Key, NamedKey},
    menu::{Menu, MenuItem},
    reactive::{create_effect, create_rw_signal, RwSignal, Scope, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, Decorators},
    IntoView,
};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::{
    components::icon::{icons, phaze_icon},
    theme::PhazeTheme,
};

/// A single visible row in the file tree.
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

impl FileEntry {
    fn id(&self) -> String {
        self.path.to_string_lossy().to_string()
    }
}

/// Load the immediate children of `parent` at `depth`, sorted dirs-first.
fn load_children(parent: &PathBuf, depth: usize) -> Vec<FileEntry> {
    let Ok(rd) = std::fs::read_dir(parent) else { return vec![] };

    let mut entries: Vec<FileEntry> = rd
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            // Skip hidden files/dirs and common noise
            if name.starts_with('.') { return None; }
            if name == "target" { return None; }
            let is_dir = path.is_dir();
            Some(FileEntry { path, name, is_dir, depth, expanded: false })
        })
        .collect();

    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    entries
}

/// Rebuild the flat visible list starting from root.
/// Walks the tree respecting `expanded` flags.
fn build_visible_tree(root: &PathBuf) -> Vec<FileEntry> {
    fn walk(entries: &mut Vec<FileEntry>, dir: &PathBuf, depth: usize) {
        let children = load_children(dir, depth);
        for child in children {
            let is_dir = child.is_dir;
            let path = child.path.clone();
            entries.push(child);
            if is_dir {
                // dirs start collapsed — will expand on click
                let _ = (path, depth);
            }
        }
    }
    let mut result = Vec::new();
    walk(&mut result, root, 0);
    result
}

/// Rebuild visible tree respecting expanded state from existing entries.
fn rebuild_tree(root: &PathBuf, existing: &[FileEntry]) -> Vec<FileEntry> {
    fn collect_expanded(entries: &[FileEntry]) -> std::collections::HashSet<PathBuf> {
        entries.iter().filter(|e| e.expanded).map(|e| e.path.clone()).collect()
    }

    fn walk(
        result: &mut Vec<FileEntry>,
        dir: &PathBuf,
        depth: usize,
        expanded_set: &std::collections::HashSet<PathBuf>,
    ) {
        let children = load_children(dir, depth);
        for mut child in children {
            let child_path = child.path.clone();
            let is_dir = child.is_dir;
            child.expanded = expanded_set.contains(&child_path);
            result.push(child);
            if is_dir && expanded_set.contains(&child_path) {
                walk(result, &child_path, depth + 1, expanded_set);
            }
        }
    }

    let expanded_set = collect_expanded(existing);
    let mut result = Vec::new();
    walk(&mut result, root, 0, &expanded_set);
    result
}

/// Perform a file operation and refresh the tree.
/// Returns an error string on failure, or None on success.
fn fs_create_file(path: &PathBuf) -> Result<(), String> {
    std::fs::File::create(path).map(|_| ()).map_err(|e| e.to_string())
}

fn fs_create_dir(path: &PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())
}

fn fs_delete(path: &PathBuf) -> Result<(), String> {
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
}

/// The file-tree explorer panel.
pub fn explorer_panel(
    workspace_root: RwSignal<PathBuf>,
    open_file: RwSignal<Option<PathBuf>>,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    let entries: RwSignal<Vec<FileEntry>> = create_rw_signal(vec![]);
    let root_sig = workspace_root;

    // React to workspace root changes — rebuilds the tree whenever root changes
    create_effect(move |_| {
        let root = root_sig.get();
        entries.set(build_visible_tree(&root));
    });

    // ── Git status badges ──────────────────────────────────────────────────
    let git_status: RwSignal<HashMap<String, char>> = create_rw_signal(HashMap::new());

    // Initial fetch + re-fetch when workspace root changes
    create_effect(move |_| {
        let root = workspace_root.get();
        let scope = Scope::new();
        let send = create_ext_action(scope, move |map: HashMap<String, char>| {
            git_status.set(map);
        });
        std::thread::spawn(move || { send(fetch_git_status(&root)); });
    });

    // ── File watcher — auto-refresh tree when files change on disk ─────────
    // We use the `notify` crate to watch the workspace root recursively.
    // Events are debounced (300 ms) and delivered via a sync_channel so
    // tree rebuilds happen on the Floem UI thread via create_effect.
    {
        let root = workspace_root.get();
        // Bounded channel of size 1 — coalesces rapid bursts naturally.
        let (refresh_tx, refresh_rx) = std::sync::mpsc::sync_channel::<()>(1);

        // UI-thread side: react when the background watcher fires.
        use floem::ext_event::create_signal_from_channel;
        let refresh_sig = create_signal_from_channel(refresh_rx);
        create_effect(move |_| {
            if refresh_sig.get().is_some() {
                let r = workspace_root.get();
                let existing = entries.get();
                entries.set(rebuild_tree(&r, &existing));
            }
        });

        // Background thread: watch and debounce filesystem events.
        std::thread::spawn(move || {
            let (ev_tx, ev_rx) = channel();
            let mut watcher = match notify::recommended_watcher(
                move |res: notify::Result<notify::Event>| {
                    if let Ok(ev) = res {
                        match ev.kind {
                            EventKind::Create(_) | EventKind::Remove(_)
                            | EventKind::Modify(_) | EventKind::Any => {
                                let _ = ev_tx.send(());
                            }
                            _ => {}
                        }
                    }
                },
            ) {
                Ok(w) => w,
                Err(_) => return,
            };

            if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
                return;
            }

            // Debounce: collect events for 300 ms then fire once.
            loop {
                if ev_rx.recv().is_err() { break; }
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_millis(300);
                while std::time::Instant::now() < deadline {
                    let _ = ev_rx.recv_timeout(std::time::Duration::from_millis(50));
                }
                // try_send: skip if the previous refresh hasn't been consumed yet.
                let _ = refresh_tx.try_send(());
            }
        });
    }

    // Index of the keyboard-focused row (None = no focus)
    let focused_idx: RwSignal<Option<usize>> = create_rw_signal(None);

    let tree = dyn_stack(
        move || entries.get(),
        |entry| entry.id(),
        move |entry| {
            let indent = entry.depth as f64 * 16.0;
            let icon = if entry.is_dir {
                if entry.expanded { icons::FOLDER_OPEN } else { icons::FOLDER }
            } else {
                file_icon(&entry.name)
            };
            let name = entry.name.clone();
            let entry_path = entry.path.clone();
            let entry_path_ctx = entry.path.clone();
            let entry_path_badge = entry.path.clone();
            let is_dir = entry.is_dir;
            let is_hovered = create_rw_signal(false);

            // Calculate this entry's index in the current list
            let this_idx = {
                let list = entries.get();
                list.iter().position(|e| e.path == entry_path).unwrap_or(0)
            };

            // Git badge label (reactive — updates when git_status refreshes)
            let badge_key = entry_path_badge.to_string_lossy().to_string();
            let git_badge = label(move || {
                match git_status.get().get(&badge_key).copied() {
                    Some('M') => "M",
                    Some('A') => "A",
                    Some('D') => "D",
                    Some('?') => "?",
                    _ => "",
                }
            })
            .style({
                let badge_key2 = entry_path_badge.to_string_lossy().to_string();
                move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    let color = match git_status.get().get(&badge_key2).copied() {
                        Some('M') => p.accent,
                        Some('A') => p.success,
                        Some('D') => p.error,
                        Some('?') => p.warning,
                        _ => floem::peniko::Color::TRANSPARENT,
                    };
                    s.font_size(10.0).color(color).margin_left(4.0).width(12.0).font_weight(floem::text::Weight::BOLD)
                }
            });

            container(
                stack((
                    // Indent spacer
                    container(label(|| "")).style(move |s| s.width(indent).height_full()),
                    phaze_icon(icon, 13.0, move |p| if is_dir { p.accent } else { p.text_muted }, theme)
                    .style(move |s: floem::style::Style| {
                        s.margin_right(4.0)
                    }),
                    // Filename
                    label(move || name.clone())
                        .style(move |s| {
                            let t = theme.get();
                            let p = &t.palette;
                            s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
                        }),
                    // Git status badge
                    git_badge,
                ))
                .style(|s| s.items_center()),
            )
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let selected = open_file.get().as_ref() == Some(&entry_path);
                let hovered = is_hovered.get();
                let keyboard_focused = focused_idx.get() == Some(this_idx);
                let bg = if selected {
                    p.selection
                } else if keyboard_focused {
                    p.bg_elevated
                } else if hovered {
                    p.bg_elevated
                } else {
                    floem::peniko::Color::TRANSPARENT
                };
                s.width_full()
                 .height(22.0)
                 .background(bg)
                 .border_radius(3.0)
                 .cursor(floem::style::CursorStyle::Pointer)
                 .padding_horiz(4.0)
            })
            .on_click_stop({
                let entry_path2 = entry.path.clone();
                move |_| {
                    focused_idx.set(Some(this_idx));
                    if is_dir {
                        // Toggle expansion
                        entries.update(|list| {
                            if let Some(e) = list.iter_mut().find(|e| e.path == entry_path2) {
                                e.expanded = !e.expanded;
                            }
                            let root = root_sig.get();
                            *list = rebuild_tree(&root, list);
                        });
                    } else {
                        open_file.set(Some(entry_path2.clone()));
                    }
                }
            })
            // Right-click context menu
            .on_event_cont(EventListener::PointerDown, {
                let entry_path3 = entry_path_ctx.clone();
                let is_dir3 = entry.is_dir;
                move |event| {
                    if let Event::PointerDown(pointer_event) = event {
                        if pointer_event.button.is_secondary() {
                            focused_idx.set(Some(this_idx));
                            let path_for_menu = entry_path3.clone();
                            let entries_ref = entries;
                            let root_ref = root_sig;

                            // Determine parent dir for "New File / New Folder"
                            let parent_dir = if is_dir3 {
                                path_for_menu.clone()
                            } else {
                                path_for_menu
                                    .parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_else(|| root_ref.get())
                            };

                            // ── New File ──────────────────────────────────────
                            let pdir = parent_dir.clone();
                            let menu = Menu::new("")
                                .entry(MenuItem::new("New File").action(move || {
                                    // Create an untitled file in parent dir
                                    let new_path = find_unique_path(&pdir, "untitled", "");
                                    let _ = fs_create_file(&new_path);
                                    entries_ref.update(|list| {
                                        let root = root_ref.get();
                                        *list = rebuild_tree(&root, list);
                                    });
                                }));

                            // ── New Folder ────────────────────────────────────
                            let pdir2 = parent_dir.clone();
                            let menu = menu.entry(MenuItem::new("New Folder").action(move || {
                                let new_path = find_unique_path(&pdir2, "new_folder", "");
                                let _ = fs_create_dir(&new_path);
                                entries_ref.update(|list| {
                                    let root = root_ref.get();
                                    *list = rebuild_tree(&root, list);
                                });
                            }));

                            let menu = menu.separator();

                            // ── Delete ────────────────────────────────────────
                            let del_path = entry_path3.clone();
                            let menu = menu.entry(MenuItem::new("Delete").action(move || {
                                let _ = fs_delete(&del_path);
                                entries_ref.update(|list| {
                                    let root = root_ref.get();
                                    *list = rebuild_tree(&root, list);
                                });
                            }));

                            let menu = menu.separator();

                            // ── Copy Path ─────────────────────────────────────
                            let cp_path = entry_path3.clone();
                            let menu = menu.entry(MenuItem::new("Copy Path").action(move || {
                                let path_str = cp_path.to_string_lossy().to_string();
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    let _ = clipboard.set_text(path_str);
                                }
                            }));

                            show_context_menu(menu, None);
                        }
                    }
                }
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                is_hovered.set(true);
            })
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
                is_hovered.set(false);
            })
        },
    )
    .style(|s| s.flex_col().padding(4.0).gap(1.0));

    // Panel header
    let header = container(
        label(|| "EXPLORER")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.color(p.text_muted)
                 .font_size(11.0)
                 .font_weight(floem::text::Weight::BOLD)
            }),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding_horiz(12.0)
         .padding_vert(8.0)
         .border_bottom(1.0)
         .border_color(p.border)
         .width_full()
    });

    // Scrollable tree wrapped in a container that captures keyboard events
    let tree_scroll = scroll(tree).style(|s| s.flex_grow(1.0).min_height(0.0));

    // Outer container handles keyboard navigation for the whole panel
    let panel_body = container(tree_scroll)
        .style(|s| s.flex_grow(1.0).min_height(0.0).width_full())
        .on_event_stop(EventListener::KeyDown, move |event| {
            if let Event::KeyDown(key_event) = event {
                let list_len = entries.get().len();
                if list_len == 0 {
                    return;
                }

                match &key_event.key.logical_key {
                    // ── Arrow Up — move focus up ──────────────────────────────
                    Key::Named(NamedKey::ArrowUp) => {
                        focused_idx.update(|idx| {
                            *idx = Some(match *idx {
                                None => 0,
                                Some(0) => 0,
                                Some(i) => i - 1,
                            });
                        });
                    }
                    // ── Arrow Down — move focus down ──────────────────────────
                    Key::Named(NamedKey::ArrowDown) => {
                        focused_idx.update(|idx| {
                            *idx = Some(match *idx {
                                None => 0,
                                Some(i) => (i + 1).min(list_len - 1),
                            });
                        });
                    }
                    // ── Arrow Right — expand dir ──────────────────────────────
                    Key::Named(NamedKey::ArrowRight) => {
                        if let Some(idx) = focused_idx.get() {
                            let entry_opt = entries.get().into_iter().nth(idx);
                            if let Some(entry) = entry_opt {
                                if entry.is_dir && !entry.expanded {
                                    entries.update(|list| {
                                        if let Some(e) = list.iter_mut().find(|e| e.path == entry.path) {
                                            e.expanded = true;
                                        }
                                        let root = root_sig.get();
                                        *list = rebuild_tree(&root, list);
                                    });
                                }
                            }
                        }
                    }
                    // ── Arrow Left — collapse dir or move to parent ───────────
                    Key::Named(NamedKey::ArrowLeft) => {
                        if let Some(idx) = focused_idx.get() {
                            let entry_opt = entries.get().into_iter().nth(idx);
                            if let Some(entry) = entry_opt {
                                if entry.is_dir && entry.expanded {
                                    // Collapse this dir
                                    entries.update(|list| {
                                        if let Some(e) = list.iter_mut().find(|e| e.path == entry.path) {
                                            e.expanded = false;
                                        }
                                        let root = root_sig.get();
                                        *list = rebuild_tree(&root, list);
                                    });
                                } else if entry.depth > 0 {
                                    // Move focus to parent dir
                                    let parent = entry.path.parent().map(|p| p.to_path_buf());
                                    if let Some(parent_path) = parent {
                                        let list = entries.get();
                                        if let Some(parent_idx) = list.iter().position(|e| e.path == parent_path) {
                                            focused_idx.set(Some(parent_idx));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // ── Enter — open file or toggle dir ──────────────────────
                    Key::Named(NamedKey::Enter) => {
                        if let Some(idx) = focused_idx.get() {
                            let entry_opt = entries.get().into_iter().nth(idx);
                            if let Some(entry) = entry_opt {
                                if entry.is_dir {
                                    entries.update(|list| {
                                        if let Some(e) = list.iter_mut().find(|e| e.path == entry.path) {
                                            e.expanded = !e.expanded;
                                        }
                                        let root = root_sig.get();
                                        *list = rebuild_tree(&root, list);
                                    });
                                } else {
                                    open_file.set(Some(entry.path.clone()));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

    stack((
        header,
        panel_body,
    ))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.flex_col()
         .width_full()
         .height_full()
         .background(p.bg_panel)
    })
}

/// Find a unique file/dir path like `name`, `name_1`, `name_2`, etc.
/// `ext` should be empty string for no extension, or e.g. `"rs"` for Rust files.
fn find_unique_path(dir: &PathBuf, base: &str, ext: &str) -> PathBuf {
    let make_path = |suffix: &str| {
        let fname = if ext.is_empty() {
            format!("{}{}", base, suffix)
        } else {
            format!("{}{}.{}", base, suffix, ext)
        };
        dir.join(fname)
    };

    let candidate = make_path("");
    if !candidate.exists() {
        return candidate;
    }

    for i in 1..=999 {
        let candidate = make_path(&format!("_{}", i));
        if !candidate.exists() {
            return candidate;
        }
    }

    make_path("_new")
}

/// Run `git status --porcelain` in the given directory and return a map of
/// absolute path string → status char (M=modified, A=added, D=deleted, ?=untracked).
fn fetch_git_status(root: &PathBuf) -> HashMap<String, char> {
    let mut map = HashMap::new();
    let Ok(out) = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
    else {
        return map;
    };
    if !out.status.success() { return map; }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.len() < 4 { continue; }
        let xy = &line[..2];
        let rel = line[3..].trim();
        // Handle rename format "old -> new"
        let rel = if let Some(arrow) = rel.find(" -> ") { &rel[arrow + 4..] } else { rel };
        let status = if xy.contains('M') { 'M' }
            else if xy.starts_with('A') { 'A' }
            else if xy.contains('D') { 'D' }
            else if xy == "??" { '?' }
            else { continue };
        let abs = root.join(rel);
        map.insert(abs.to_string_lossy().to_string(), status);
    }
    map
}

/// Pick a Nerd Font icon based on file extension.
fn file_icon(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "rs" => icons::FILE_RUST,
        "py" => icons::FILE_PYTHON,
        "js" | "mjs" | "cjs" => icons::FILE_JS,
        "ts" | "tsx" => icons::FILE_TS,
        "json" => icons::FILE_JSON,
        "toml" => icons::FILE_TOML,
        "md" | "mdx" => icons::FILE_MARKDOWN,
        "lock" => icons::FILE_LOCK,
        _ => icons::FILE,
    }
}
