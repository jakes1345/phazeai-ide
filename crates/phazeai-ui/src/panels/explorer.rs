use std::{collections::HashMap, path::PathBuf, sync::mpsc::channel};

use floem::{
    action::show_context_menu,
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, NamedKey},
    menu::{Menu, MenuItem},
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, label, scroll, stack, Decorators},
    IntoView,
};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::{
    components::icon::{icons, phaze_icon},
    theme::PhazeTheme,
    util::safe_get,
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
    let Ok(rd) = std::fs::read_dir(parent) else {
        return vec![];
    };

    let mut entries: Vec<FileEntry> = rd
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            // Skip hidden files/dirs and common noise
            if name.starts_with('.') {
                return None;
            }
            if name == "target" {
                return None;
            }
            let is_dir = path.is_dir();
            Some(FileEntry {
                path,
                name,
                is_dir,
                depth,
                expanded: false,
            })
        })
        .collect();

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
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
        entries
            .iter()
            .filter(|e| e.expanded)
            .map(|e| e.path.clone())
            .collect()
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
    std::fs::File::create(path)
        .map(|_| ())
        .map_err(|e| e.to_string())
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
    open_tabs: RwSignal<Vec<PathBuf>>,
) -> impl IntoView {
    // ── Open Editors section state ─────────────────────────────────────────
    let open_editors_expanded: RwSignal<bool> = create_rw_signal(true);
    let entries: RwSignal<Vec<FileEntry>> = create_rw_signal(vec![]);
    let root_sig = workspace_root;

    // React to workspace root changes — rebuilds the tree whenever root changes
    create_effect(move |_| {
        let root = root_sig.get();
        entries.set(build_visible_tree(&root));
    });

    // ── Git status badges ──────────────────────────────────────────────────
    let git_status: RwSignal<HashMap<String, char>> = create_rw_signal(HashMap::new());

    // Initial fetch + re-fetch when workspace root changes.
    // Use a sync_channel + create_signal_from_channel to avoid creating
    // Scope::new() per-invocation (which leaks since scopes are never disposed).
    {
        let (git_tx, git_rx) = std::sync::mpsc::sync_channel::<HashMap<String, char>>(1);
        let git_result_sig = create_signal_from_channel(git_rx);
        create_effect(move |_| {
            if let Some(map) = git_result_sig.get() {
                git_status.set(map);
            }
        });
        create_effect(move |_| {
            let root = workspace_root.get();
            let tx = git_tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(fetch_git_status(&root));
            });
        });
    }

    // ── File watcher — auto-refresh tree when files change on disk ─────────
    // We use the `notify` crate to watch the workspace root recursively.
    // Events are debounced (300 ms) and delivered via a sync_channel so
    // tree rebuilds happen on the Floem UI thread via create_effect.
    {
        let root = workspace_root.get();
        // Bounded channel of size 1 — coalesces rapid bursts naturally.
        let (refresh_tx, refresh_rx) = std::sync::mpsc::sync_channel::<()>(1);

        // UI-thread side: react when the background watcher fires.
        // Use get_untracked() for entries to avoid subscribing this effect
        // to entries changes (which would cause it to re-run on every tree
        // mutation, not just file-watcher events).
        let refresh_sig = create_signal_from_channel(refresh_rx);
        create_effect(move |_| {
            if refresh_sig.get().is_some() {
                let r = workspace_root.get_untracked();
                let existing = entries.get_untracked();
                entries.set(rebuild_tree(&r, &existing));
            }
        });

        // Background thread: watch and debounce filesystem events.
        std::thread::spawn(move || {
            let (ev_tx, ev_rx) = channel();
            let mut watcher =
                match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                    if let Ok(ev) = res {
                        match ev.kind {
                            EventKind::Create(_)
                            | EventKind::Remove(_)
                            | EventKind::Modify(_)
                            | EventKind::Any => {
                                let _ = ev_tx.send(());
                            }
                            _ => {}
                        }
                    }
                }) {
                    Ok(w) => w,
                    Err(_) => return,
                };

            if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
                return;
            }

            // Debounce: collect events for 300 ms then fire once.
            loop {
                if ev_rx.recv().is_err() {
                    break;
                }
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
                while std::time::Instant::now() < deadline {
                    let _ = ev_rx.recv_timeout(std::time::Duration::from_millis(50));
                }
                // try_send: skip if the previous refresh hasn't been consumed yet.
                // Break on disconnect so thread doesn't leak.
                if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) = refresh_tx.try_send(()) {
                    break;
                }
            }
        });
    }

    // ── Periodic git status refresh (every 5 seconds) ─────────────────────
    // A tick thread sends () every 5s. The effect reads the tick, grabs the
    // current workspace root (on UI thread), and spawns a one-shot fetch.
    // Results come back via a second channel. No Scope::new() leak.
    {
        let (tick_tx, tick_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let tick_sig = create_signal_from_channel(tick_rx);

        let (status_tx, status_rx) = std::sync::mpsc::sync_channel::<HashMap<String, char>>(1);
        let periodic_result_sig = create_signal_from_channel(status_rx);

        // Apply periodic results to git_status
        create_effect(move |_| {
            if let Some(map) = periodic_result_sig.get() {
                git_status.set(map);
            }
        });

        // When tick fires, read root on UI thread & spawn fetch
        create_effect(move |_| {
            tick_sig.get(); // re-run every 5s tick
            let root = workspace_root.get_untracked();
            let tx = status_tx.clone();
            std::thread::spawn(move || {
                let _ = tx.try_send(fetch_git_status(&root));
            });
        });

        // Background tick thread
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            match tick_tx.try_send(()) {
                Ok(_) => {}
                Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
            }
        });
    }

    // ── Reveal active file nonce — bumped to trigger the expand effect ─────
    let reveal_nonce: RwSignal<u32> = create_rw_signal(0u32);

    // ── Reveal-active-file: expand parent dirs when open_file changes ──────
    {
        let entries_for_reveal = entries;
        let root_for_reveal = workspace_root;
        create_effect(move |_| {
            let _nonce = reveal_nonce.get(); // also triggered by Locate button
            let Some(active_path) = open_file.get() else {
                return;
            };
            let workspace = root_for_reveal.get_untracked();

            // Collect all ancestor dirs between active_path and workspace root.
            let mut ancestors: Vec<PathBuf> = Vec::new();
            let mut cur = active_path.parent().map(|p| p.to_path_buf());
            while let Some(dir) = cur {
                if dir == workspace {
                    break;
                }
                ancestors.push(dir.clone());
                cur = dir.parent().map(|p| p.to_path_buf());
            }

            if ancestors.is_empty() {
                return;
            }

            // Mark each ancestor as expanded and rebuild the tree.
            entries_for_reveal.update(|list| {
                let mut changed = false;
                for entry in list.iter_mut() {
                    if entry.is_dir && ancestors.contains(&entry.path) && !entry.expanded {
                        entry.expanded = true;
                        changed = true;
                    }
                }
                if changed {
                    let root = root_for_reveal.get_untracked();
                    *list = rebuild_tree(&root, list);
                }
            });
        });
    }

    // Index of the keyboard-focused row (None = no focus)
    let focused_idx: RwSignal<Option<usize>> = create_rw_signal(None);

    let tree = dyn_stack(
        move || safe_get(entries, Vec::new()),
        |entry| entry.id(),
        move |entry| {
            let indent = entry.depth as f64 * 16.0;
            let icon = if entry.is_dir {
                if entry.expanded {
                    icons::FOLDER_OPEN
                } else {
                    icons::FOLDER
                }
            } else {
                file_icon(&entry.name)
            };
            let name = entry.name.clone();
            let entry_path = entry.path.clone();
            let entry_path_ctx = entry.path.clone();
            let entry_path_badge = entry.path.clone();
            let is_dir = entry.is_dir;
            let is_hovered = create_rw_signal(false);

            // Calculate this entry's index in the current list.
            // CRITICAL: use get_untracked() — NOT get() — to avoid subscribing
            // each row view to the `entries` signal. dyn_stack's items_fn already
            // subscribes; a second subscription inside view_fn causes an infinite
            // re-render cascade (~70 MB/sec memory growth).
            let this_idx = {
                let list = entries.get_untracked();
                list.iter().position(|e| e.path == entry_path).unwrap_or(0)
            };

            // Git badge label (reactive — updates when git_status refreshes)
            let badge_key = entry_path_badge.to_string_lossy().to_string();
            let git_badge = label(move || match git_status.get().get(&badge_key).copied() {
                Some('M') => "M",
                Some('A') => "A",
                Some('D') => "D",
                Some('?') => "?",
                _ => "",
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
                    s.font_size(10.0)
                        .color(color)
                        .margin_left(4.0)
                        .width(12.0)
                        .font_weight(floem::text::Weight::BOLD)
                }
            });

            container(
                stack((
                    // Indent spacer
                    container(label(|| "")).style(move |s| s.width(indent).height_full()),
                    phaze_icon(
                        icon,
                        13.0,
                        move |p| if is_dir { p.accent } else { p.text_muted },
                        theme,
                    )
                    .style(move |s: floem::style::Style| s.margin_right(4.0)),
                    // Filename
                    label(move || name.clone()).style(move |s| {
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
                } else if keyboard_focused || hovered {
                    p.bg_elevated
                } else {
                    floem::peniko::Color::TRANSPARENT
                };
                // min_width(0) + no width_full() lets rows extend beyond scroll
                // viewport width, enabling horizontal scrolling.
                s.min_width(0.0)
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
                            let menu =
                                Menu::new("").entry(MenuItem::new("New File").action(move || {
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

                            // ── Copy Absolute Path ────────────────────────────
                            let abs_path = entry_path3.clone();
                            let menu =
                                menu.entry(MenuItem::new("Copy Absolute Path").action(move || {
                                    let path_str = if let Ok(abs) = abs_path.canonicalize() {
                                        abs.to_string_lossy().to_string()
                                    } else {
                                        abs_path.to_string_lossy().to_string()
                                    };
                                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                        let _ = clipboard.set_text(path_str);
                                    }
                                }));

                            // ── Copy Relative Path ────────────────────────────
                            let rel_path_entry = entry_path3.clone();
                            let menu =
                                menu.entry(MenuItem::new("Copy Relative Path").action(move || {
                                    let root = root_ref.get();
                                    let rel = rel_path_entry
                                        .strip_prefix(&root)
                                        .map(|r| r.to_string_lossy().to_string())
                                        .unwrap_or_else(|_| {
                                            rel_path_entry.to_string_lossy().to_string()
                                        });
                                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                        let _ = clipboard.set_text(rel);
                                    }
                                }));

                            let menu = menu.separator();

                            // ── Duplicate (files only) ────────────────────────
                            let menu = if !is_dir3 {
                                let dup_path = entry_path3.clone();
                                let stem = dup_path
                                    .file_stem()
                                    .map(|s| s.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "file".to_string());
                                let ext_str = dup_path
                                    .extension()
                                    .map(|e| e.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                let dup_dir = dup_path
                                    .parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_else(|| root_ref.get());
                                menu.entry(MenuItem::new("Duplicate").action(move || {
                                    let new_path = find_unique_path(
                                        &dup_dir,
                                        &format!("{}_copy", stem),
                                        &ext_str,
                                    );
                                    let _ = std::fs::copy(&dup_path, &new_path);
                                    entries_ref.update(|list| {
                                        let root = root_ref.get();
                                        *list = rebuild_tree(&root, list);
                                    });
                                }))
                            } else {
                                menu
                            };

                            // ── Reveal in File Manager ────────────────────────
                            let reveal_path = entry_path3.clone();
                            let menu = menu.entry(MenuItem::new("Reveal in File Manager").action(
                                move || {
                                    let dir = if reveal_path.is_dir() {
                                        reveal_path.clone()
                                    } else {
                                        reveal_path
                                            .parent()
                                            .map(|p| p.to_path_buf())
                                            .unwrap_or_else(|| reveal_path.clone())
                                    };
                                    #[cfg(target_os = "linux")]
                                    let _ =
                                        std::process::Command::new("xdg-open").arg(&dir).spawn();
                                    #[cfg(target_os = "macos")]
                                    let _ = std::process::Command::new("open").arg(&dir).spawn();
                                    #[cfg(target_os = "windows")]
                                    let _ =
                                        std::process::Command::new("explorer").arg(&dir).spawn();
                                },
                            ));

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

    // Panel header — EXPLORER label + action buttons (Collapse All, Locate)
    let header = {
        let title = label(|| "EXPLORER").style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.color(p.text_muted)
                .font_size(11.0)
                .font_weight(floem::text::Weight::BOLD)
                .flex_grow(1.0)
        });

        // Collapse All button — collapses all expanded directories
        let collapse_btn = container(label(|| "\u{229F}")) // ⊟
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(13.0)
                    .padding_horiz(5.0)
                    .padding_vert(3.0)
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .color(p.text_muted)
                    .hover(|s| s.background(p.bg_elevated))
            })
            .on_click_stop(move |_| {
                entries.update(|list| {
                    for e in list.iter_mut() {
                        e.expanded = false;
                    }
                    let root = root_sig.get();
                    *list = rebuild_tree(&root, list);
                });
            });

        // Locate (reveal active file) button
        let locate_btn = container(label(|| "\u{2299}")) // ⊙
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(13.0)
                    .padding_horiz(5.0)
                    .padding_vert(3.0)
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
                    .color(p.text_muted)
                    .hover(|s| s.background(p.bg_elevated))
            })
            .on_click_stop(move |_| {
                reveal_nonce.update(|v| *v += 1);
            });

        container(stack((title, collapse_btn, locate_btn)).style(|s| s.items_center())).style(
            move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.padding_horiz(8.0)
                    .padding_vert(6.0)
                    .border_bottom(1.0)
                    .border_color(p.border)
                    .width_full()
            },
        )
    };

    // Scrollable tree wrapped in a container that captures keyboard events.
    // The scroll view allows both vertical and horizontal scrolling — rows are
    // sized by content width (not clamped to viewport) so deep paths scroll.
    let tree_scroll = scroll(tree).style(|s| s.flex_grow(1.0).min_height(0.0).min_width(0.0));

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
                                        if let Some(e) =
                                            list.iter_mut().find(|e| e.path == entry.path)
                                        {
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
                                        if let Some(e) =
                                            list.iter_mut().find(|e| e.path == entry.path)
                                        {
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
                                        if let Some(parent_idx) =
                                            list.iter().position(|e| e.path == parent_path)
                                        {
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
                                        if let Some(e) =
                                            list.iter_mut().find(|e| e.path == entry.path)
                                        {
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

    // ── Open Editors section ───────────────────────────────────────────────
    let open_editors_section = {
        // Section header row
        let oe_header = container(
            stack((
                // Toggle arrow
                label(move || {
                    if open_editors_expanded.get() {
                        "▾"
                    } else {
                        "▸"
                    }
                })
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.font_size(10.0).color(p.text_muted).margin_right(4.0)
                }),
                // Title with count
                label(move || {
                    let n = open_tabs.get().len();
                    format!("OPEN EDITORS ({})", n)
                })
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.font_size(11.0)
                        .color(p.text_muted)
                        .font_weight(floem::text::Weight::BOLD)
                        .flex_grow(1.0)
                }),
            ))
            .style(|s| s.items_center()),
        )
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.padding_horiz(8.0)
                .padding_vert(4.0)
                .width_full()
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(p.bg_elevated))
        })
        .on_click_stop(move |_| {
            open_editors_expanded.update(|v| *v = !*v);
        });

        // Rows for each open tab
        let oe_rows = dyn_stack(
            move || safe_get(open_tabs, Vec::new()).into_iter().enumerate().collect::<Vec<_>>(),
            |(i, _)| *i,
            move |(_, tab_path)| {
                let filename = tab_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| tab_path.to_string_lossy().to_string());
                let tab_path_click = tab_path.clone();
                let tab_path_close = tab_path.clone();
                let tab_path_active = tab_path.clone();
                let is_row_hovered = create_rw_signal(false);

                container(
                    stack((
                        // Indent spacer
                        container(label(|| "")).style(|s| s.width(20.0)),
                        // Filename label
                        label(move || filename.clone()).style(move |s| {
                            let t = theme.get();
                            let p = &t.palette;
                            let is_active = open_file.get().as_ref() == Some(&tab_path_active);
                            s.font_size(13.0)
                                .color(if is_active { p.accent } else { p.text_primary })
                                .flex_grow(1.0)
                        }),
                        // Close button (×) — only visible on hover
                        container(label(|| "\u{00D7}"))
                            .style(move |s| {
                                let t = theme.get();
                                let p = &t.palette;
                                s.font_size(12.0)
                                    .color(p.text_muted)
                                    .padding_horiz(4.0)
                                    .border_radius(3.0)
                                    .cursor(floem::style::CursorStyle::Pointer)
                                    .apply_if(!is_row_hovered.get(), |s| {
                                        s.display(floem::style::Display::None)
                                    })
                                    .hover(|s| s.background(p.bg_elevated))
                            })
                            .on_click_stop(move |_| {
                                open_tabs.update(|list| {
                                    list.retain(|p| p != &tab_path_close);
                                });
                            }),
                    ))
                    .style(|s| s.items_center()),
                )
                .style(move |s| {
                    let t = theme.get();
                    let p = &t.palette;
                    s.height(22.0)
                        .padding_horiz(4.0)
                        .border_radius(3.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                        .apply_if(is_row_hovered.get(), |s| s.background(p.bg_elevated))
                })
                .on_click_stop(move |_| {
                    open_file.set(Some(tab_path_click.clone()));
                })
                .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                    is_row_hovered.set(true);
                })
                .on_event_stop(
                    floem::event::EventListener::PointerLeave,
                    move |_| {
                        is_row_hovered.set(false);
                    },
                )
            },
        )
        .style(move |s| {
            s.flex_col()
                .padding_horiz(4.0)
                .gap(1.0)
                .apply_if(!open_editors_expanded.get(), |s| {
                    s.display(floem::style::Display::None)
                })
        });

        // Border separator below the section
        container(stack((oe_header, oe_rows)).style(|s| s.flex_col())).style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.width_full().border_bottom(1.0).border_color(p.border)
        })
    };

    stack((header, open_editors_section, panel_body)).style(move |s| {
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
fn find_unique_path(dir: &std::path::Path, base: &str, ext: &str) -> PathBuf {
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
    if !out.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let xy = &line[..2];
        let rel = line[3..].trim();
        // Handle rename format "old -> new"
        let rel = if let Some(arrow) = rel.find(" -> ") {
            &rel[arrow + 4..]
        } else {
            rel
        };
        let status = if xy.contains('M') {
            'M'
        } else if xy.starts_with('A') {
            'A'
        } else if xy.contains('D') {
            'D'
        } else if xy == "??" {
            '?'
        } else {
            continue;
        };
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
