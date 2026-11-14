//! Floating overlays: file picker, command palette, completions popup,
//! Ctrl+K inline AI edit, hover tooltip, code actions, rename, signature
//! help, toast, workspace symbols, branch picker, vim ex bar, goto, peek
//! definition. Extracted from `app.rs` as part of the Session 11 split.

use floem::{
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, NamedKey},
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate, SignalWith},
    views::{container, dyn_stack, empty, label, scroll, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::constants::ui as ui_const;
use phazeai_core::Settings;

use crate::components::FocusWhenOpen;
use crate::domain_state::IdeState;
use crate::lsp_bridge::{CodeAction, CompletionEntry, LspCommand};
use crate::util::safe_get;

use super::ex_command::{run_ex_command, EditorPaneHandle};
use super::{all_commands, request_quit, show_toast, Tab};

/// Most files the picker keeps; beyond this the list is cut, shallowest first.
const FILE_PICKER_LIMIT: usize = 2000;

/// Directories the file picker never lists or descends into. Skipping them
/// while walking (rather than filtering their contents afterwards) matters:
/// `target/` and `node_modules/` hold hundreds of thousands of files, and
/// walking them first left the picker on "Searching workspace files…" for a
/// long time.
fn is_picker_skipped_dir(name: &str) -> bool {
    matches!(name, "target" | ".git" | "node_modules" | ".cache")
}

/// Files under `root` for the file picker: at most `limit`, shallowest and
/// alphabetically first, without dot-files or the skipped directories.
fn list_workspace_files(root: &std::path::Path, limit: usize) -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(root)
        .max_depth(10)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !(e.file_type().is_dir()
                    && is_picker_skipped_dir(&e.file_name().to_string_lossy()))
        })
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.into_path())
        .collect();
    // Shallow files first so the 2000-file cut keeps the ones people open.
    files.sort_by_key(|p| p.components().count());
    files.truncate(limit);
    files
}

#[cfg(test)]
mod picker_walk_tests {
    use super::list_workspace_files;

    fn touch(root: &std::path::Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
    }

    #[test]
    fn skips_build_and_vcs_directories_but_keeps_dot_directories_with_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for f in [
            "a.rs",
            "src/b.rs",
            ".github/workflows/ci.yml",
            ".env",
            "target/debug/huge.rlib",
            "node_modules/pkg/index.js",
            ".git/config",
            "web/node_modules/pkg/x.js",
            ".cache/data",
        ] {
            touch(root, f);
        }
        let names: Vec<String> = list_workspace_files(root, 100)
            .into_iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, vec!["a.rs", "src/b.rs", ".github/workflows/ci.yml"]);
    }

    #[test]
    fn the_cut_keeps_the_shallowest_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(root, "deep/er/est/z.rs");
        touch(root, "top.rs");
        touch(root, "mid/m.rs");
        let names: Vec<String> = list_workspace_files(root, 2)
            .into_iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, vec!["top.rs", "mid/m.rs"]);
    }
}

// ── File picker overlay (Ctrl+P) ──────────────────────────────────────────────

/// Identity of a file-picker row.
///
/// A row bakes in its file name when its view is created, and the stack only
/// creates views for keys it has not seen. The position alone is therefore not
/// enough: typing a filter keeps positions 0..n alive while the files at them
/// change, which left the old names on screen (and a click opening them) next
/// to a query that matched other files.
fn picker_row_key(row: &(usize, std::path::PathBuf)) -> (usize, std::path::PathBuf) {
    (row.0, row.1.clone())
}

pub(crate) fn file_picker(state: IdeState) -> impl IntoView {
    let query = state.workbench.file_picker_query;
    let all_files = state.workbench.file_picker_files;
    let hovered: RwSignal<Option<usize>> = create_rw_signal(None);

    let last_root: RwSignal<Option<std::path::PathBuf>> = create_rw_signal(None);
    let (files_tx, files_rx) = std::sync::mpsc::sync_channel::<Vec<std::path::PathBuf>>(1);
    let files_sig = create_signal_from_channel(files_rx);
    create_effect(move |_| {
        if let Some(files) = files_sig.get() {
            all_files.set(files);
        }
    });
    create_effect(move |_| {
        if !state.workbench.file_picker_open.get() {
            return;
        }
        let root = state.project.workspace_root.get();
        if last_root.get().as_ref() == Some(&root) {
            return;
        }
        last_root.set(Some(root.clone()));
        let tx = files_tx.clone();
        std::thread::spawn(move || {
            let files = list_workspace_files(&root, FILE_PICKER_LIMIT);
            let _ = tx.send(files);
        });
    });

    let filtered = move || -> Vec<(usize, std::path::PathBuf)> {
        let q = query.get().to_lowercase();
        all_files
            .get()
            .into_iter()
            .filter(|p| {
                if q.is_empty() {
                    return true;
                }
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                name.contains(&q) || p.to_string_lossy().to_lowercase().contains(&q)
            })
            .take(50)
            .enumerate()
            .collect()
    };

    let picker_open = state.workbench.file_picker_open;
    let search_box = text_input(query)
        .focus_when_open(move || picker_open.get())
        .on_enter({
            let state = state.clone();
            move || {
                // Enter opens the top match, exactly like clicking the first row.
                if let Some((_, path)) = filtered().into_iter().next() {
                    state.editor.open_file.set(Some(path));
                    state.workbench.file_picker_open.set(false);
                    state.workbench.file_picker_query.set(String::new());
                }
            }
        })
        .style(move |s| {
            let t = state.workbench.theme.get();
            let p = &t.palette;
            s.width_full()
                .padding(10.0)
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_elevated)
                .border(1.0)
                .border_color(p.border_focus)
                .border_radius(6.0)
                .margin_bottom(8.0)
        });

    let items_view = scroll(
        dyn_stack(filtered, picker_row_key, {
            let state = state.clone();
            move |(idx, path)| {
                let path_clone = path.clone();
                let root = state.project.workspace_root.get();
                let display = path
                    .strip_prefix(&root)
                    .ok()
                    .map(|r| r.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());
                let display2 = display.clone();
                let hov = hovered;
                let state = state.clone();
                container(
                    stack((
                        label(move || {
                            path_clone
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default()
                        })
                        .style({
                            let state = state.clone();
                            move |s| {
                                s.font_size(13.0)
                                    .color(state.workbench.theme.get().palette.text_primary)
                            }
                        }),
                        label(move || format!("  {}", display2)).style({
                            let state = state.clone();
                            move |s| {
                                s.font_size(11.0)
                                    .color(state.workbench.theme.get().palette.text_muted)
                                    .flex_grow(1.0)
                            }
                        }),
                    ))
                    .style(|s| s.items_center()),
                )
                .style({
                    let state = state.clone();
                    move |s| {
                        let t = state.workbench.theme.get();
                        let p = &t.palette;
                        s.width_full()
                            .padding_horiz(12.0)
                            .padding_vert(7.0)
                            .border_radius(4.0)
                            .background(if hov.get() == Some(idx) {
                                p.bg_elevated
                            } else {
                                floem::peniko::Color::TRANSPARENT
                            })
                            .cursor(floem::style::CursorStyle::Pointer)
                    }
                })
                .on_click_stop({
                    let state = state.clone();
                    let path2 = path.clone();
                    move |_| {
                        state.editor.open_file.set(Some(path2.clone()));
                        state.workbench.file_picker_open.set(false);
                        state.workbench.file_picker_query.set(String::new());
                    }
                })
                .on_event_stop(EventListener::PointerEnter, move |_| {
                    hov.set(Some(idx));
                })
                .on_event_stop(EventListener::PointerLeave, move |_| {
                    hov.set(None);
                })
            }
        })
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(360.0));

    let empty_hint = container(label(|| "Searching workspace files…").style(move |s| {
        s.font_size(12.0)
            .color(state.workbench.theme.get().palette.text_muted)
    }))
    .style(move |s| {
        let empty = all_files.get().is_empty() && state.workbench.file_picker_open.get();
        s.width_full()
            .padding_vert(12.0)
            .items_center()
            .justify_center()
            .apply_if(!empty, |s| s.display(floem::style::Display::None))
    });

    let picker_box = stack((search_box, empty_hint, items_view))
        .style({
            let state = state.clone();
            move |s| {
                let t = state.workbench.theme.get();
                let p = &t.palette;
                s.flex_col()
                    .width(560.0)
                    .padding(16.0)
                    .background(p.bg_panel)
                    .border(1.0)
                    .border_color(p.glass_border)
                    .border_radius(10.0)
                    .box_shadow_h_offset(0.0)
                    .box_shadow_v_offset(4.0)
                    .box_shadow_blur(40.0)
                    .box_shadow_color(p.glow)
                    .box_shadow_spread(0.0)
            }
        })
        .on_event_stop(EventListener::KeyDown, {
            let state = state.clone();
            move |event| {
                if let Event::KeyDown(e) = event {
                    if e.key.logical_key == Key::Named(NamedKey::Escape) {
                        state.workbench.file_picker_open.set(false);
                        state.workbench.file_picker_query.set(String::new());
                    }
                }
            }
        });

    container(picker_box)
        .style({
            let state = state.clone();
            move |s| {
                let shown = state.workbench.file_picker_open.get();
                s.absolute()
                    .inset(0)
                    .items_start()
                    .justify_center()
                    .padding_top(80.0)
                    .background(state.workbench.theme.get().palette.overlay_bg)
                    .z_index(ui_const::Z_FILE_PICKER)
                    .apply_if(!shown, |s| s.display(floem::style::Display::None))
            }
        })
        .on_click_stop({
            let state = state.clone();
            move |_| {
                state.workbench.file_picker_open.set(false);
                state.workbench.file_picker_query.set(String::new());
            }
        })
}

// ── Command palette overlay ───────────────────────────────────────────────────

pub(crate) fn command_palette(state: IdeState) -> impl IntoView {
    let query = state.workbench.command_palette_query;

    #[allow(clippy::type_complexity)]
    let commands_list = move || -> Vec<(usize, &'static str, fn(IdeState))> {
        let q = query.get().to_lowercase();
        all_commands()
            .into_iter()
            .enumerate()
            .filter(|(_, cmd)| q.is_empty() || cmd.label.to_lowercase().contains(&q))
            .map(|(idx, cmd)| (idx, cmd.label, cmd.action))
            .collect()
    };

    let row_hovered: RwSignal<Option<usize>> = create_rw_signal(None);

    let palette_open = state.workbench.command_palette_open;
    let search_box = text_input(query)
        .focus_when_open(move || palette_open.get())
        .on_enter({
            let state = state.clone();
            move || {
                // Enter runs the top match, exactly like clicking the first row.
                if let Some((_, _, action)) = commands_list().into_iter().next() {
                    action(state.clone());
                    state.workbench.command_palette_open.set(false);
                    state.workbench.command_palette_query.set(String::new());
                }
            }
        })
        .style(move |s| {
            let t = state.workbench.theme.get();
            let p = &t.palette;
            s.width_full()
                .padding(10.0)
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_elevated)
                .border(1.0)
                .border_color(p.border_focus)
                .border_radius(6.0)
                .margin_bottom(8.0)
        });

    let items_view = scroll(
        dyn_stack(commands_list, |(idx, lbl, _action)| (*idx, *lbl), {
            let state = state.clone();
            move |(idx, cmd_label, cmd_action)| {
                let hovered = row_hovered;
                let state = state.clone();
                container(label(move || cmd_label).style({
                    let state = state.clone();
                    move |s| {
                        s.font_size(13.0)
                            .color(state.workbench.theme.get().palette.text_primary)
                    }
                }))
                .style({
                    let state = state.clone();
                    move |s| {
                        let t = state.workbench.theme.get();
                        let p = &t.palette;
                        let is_hov = hovered.get() == Some(idx);
                        s.width_full()
                            .padding_horiz(12.0)
                            .padding_vert(8.0)
                            .border_radius(4.0)
                            .background(if is_hov {
                                p.bg_elevated
                            } else {
                                floem::peniko::Color::TRANSPARENT
                            })
                            .cursor(floem::style::CursorStyle::Pointer)
                    }
                })
                .on_click_stop({
                    let state = state.clone();
                    move |_| {
                        cmd_action(state.clone());
                        state.workbench.command_palette_open.set(false);
                        state.workbench.command_palette_query.set(String::new());
                    }
                })
                .on_event_stop(EventListener::PointerEnter, move |_| {
                    hovered.set(Some(idx));
                })
                .on_event_stop(EventListener::PointerLeave, move |_| {
                    hovered.set(None);
                })
            }
        })
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(320.0));

    let palette_box = stack((search_box, items_view))
        .style({
            let state = state.clone();
            move |s| {
                let t = state.workbench.theme.get();
                let p = &t.palette;
                s.flex_col()
                    .width(500.0)
                    .padding(16.0)
                    .background(p.bg_panel)
                    .border(1.0)
                    .border_color(p.glass_border)
                    .border_radius(10.0)
                    .box_shadow_h_offset(0.0)
                    .box_shadow_v_offset(4.0)
                    .box_shadow_blur(40.0)
                    .box_shadow_color(p.glow)
                    .box_shadow_spread(0.0)
            }
        })
        .on_event_stop(EventListener::KeyDown, {
            let state = state.clone();
            move |event| {
                if let Event::KeyDown(e) = event {
                    if e.key.logical_key == Key::Named(NamedKey::Escape) {
                        state.workbench.command_palette_open.set(false);
                        state.workbench.command_palette_query.set(String::new());
                    }
                }
            }
        });

    container(palette_box)
        .style({
            let state = state.clone();
            move |s| {
                let shown = state.workbench.command_palette_open.get();
                s.absolute()
                    .inset(0)
                    .items_center()
                    .justify_center()
                    .background(state.workbench.theme.get().palette.overlay_bg)
                    .z_index(ui_const::Z_COMMAND_PALETTE)
                    .apply_if(!shown, |s| s.display(floem::style::Display::None))
            }
        })
        .on_click_stop({
            let state = state.clone();
            move |_| {
                state.workbench.command_palette_open.set(false);
                state.workbench.command_palette_query.set(String::new());
            }
        })
}

// ── Completion popup ──────────────────────────────────────────────────────────

pub(crate) fn completion_popup(state: IdeState) -> impl IntoView {
    let items = state.editor.completions;
    let selected = state.editor.completion_selected;
    let filter = state.editor.completion_filter_text;

    let filtered_items = move || -> Vec<(usize, CompletionEntry)> {
        let f = filter.get().to_lowercase();
        items
            .get()
            .into_iter()
            .enumerate()
            .filter(|(_, e)| f.is_empty() || e.label.to_lowercase().contains(&f))
            .collect()
    };

    let list = scroll(
        dyn_stack(filtered_items, |(idx, _)| *idx, {
            let state = state.clone();
            move |(idx, entry): (usize, CompletionEntry)| {
                let is_sel = move || selected.get() == idx;
                let item_detail = entry.detail.clone().unwrap_or_default();
                let item_label = entry.label.clone();
                stack((
                    label(move || item_label.clone()).style(move |s: floem::style::Style| {
                        let p = state.workbench.theme.get().palette;
                        s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
                    }),
                    label(move || item_detail.clone()).style(move |s: floem::style::Style| {
                        let p = state.workbench.theme.get().palette;
                        s.font_size(11.0).color(p.text_muted).margin_left(8.0)
                    }),
                ))
                .style(move |s: floem::style::Style| {
                    let p = state.workbench.theme.get().palette;
                    s.items_center()
                        .width_full()
                        .padding_horiz(12.0)
                        .padding_vert(5.0)
                        .border_radius(4.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                        .background(if is_sel() {
                            p.accent_dim
                        } else {
                            floem::peniko::Color::TRANSPARENT
                        })
                })
                .on_click_stop({
                    let state = state.clone();
                    move |_| {
                        let items = state.editor.completions.get_untracked();
                        let prefix_len = state.editor.completion_filter_text.get_untracked().len();
                        if let Some(entry) = items.get(idx) {
                            let text = if entry.insert_text.is_empty() {
                                entry.label.clone()
                            } else {
                                entry.insert_text.clone()
                            };
                            state
                                .editor
                                .pending_completion
                                .set(Some((text, prefix_len)));
                        }
                        state.editor.completion_open.set(false);
                    }
                })
                .on_event_stop(EventListener::PointerEnter, move |_| selected.set(idx))
            }
        })
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| {
        s.width_full()
            .max_height(ui_const::COMPLETION_POPUP_MAX_HEIGHT)
    });

    let header = stack((
        label(|| "Completions").style(move |s| {
            s.font_size(11.0)
                .color(state.workbench.theme.get().palette.text_muted)
                .flex_grow(1.0)
        }),
        container(label(|| "Esc"))
            .style(move |s| {
                let p = state.workbench.theme.get().palette;
                s.font_size(10.0)
                    .color(p.text_muted)
                    .background(p.bg_elevated)
                    .padding_horiz(5.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| state.editor.completion_open.set(false)),
    ))
    .style(move |s| {
        s.items_center()
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(state.workbench.theme.get().palette.border)
            .margin_bottom(4.0)
    });

    let empty_hint = label(move || {
        let f = filter.get().to_lowercase();
        let count = items
            .get()
            .into_iter()
            .filter(|e| f.is_empty() || e.label.to_lowercase().contains(&f))
            .count();
        if count == 0 {
            format!(
                "No completions{}",
                if f.is_empty() {
                    String::new()
                } else {
                    format!(" for \"{f}\"")
                }
            )
        } else {
            String::new()
        }
    })
    .style(move |s| {
        let f = filter.get().to_lowercase();
        let count = items
            .get()
            .into_iter()
            .filter(|e| f.is_empty() || e.label.to_lowercase().contains(&f))
            .count();
        s.font_size(12.0)
            .color(state.workbench.theme.get().palette.text_muted)
            .padding(12.0)
            .apply_if(count > 0, |s| s.display(floem::style::Display::None))
    });

    let popup_box = stack((header, empty_hint, list))
        .style(move |s| {
            let t = state.workbench.theme.get();
            let p = &t.palette;
            s.flex_col()
                .width(ui_const::COMPLETION_POPUP_WIDTH)
                .background(p.bg_panel)
                .border(1.0)
                .border_color(p.glass_border)
                .border_radius(8.0)
                .box_shadow_h_offset(0.0)
                .box_shadow_v_offset(4.0)
                .box_shadow_blur(32.0)
                .box_shadow_color(p.glow)
                .box_shadow_spread(0.0)
        })
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                match &ke.key.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        state.editor.completion_open.set(false);
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        let f = filter.get().to_lowercase();
                        let max = items
                            .get()
                            .into_iter()
                            .filter(|e| f.is_empty() || e.label.to_lowercase().contains(&f))
                            .count()
                            .saturating_sub(1);
                        selected.update(|v| *v = (*v + 1).min(max));
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        selected.update(|v| *v = v.saturating_sub(1));
                    }
                    _ => {}
                }
            }
        });

    container(popup_box)
        .style(move |s| {
            let shown = state.editor.completion_open.get();
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(120.0)
                .z_index(ui_const::Z_COMPLETIONS)
                .background(floem::peniko::Color::TRANSPARENT)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| state.editor.completion_open.set(false))
}

// ── Ctrl+K inline AI-edit overlay ────────────────────────────────────────────

#[derive(Clone, Debug)]
enum InlineEditUpdate {
    /// No selection: text to insert at the cursor.
    Done(String),
    /// A selection: the rewrite to put in its place, for review.
    Replace(crate::domain_state::editor::InlineReplace),
    Err(String),
}

/// Upper bound on the file text sent as context for a Ctrl+K request that
/// has no selection.
const INLINE_EDIT_CONTEXT_CHARS: usize = 4096;

/// With a selection, at most this much of the file on each side of it goes
/// along, so the rewrite matches the names and style around it.
const INLINE_EDIT_AROUND_CHARS: usize = 1500;

/// Most changed lines the review shows; the rest are counted.
const INLINE_REVIEW_LINES: usize = 40;

/// The file text on each side of `selection` in `file_ctx`, cut to whole
/// lines and [`INLINE_EDIT_AROUND_CHARS`] each. Empty when the selection is
/// not in the text exactly once (the file on disk may not match the buffer),
/// since context from the wrong place would mislead the model.
pub(crate) fn text_around<'a>(selection: &str, file_ctx: &'a str) -> (&'a str, &'a str) {
    if selection.trim().is_empty() {
        return ("", "");
    }
    let mut found = file_ctx.match_indices(selection);
    let Some((at, _)) = found.next() else {
        return ("", "");
    };
    if found.next().is_some() {
        return ("", "");
    }
    let before = &file_ctx[..at];
    let after = &file_ctx[at + selection.len()..];
    let mut b = before.len().saturating_sub(INLINE_EDIT_AROUND_CHARS);
    b = before.ceil_char_boundary(b);
    if b > 0 {
        b = before[b..].find('\n').map_or(before.len(), |n| b + n + 1);
    }
    let mut a = after.floor_char_boundary(INLINE_EDIT_AROUND_CHARS.min(after.len()));
    if a < after.len() {
        a = after[..a].rfind('\n').map_or(0, |n| n + 1);
    }
    (&before[b..], &after[..a])
}

/// The rewrite to put in place of `original`, from the model's reply: a
/// Markdown fence around it is dropped (models add one despite being told
/// not to) and the trailing line break is made to match the selection's, so
/// a rewrite of whole lines doesn't glue the next line on or add a blank one.
pub(crate) fn fit_replacement(original: &str, reply: &str) -> Result<String, String> {
    let mut text = reply;
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some((_lang, body)) = rest.split_once('\n') {
            let body = body.trim_end();
            text = body.strip_suffix("```").unwrap_or(body);
        }
    }
    if text.trim().is_empty() {
        return Err("the model returned nothing".into());
    }
    if !original.starts_with(['\n', '\r']) {
        text = text.trim_start_matches(['\n', '\r']);
    }
    let mut out = text.trim_end_matches(['\n', '\r']).to_string();
    if original.ends_with("\r\n") {
        out.push_str("\r\n");
    } else if original.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// The changed lines of a rewrite, `'-'` for removed and `'+'` for added,
/// at most `cap` of them, and how many more there are.
pub(crate) fn review_lines(original: &str, text: &str, cap: usize) -> (Vec<(char, String)>, usize) {
    let diff = similar::TextDiff::from_lines(original, text);
    let mut lines = Vec::new();
    let mut more = 0;
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            similar::ChangeTag::Delete => '-',
            similar::ChangeTag::Insert => '+',
            similar::ChangeTag::Equal => continue,
        };
        if lines.len() < cap {
            let line = change.value().trim_end_matches(['\n', '\r']);
            lines.push((sign, line.to_string()));
        } else {
            more += 1;
        }
    }
    (lines, more)
}

/// The file a Ctrl+K request works on.
///
/// The code that is sent (the selection, or the file text) comes from the
/// active editor tab — the tab whose cursor and selection the editor mirrors
/// into `active_cursor` / `selected_text`. `open_file` is the explorer's last
/// pick and lags behind once the user switches tabs by clicking them (or
/// closes one), so it is only a fallback for when no tab is active; then
/// there is no selection either.
pub(crate) fn inline_edit_source(
    active_tab: Option<&std::path::Path>,
    open_file: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    active_tab.or(open_file).map(std::path::Path::to_path_buf)
}

/// The Ctrl+K prompt: rewrite only the selection when there is one, otherwise
/// generate a fragment against the (capped) file text.
pub(crate) fn inline_edit_prompt(instruction: &str, selection: &str, file_ctx: &str) -> String {
    if !selection.is_empty() {
        // Selection-scoped: only rewrite the selected region. The code
        // around it goes along when it can be found, so the rewrite fits.
        let (before, after) = text_around(selection, file_ctx);
        if before.is_empty() && after.is_empty() {
            return format!(
                "Rewrite ONLY the following code snippet according to the instruction. \
                 Respond with ONLY the rewritten snippet, no explanation, no markdown fences.\n\n\
                 Instruction: {instruction}\n\nCode to rewrite:\n{selection}"
            );
        }
        return format!(
            "Rewrite ONLY the selected code according to the instruction. Respond with ONLY \
             the code that replaces the selection: no explanation, no markdown fences, and \
             nothing from the code before or after it, which is shown only so the rewrite \
             fits.\n\nInstruction: {instruction}\n\nCode before the selection:\n{before}\n\
             Selected code to rewrite:\n{selection}\n\nCode after the selection:\n{after}"
        );
    }
    let file_ctx = if file_ctx.len() > INLINE_EDIT_CONTEXT_CHARS {
        &file_ctx[..file_ctx.floor_char_boundary(INLINE_EDIT_CONTEXT_CHARS)]
    } else {
        file_ctx
    };
    format!(
        "Apply the following edit to the code. \
         Respond with ONLY the generated code fragment, no explanation, no markdown fences.\n\n\
         Instruction: {instruction}\n\nCode context:\n{file_ctx}"
    )
}

pub(crate) fn inline_edit_overlay(state: IdeState) -> impl IntoView {
    let open = state.ai.inline_edit_open;
    let query = state.ai.inline_edit_query;
    let ai_thinking = state.ai.thinking;
    // True from submitting an edit until its result (or error) arrives, so a
    // second Enter or a held key can't start a second request and insert twice.
    // (`ai.thinking` is shared with chat, so it can't serve as this guard.)
    let in_flight = create_rw_signal(false);
    // A request that never reports back must not block later edits: reopening
    // the box starts fresh.
    create_effect(move |_| {
        if open.get() {
            in_flight.set(false);
        }
    });

    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<InlineEditUpdate>(64);
    let update_sig = create_signal_from_channel(update_rx);

    {
        let state2 = state.clone();
        create_effect(move |_| {
            let Some(upd) = update_sig.get() else { return };
            in_flight.set(false);
            match upd {
                InlineEditUpdate::Done(text) => {
                    state2.editor.pending_completion.set(Some((text, 0)));
                    state2.ai.thinking.set(false);
                    state2.ai.inline_edit_open.set(false);
                    state2.ai.inline_edit_query.set(String::new());
                }
                InlineEditUpdate::Replace(req) => {
                    let request = state2.editor.inline_edit.request;
                    request.set(Some(req));
                    // The main pane takes the request at once when the file
                    // is its active tab. Still here means it isn't: drop it
                    // rather than replace text later, somewhere unseen.
                    if request.get_untracked().is_some() {
                        request.set(None);
                        show_toast(
                            state2.workbench.status_toast,
                            "AI edit: that file is no longer the open tab, so nothing was replaced"
                                .to_string(),
                        );
                    }
                    state2.ai.thinking.set(false);
                    state2.ai.inline_edit_open.set(false);
                    state2.ai.inline_edit_query.set(String::new());
                }
                InlineEditUpdate::Err(e) => {
                    tracing::warn!(target: "phazeai_ui", "Ctrl+K error: {e}");
                    // Say so on screen too: the box closing alone looks like success.
                    let first_line = e.lines().next().unwrap_or("").trim();
                    let shown = phazeai_core::text::truncate_bytes(first_line, 160);
                    show_toast(
                        state2.workbench.status_toast,
                        format!("AI edit failed: {shown}"),
                    );
                    state2.ai.thinking.set(false);
                    state2.ai.inline_edit_open.set(false);
                    state2.ai.inline_edit_query.set(String::new());
                }
            }
        });
    }

    let hint = label(|| "Describe the change (Enter to apply, Esc to cancel)").style(move |s| {
        s.font_size(11.0)
            .color(state.workbench.theme.get().palette.text_muted)
            .margin_bottom(6.0)
    });

    let input = text_input(query)
        .focus_when_open(move || open.get())
        .placeholder("e.g. \"add error handling\", \"convert to async\", \"add JSDoc\"")
        .style(move |s| {
            let t = state.workbench.theme.get();
            let p = &t.palette;
            s.width_full()
                .padding_horiz(10.0)
                .padding_vert(8.0)
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_elevated)
                .border(1.0)
                .border_color(p.border_focus)
                .border_radius(6.0)
        })
        .on_enter(move || {
            let instruction = query.get();
            if instruction.is_empty() || in_flight.get_untracked() {
                return;
            }
            // Privacy: a denied file's code is never sent. The check,
            // the selection and the file text all refer to the same
            // file — the active tab (see `inline_edit_source`).
            let active_tab = state
                .editor
                .active_cursor
                .get_untracked()
                .map(|(path, _, _)| path);
            let open_file = state.editor.open_file.get_untracked();
            let ctx_file = inline_edit_source(active_tab.as_deref(), open_file.as_deref());
            if let Some(ref p) = ctx_file {
                if !crate::panels::privacy::allow_editor_content(
                    state.workbench.status_toast,
                    p,
                    "inline edit",
                ) {
                    open.set(false);
                    query.set(String::new());
                    return;
                }
            }
            let selection = state.editor.selected_text.get_untracked();
            // A selection is rewritten in place, so it must be one region of
            // the file that was checked above.
            let target = if selection.is_empty() {
                None
            } else {
                match state.editor.inline_edit.selection.get_untracked() {
                    Some((path, start, end)) if Some(&path) == ctx_file.as_ref() => {
                        Some((path, start, end))
                    }
                    _ => {
                        show_toast(
                            state.workbench.status_toast,
                            "AI edit rewrites one selection at a time; select a single range"
                                .to_string(),
                        );
                        return;
                    }
                }
            };
            state.editor.inline_edit.review.set(None);
            let attached: Vec<std::path::PathBuf> = ctx_file.iter().cloned().collect();
            in_flight.set(true);
            ai_thinking.set(true);
            let file_ctx = ctx_file
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .unwrap_or_default();
            let prompt = inline_edit_prompt(&instruction, &selection, &file_ctx);
            let settings = Settings::load();
            let tx = update_tx.clone();
            std::thread::spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(InlineEditUpdate::Err(format!("Runtime: {e}")));
                        return;
                    }
                };
                rt.block_on(async move {
                    let client = match settings.build_llm_client() {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.send(InlineEditUpdate::Err(format!("LLM: {e}")));
                            return;
                        }
                    };
                    let reply =
                        crate::ai_helpers::inline_edit_reply(client, &prompt, attached).await;
                    let update = match (reply, target) {
                        (Ok(text), None) => InlineEditUpdate::Done(text),
                        (Ok(text), Some((path, start, end))) => {
                            match fit_replacement(&selection, &text) {
                                Ok(text) => InlineEditUpdate::Replace(
                                    crate::domain_state::editor::InlineReplace {
                                        path,
                                        start,
                                        end,
                                        original: selection,
                                        text,
                                        undo: false,
                                    },
                                ),
                                Err(e) => InlineEditUpdate::Err(e),
                            }
                        }
                        (Err(e), _) => InlineEditUpdate::Err(e),
                    };
                    let _ = tx.send(update);
                });
            });
        });

    let badge = label(|| "✦ AI Edit").style(move |s| {
        let p = state.workbench.theme.get().palette;
        s.font_size(11.0)
            .color(p.accent)
            .font_weight(floem::text::Weight::BOLD)
            .margin_bottom(8.0)
    });

    let box_view = stack((badge, hint, input)).style(move |s| {
        let t = state.workbench.theme.get();
        let p = &t.palette;
        s.flex_col()
            .padding(20.0)
            .width(520.0)
            .background(p.bg_panel)
            .border(1.0)
            .border_color(p.glass_border)
            .border_radius(10.0)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(6.0)
            .box_shadow_blur(40.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    });

    container(box_view)
        .style(move |s| {
            let shown = open.get();
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(200.0)
                .z_index(ui_const::Z_INLINE_EDIT)
                .background(state.workbench.theme.get().palette.overlay_bg)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| {
            open.set(false);
            query.set(String::new());
        })
}

// ── Ctrl+K review: Keep or Undo the rewrite ──────────────────────────────────

/// The bar under a Ctrl+K rewrite: what changed, with Keep and Undo. Undo
/// puts the original back while the rewrite is still as it was made; Escape
/// or Keep closes the bar and leaves the rewrite (Ctrl+Z still undoes it).
pub(crate) fn inline_review_bar(state: IdeState) -> impl IntoView {
    let review = state.editor.inline_edit.review;
    let request = state.editor.inline_edit.request;
    let theme = state.workbench.theme;

    let title = label(move || {
        review.with(|r| match r {
            Some(r) => {
                let (lines, more) = review_lines(&r.original, &r.text, usize::MAX);
                let added = lines.iter().filter(|(s, _)| *s == '+').count();
                let removed = lines.len() + more - added;
                format!("✦ AI edit applied   +{added} −{removed}")
            }
            None => String::new(),
        })
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(12.0)
            .color(p.accent)
            .font_weight(floem::text::Weight::BOLD)
            .flex_grow(1.0)
    });

    let button = move |text: &'static str, primary: bool| {
        label(move || text).style(move |s| {
            let p = theme.get().palette;
            s.font_size(12.0)
                .padding_horiz(12.0)
                .padding_vert(4.0)
                .margin_left(8.0)
                .border_radius(4.0)
                .border(1.0)
                .border_color(p.border)
                .cursor(floem::style::CursorStyle::Pointer)
                .apply_if(primary, |s| {
                    s.background(p.accent)
                        .color(p.bg_panel)
                        .border_color(p.accent)
                })
                .apply_if(!primary, |s| s.color(p.text_primary))
        })
    };
    let keep = button("Keep", true).on_click_stop(move |_| review.set(None));
    let undo = button("Undo", false).on_click_stop(move |_| {
        let Some(r) = review.get_untracked() else {
            return;
        };
        review.set(None);
        request.set(Some(crate::domain_state::editor::InlineReplace {
            path: r.path,
            start: r.start,
            end: r.end,
            original: r.text,
            text: r.original,
            undo: true,
        }));
    });
    let header = stack((title, keep, undo)).style(|s| s.items_center().margin_bottom(6.0));

    let diff = scroll(
        dyn_stack(
            move || {
                review.with(|r| match r {
                    Some(r) => {
                        let (mut lines, more) =
                            review_lines(&r.original, &r.text, INLINE_REVIEW_LINES);
                        if more > 0 {
                            lines.push((' ', format!("… {more} more changed lines")));
                        }
                        lines.into_iter().enumerate().collect::<Vec<_>>()
                    }
                    None => Vec::new(),
                })
            },
            |(i, line): &(usize, (char, String))| (*i, line.clone()),
            move |(_, (sign, text))| {
                label(move || format!("{sign} {text}")).style(move |s| {
                    let p = theme.get().palette;
                    let s = s.font_size(12.0).font_family("monospace".to_string());
                    match sign {
                        '+' => s.color(p.success),
                        '-' => s.color(p.error),
                        _ => s.color(p.text_muted),
                    }
                })
            },
        )
        .style(|s| s.flex_col()),
    )
    .style(|s| s.max_height(220.0));

    stack((header, diff)).style(move |s| {
        let shown = review.with(|r| r.is_some());
        let p = theme.get().palette;
        s.flex_col()
            .absolute()
            .inset_bottom(48.0)
            .inset_left_pct(20.0)
            .width_pct(60.0)
            .padding(12.0)
            .background(p.bg_panel)
            .border(1.0)
            .border_color(p.glass_border)
            .border_radius(8.0)
            .box_shadow_v_offset(4.0)
            .box_shadow_blur(24.0)
            .box_shadow_color(p.glow)
            .z_index(ui_const::Z_INLINE_EDIT)
            .apply_if(!shown, |s| s.display(floem::style::Display::None))
    })
}

// ── LSP Hover tooltip overlay ─────────────────────────────────────────────────

pub(crate) fn hover_tooltip(state: IdeState) -> impl IntoView {
    let hover_text = state.editor.hover_text;
    let hover_anchor_x = state.editor.hover_anchor_x;
    let hover_anchor_y = state.editor.hover_anchor_y;
    let theme = state.workbench.theme;

    let tooltip_box = container(label(move || hover_text.get().unwrap_or_default()).style(
        move |s| {
            let p = theme.get().palette;
            s.font_size(12.0).color(p.text_primary).max_width(480.0)
        },
    ))
    .style(move |s| {
        let shown = hover_text.get().is_some();
        let p = theme.get().palette;
        s.padding_horiz(12.0)
            .padding_vert(8.0)
            .background(p.bg_elevated)
            .border(1.0)
            .border_color(p.border)
            .border_radius(6.0)
            .max_width(500.0)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(4.0)
            .box_shadow_blur(16.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
            .apply_if(!shown, |s| s.display(floem::style::Display::None))
    })
    .on_click_stop(move |_| {
        hover_text.set(None);
    });

    container(tooltip_box).style(move |s| {
        let shown = hover_text.get().is_some();
        let ax = hover_anchor_x.get();
        let ay = hover_anchor_y.get();
        // Show the tooltip below the pointer, clamped so it starts no further
        // right than 200 px from the left edge and doesn't go off-screen top.
        let tip_x = (ax - 8.0).max(8.0).min(ax);
        let tip_y = (ay + 24.0).max(8.0);
        s.absolute()
            .inset_top(tip_y)
            .inset_left(tip_x)
            .z_index(ui_const::Z_HOVER_TIP)
            .apply_if(!shown, |s| s.display(floem::style::Display::None))
    })
}

// ── Code Actions dropdown overlay (Ctrl+.) ───────────────────────────────────

pub(crate) fn code_actions_overlay(state: IdeState) -> impl IntoView {
    let open = state.editor.code_actions_open;
    let actions = state.editor.code_actions;
    let theme = state.workbench.theme;
    let hovered: RwSignal<Option<usize>> = create_rw_signal(None);

    let header = stack((
        label(|| "Quick Fix").style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .color(p.accent)
                .font_weight(floem::text::Weight::SEMIBOLD)
                .flex_grow(1.0)
        }),
        container(label(|| "Esc"))
            .style(move |s| {
                let p = theme.get().palette;
                s.font_size(10.0)
                    .color(p.text_muted)
                    .background(p.bg_elevated)
                    .padding_horiz(5.0)
                    .padding_vert(2.0)
                    .border_radius(3.0)
                    .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| open.set(false)),
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.items_center()
            .width_full()
            .padding_horiz(12.0)
            .padding_vert(8.0)
            .border_bottom(1.0)
            .border_color(p.border)
            .margin_bottom(4.0)
    });

    let empty_hint = label(move || {
        if actions.get().is_empty() {
            "No actions available at this position.".to_string()
        } else {
            String::new()
        }
    })
    .style(move |s| {
        let p = theme.get().palette;
        s.font_size(12.0)
            .color(p.text_muted)
            .padding(12.0)
            .apply_if(!actions.get().is_empty(), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let list = scroll(
        dyn_stack(
            move || {
                safe_get(actions, Vec::new())
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(idx, _)| *idx,
            {
                let state2 = state.clone();
                move |(idx, action): (usize, CodeAction)| {
                    let title = action.title.clone();
                    let kind = action.kind.clone();
                    let edits = action.edit.clone();
                    let hov = hovered;
                    let state3 = state2.clone();

                    container(
                        stack((
                            label(|| "▶ ").style(move |s| {
                                let p = state3.workbench.theme.get().palette;
                                s.font_size(10.0).color(p.accent).margin_right(4.0)
                            }),
                            label(move || title.clone()).style(move |s: floem::style::Style| {
                                let p = state3.workbench.theme.get().palette;
                                s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
                            }),
                        ))
                        .style(|s| s.flex_row().items_center().width_full()),
                    )
                    .style({
                        let state4 = state2.clone();
                        move |s| {
                            let p = state4.workbench.theme.get().palette;
                            s.width_full()
                                .padding_horiz(12.0)
                                .padding_vert(8.0)
                                .border_radius(4.0)
                                .cursor(floem::style::CursorStyle::Pointer)
                                .background(if hov.get() == Some(idx) {
                                    p.bg_elevated
                                } else {
                                    floem::peniko::Color::TRANSPARENT
                                })
                        }
                    })
                    .on_click_stop({
                        let state5 = state2.clone();
                        let kind2 = kind.clone();
                        let edits2 = edits.clone();
                        move |_| {
                            state5.editor.code_actions_open.set(false);
                            if kind2 == "source.formatDocument" {
                                if let Some(path) = state5.editor.open_file.get() {
                                    if let Ok(text) = std::fs::read_to_string(&path) {
                                        let _ = state5.project.lsp_cmd.send(LspCommand::OpenFile {
                                            path: path.clone(),
                                            text,
                                        });
                                    }
                                }
                            } else if kind2 == "refactor.findReferences" {
                                state5.workbench.show_bottom_panel.set(true);
                                state5.workbench.bottom_panel_tab.set(Tab::References);
                            } else if let Some(file_edits) = edits2.as_ref() {
                                for (fpath, new_content) in file_edits {
                                    let _ = std::fs::write(fpath, new_content);
                                    if state5.editor.open_file.get().as_ref() == Some(fpath) {
                                        state5.editor.open_file.set(Some(fpath.clone()));
                                    }
                                }
                            }
                        }
                    })
                    .on_event_stop(EventListener::PointerEnter, move |_| {
                        hov.set(Some(idx));
                    })
                    .on_event_stop(EventListener::PointerLeave, move |_| {
                        hov.set(None);
                    })
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(320.0));

    let dropdown_box = stack((header, empty_hint, list))
        .style(move |s| {
            let t = state.workbench.theme.get();
            let p = &t.palette;
            s.flex_col()
                .width(380.0)
                .background(p.bg_panel)
                .border(1.0)
                .border_color(p.glass_border)
                .border_radius(8.0)
                .box_shadow_h_offset(0.0)
                .box_shadow_v_offset(4.0)
                .box_shadow_blur(32.0)
                .box_shadow_color(p.glow)
                .box_shadow_spread(0.0)
        })
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                if ke.key.logical_key == Key::Named(NamedKey::Escape) {
                    open.set(false);
                }
            }
        });

    container(dropdown_box)
        .style(move |s| {
            let shown = state.editor.code_actions_open.get();
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(150.0)
                .z_index(ui_const::Z_CODE_ACTIONS)
                .background(state.workbench.theme.get().palette.overlay_bg_light)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| state.editor.code_actions_open.set(false))
}

// ── Rename overlay (F2) ──────────────────────────────────────────────────────

/// Write every tab with unsaved edits to disk; returns the names of those that
/// could not be written.
fn save_unsaved_tabs(panes: &[EditorPaneHandle]) -> Vec<String> {
    let mut failed = Vec::new();
    for pane in panes {
        for (index, tab) in (pane.tabs)().iter().enumerate() {
            if !tab.dirty {
                continue;
            }
            match (pane.save_tab)(index) {
                super::ex_command::SaveOutcome::Saved => {}
                _ => failed.push(tab.name.clone()),
            }
        }
    }
    failed
}

pub(crate) fn rename_overlay(state: IdeState) -> impl IntoView {
    let open = state.editor.rename_open;
    let query = state.editor.rename_query;
    let target = state.editor.rename_target;
    let lsp_cmd = state.project.lsp_cmd.clone();
    let cursor = state.editor.active_cursor;
    let ws = state.project.workspace_root.get_untracked();

    // Rename the symbol: Enter in the box or the Rename button.
    let submit: std::rc::Rc<dyn Fn()> = {
        let lsp_cmd2 = lsp_cmd.clone();
        let panes = state.editor.panes;
        let toast = state.workbench.status_toast;
        std::rc::Rc::new(move || {
            let new_name = query.get_untracked().trim().to_string();
            open.set(false);
            if new_name.is_empty() {
                return;
            }
            let Some((path, line, col)) = cursor.get_untracked() else {
                return;
            };
            // The rename rewrites files on disk, from positions the language
            // server read in the open tabs; save those first so both agree.
            let failed = save_unsaved_tabs(&panes.get_untracked());
            if !failed.is_empty() {
                show_toast(
                    toast,
                    format!("Rename cancelled: could not save {}", failed.join(", ")),
                );
                return;
            }
            let _ = lsp_cmd2.send(LspCommand::RequestRename {
                path,
                line,
                col,
                new_name,
                workspace_root: ws.clone(),
            });
        })
    };

    let input = text_input(query)
        .focus_when_open(move || open.get())
        .on_enter({
            let submit = submit.clone();
            move || submit()
        })
        .style(|s| s.width(320.0).padding(8.0).font_size(14.0));

    let confirm = {
        label(|| "Rename".to_string())
            .style(move |s| {
                let pal = &state.workbench.theme.get().palette;
                s.padding_horiz(16.0)
                    .padding_vert(6.0)
                    .background(pal.button_primary_bg)
                    .color(pal.button_primary_fg)
                    .border_radius(4.0)
                    .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| submit())
    };

    let cancel = label(|| "Cancel".to_string())
        .style(move |s| {
            let pal = &state.workbench.theme.get().palette;
            s.padding_horiz(16.0)
                .padding_vert(6.0)
                .background(pal.button_hover_bg)
                .border_radius(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| {
            open.set(false);
        });

    let title = label(move || format!("Rename '{}'", target.get())).style(move |s| {
        s.font_size(13.0)
            .color(state.workbench.theme.get().palette.text_secondary)
            .margin_bottom(8.0)
    });

    let dialog = container(
        stack((
            title,
            input,
            stack((confirm, cancel))
                .style(|s| s.flex_row().gap(8.0).margin_top(10.0).justify_end()),
        ))
        .style(|s| s.flex_col().gap(4.0)),
    )
    .style(move |s| {
        let t = state.workbench.theme.get();
        let p = &t.palette;
        s.padding(20.0)
            .border_radius(10.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .min_width(360.0)
    });

    container(dialog)
        .style(move |s| {
            let shown = open.get();
            s.absolute()
                .inset(0)
                .items_center()
                .justify_center()
                .z_index(ui_const::Z_RENAME)
                .background(state.workbench.theme.get().palette.overlay_bg)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

// ── Signature help overlay (Ctrl+Shift+Space) ────────────────────────────────

pub(crate) fn sig_help_overlay(state: IdeState) -> impl IntoView {
    let sig_help = state.editor.sig_help;

    let content = container(
        label(move || {
            if let Some(sh) = sig_help.get() {
                let mut text = sh.label.clone();
                if !sh.params.is_empty() {
                    let params_joined = sh.params.join(", ");
                    if let Some(param) = sh.params.get(sh.active_param) {
                        text = format!("{text}\nActive: {param}  |  {params_joined}");
                    } else {
                        text = format!("{text}\n{params_joined}");
                    }
                }
                text
            } else {
                String::new()
            }
        })
        .style(move |s| {
            s.font_size(12.0)
                .color(state.workbench.theme.get().palette.text_secondary)
        }),
    )
    .style(move |s| {
        let t = state.workbench.theme.get();
        let p = &t.palette;
        s.padding(10.0)
            .border_radius(6.0)
            .background(p.bg_panel)
            .border(1.0)
            .border_color(p.glass_border)
            .max_width(600.0)
    });

    container(content)
        .style(move |s| {
            let shown = sig_help.get().is_some();
            s.absolute()
                .inset(0)
                .items_end()
                .justify_center()
                .padding_bottom(80.0)
                .z_index(ui_const::Z_SIG_HELP)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| sig_help.set(None))
}

// ── Toast notification overlay ────────────────────────────────────────────────

pub(crate) fn toast_overlay(state: IdeState) -> impl IntoView {
    let toast = state.workbench.status_toast;
    let theme = state.workbench.theme;

    container(
        label(move || toast.get().unwrap_or_default()).style(move |s| {
            let p = theme.get().palette;
            s.font_size(13.0).color(p.text_primary)
        }),
    )
    .style(move |s| {
        let shown = toast.get().is_some();
        let p = theme.get().palette;
        s.absolute()
            .inset_bottom(40.0)
            .inset_left(320.0)
            .z_index(ui_const::Z_TOAST)
            .padding_horiz(18.0)
            .padding_vert(10.0)
            .background(p.bg_elevated)
            .border_radius(8.0)
            .border(1.0)
            .border_color(p.border)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(4.0)
            .box_shadow_blur(20.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
            .apply_if(!shown, |s| s.display(floem::style::Display::None))
    })
}

// ── Workspace Symbols overlay (Ctrl+T) ───────────────────────────────────────

pub(crate) fn workspace_symbols_overlay(state: IdeState) -> impl IntoView {
    let open = state.editor.ws_syms_open;
    let query = state.editor.ws_syms_query;
    let symbols = state.editor.workspace_symbols;
    let theme = state.workbench.theme;
    let lsp_cmd = state.project.lsp_cmd.clone();
    let goto_line = state.editor.goto_line;

    let filtered = move || {
        let q = query.get().to_lowercase();
        let syms = symbols.get();
        if q.is_empty() {
            syms.into_iter().take(50).collect::<Vec<_>>()
        } else {
            syms.into_iter()
                .filter(|s| s.name.to_lowercase().contains(&q))
                .take(50)
                .collect::<Vec<_>>()
        }
    };

    let rows = scroll(
        dyn_stack(
            filtered,
            |s| format!("{}:{}", s.name, s.line),
            move |sym| {
                let name = sym.name.clone();
                let kind = sym.kind.clone();
                let line = sym.line;
                let row_theme = theme;
                container(
                    stack((
                        label(move || kind.clone()).style(move |s| {
                            let p = row_theme.get().palette;
                            s.font_size(10.0).color(p.accent).width(44.0)
                        }),
                        label(move || name.clone()).style(move |s| {
                            let p = row_theme.get().palette;
                            s.font_size(13.0).color(p.text_primary)
                        }),
                        label(move || format!("  :{line}")).style(move |s| {
                            let p = row_theme.get().palette;
                            s.font_size(11.0).color(p.text_muted)
                        }),
                    ))
                    .style(|s| s.items_center().padding_vert(2.0)),
                )
                .style(move |s| {
                    let p = row_theme.get().palette;
                    s.padding_horiz(12.0)
                        .padding_vert(4.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                        .hover(|s| s.background(p.bg_elevated))
                })
                .on_click_stop(move |_| {
                    open.set(false);
                    goto_line.set(line);
                })
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.max_height(320.0).width_full());

    let search_box = text_input(query)
        .focus_when_open(move || open.get())
        .style(move |s| {
            let p = theme.get().palette;
            s.width_full()
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_base)
                .border(0.0)
                .padding_horiz(12.0)
                .padding_vert(8.0)
        })
        .on_enter(move || {
            if let Some(sym) = filtered().into_iter().next() {
                open.set(false);
                goto_line.set(sym.line);
            }
        });

    {
        let lsp_tx = lsp_cmd.clone();
        create_effect(move |_| {
            let q = query.get();
            if open.get_untracked() {
                let _ = lsp_tx.send(LspCommand::RequestWorkspaceSymbols { query: q });
            }
        });
    }

    let dialog = stack((
        stack((label(|| "Workspace Symbols").style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .color(p.text_muted)
                .padding_horiz(12.0)
                .padding_vert(6.0)
        }),))
        .style(|s| s.width_full()),
        search_box,
        container(empty()).style(move |s| {
            s.height(1.0)
                .width_full()
                .background(theme.get().palette.border)
        }),
        rows,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width(520.0)
            .max_height(400.0)
            .border_radius(10.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(8.0)
            .box_shadow_blur(32.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    });

    container(dialog)
        .style(move |s| {
            let shown = open.get();
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(80.0)
                .z_index(ui_const::Z_WS_SYMBOLS)
                .background(state.workbench.theme.get().palette.overlay_bg)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

// ── Branch picker overlay (click branch in status bar) ───────────────────────

pub(crate) fn branch_picker_overlay(state: IdeState) -> impl IntoView {
    let open = state.project.branch_picker_open;
    let branches = state.project.branch_list;
    let current = state.project.git_branch;
    let theme = state.workbench.theme;
    let workspace = state.project.workspace_root;
    let toast = state.workbench.status_toast;

    let (checkout_tx, checkout_rx) = std::sync::mpsc::sync_channel::<Result<String, String>>(1);
    let checkout_sig = create_signal_from_channel(checkout_rx);
    {
        let tst = toast;
        let cur = current;
        create_effect(move |_| {
            if let Some(result) = checkout_sig.get() {
                match result {
                    Ok(new_branch) => {
                        cur.set(new_branch.clone());
                        show_toast(tst, format!("Switched to {new_branch}"));
                    }
                    Err(e) => {
                        show_toast(tst, format!("Checkout failed: {e}"));
                    }
                }
            }
        });
    }

    let rows = scroll(
        dyn_stack(
            move || safe_get(branches, Vec::new()),
            |b| b.clone(),
            move |branch| {
                let b_label = branch.clone();
                let b_current = branch.clone();
                let b_click = branch.clone();
                let is_current = move || current.get() == b_current;
                let hov = create_rw_signal(false);
                let ws = workspace;
                let row_tx = checkout_tx.clone();

                container(
                    stack((
                        label(move || if is_current() { "✓ " } else { "  " }.to_string()).style(
                            move |s| {
                                s.font_size(12.0)
                                    .color(theme.get().palette.success)
                                    .width(20.0)
                            },
                        ),
                        label(move || b_label.clone()).style(move |s| {
                            s.font_size(13.0).color(theme.get().palette.text_primary)
                        }),
                    ))
                    .style(|s| s.items_center()),
                )
                .style(move |s| {
                    let p = theme.get().palette;
                    s.padding_horiz(12.0)
                        .padding_vert(6.0)
                        .cursor(floem::style::CursorStyle::Pointer)
                        .background(if hov.get() {
                            p.bg_elevated
                        } else {
                            floem::peniko::Color::TRANSPARENT
                        })
                })
                .on_click_stop(move |_| {
                    let branch_name = b_click.clone();
                    open.set(false);
                    let root = ws.get();
                    let tx = row_tx.clone();
                    std::thread::spawn(move || {
                        let out = phazeai_core::process::std_command("git")
                            .args(["checkout", &branch_name])
                            .current_dir(&root)
                            .output();
                        let result = match out {
                            Ok(o) if o.status.success() => Ok(branch_name),
                            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                            Err(e) => Err(e.to_string()),
                        };
                        let _ = tx.send(result);
                    });
                })
                .on_event_stop(EventListener::PointerEnter, move |_| hov.set(true))
                .on_event_stop(EventListener::PointerLeave, move |_| hov.set(false))
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.max_height(320.0).width_full());

    let dialog = stack((
        label(|| "Switch Branch").style(move |s| {
            let p = theme.get().palette;
            s.font_size(11.0)
                .color(p.text_muted)
                .padding_horiz(12.0)
                .padding_vert(8.0)
                .font_weight(floem::text::Weight::BOLD)
        }),
        container(empty()).style(move |s| {
            s.height(1.0)
                .width_full()
                .background(theme.get().palette.border)
        }),
        rows,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width(320.0)
            .max_height(400.0)
            .border_radius(10.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(8.0)
            .box_shadow_blur(32.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    });

    container(dialog)
        .style(move |s| {
            let shown = open.get();
            s.absolute()
                .inset(0)
                .items_end()
                .justify_start()
                .padding_bottom(30.0)
                .padding_left(8.0)
                .z_index(ui_const::Z_BRANCH_PICKER)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

// ── Vim ex command bar (:w, :q, :wq, :wqa, :e <file>, etc.) ─────────────────

/// Carry out one command typed into the ex bar. `:w :wq :x :q :q! :qa :wqa ...` save through
/// the editor's own save path and quit through the window-close path (`request_quit`); `:e` and
/// `:cd` are handled here; anything else is reported as unknown. Messages go to `notify`.
fn submit_ex_command(
    cmd: &str,
    panes: &[EditorPaneHandle],
    workspace: &std::path::Path,
    open_file: RwSignal<Option<std::path::PathBuf>>,
    quit_nonce: RwSignal<u64>,
    notify: impl Fn(String),
) {
    let cmd = cmd.trim();
    if let Some(run) = run_ex_command(cmd, panes) {
        if let Some(msg) = run.toast {
            notify(msg);
        }
        if run.quit {
            request_quit(quit_nonce);
        }
    } else if let Some(rest) = cmd.strip_prefix("e ") {
        let path = rest.trim();
        let full = workspace.join(path);
        if full.exists() {
            open_file.set(Some(full));
        } else {
            notify(format!("No such file: {path}"));
        }
    } else if let Some(rest) = cmd.strip_prefix("cd ") {
        let dir = rest.trim();
        let _ = std::env::set_current_dir(dir);
    } else if !cmd.is_empty() {
        notify(format!("Unknown command: {cmd}"));
    }
}

pub(crate) fn vim_ex_overlay(state: IdeState) -> impl IntoView {
    let open = state.editor.vim_ex_open;
    let input_sig = state.editor.vim_ex_input;
    let theme = state.workbench.theme;
    let open_file = state.editor.open_file;
    let toast = state.workbench.status_toast;
    let workspace = state.project.workspace_root;
    let panes = state.editor.panes;
    let quit_nonce = state.workbench.quit_nonce;

    let input_view = text_input(input_sig)
        .style(move |s| {
            let p = theme.get().palette;
            s.width_full()
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_panel)
                .border(0.0)
                .padding_horiz(4.0)
        })
        // The text input consumes Enter itself, so it never reaches a KeyDown listener.
        .on_enter(move || {
            let cmd = input_sig.get_untracked();
            open.set(false);
            input_sig.set(String::new());
            submit_ex_command(
                &cmd,
                &panes.get_untracked(),
                &workspace.get_untracked(),
                open_file,
                quit_nonce,
                |msg| show_toast(toast, msg),
            );
        })
        // Escape makes the text input drop focus (it consumes the key too); that cancels the bar.
        .on_event_stop(EventListener::FocusLost, move |_| {
            if open.get_untracked() {
                open.set(false);
                input_sig.set(String::new());
            }
        })
        // Without focus the typed command went to the editor's Vim keys instead of the bar.
        .focus_when_open(move || open.get());

    let bar = stack((
        label(|| ":").style(move |s| {
            s.font_size(14.0)
                .color(theme.get().palette.accent)
                .margin_right(4.0)
        }),
        input_view,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .width_full()
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .background(p.bg_panel)
            .border_top(1.5)
            .border_color(p.glass_border)
    });

    container(bar).style(move |s| {
        s.absolute()
            .inset_bottom(32.0)
            .inset_left(0)
            .inset_right(0)
            .z_index(ui_const::Z_VIM_EX)
            .apply_if(!open.get(), |s| s.display(floem::style::Display::None))
    })
}

// ── Goto line/col overlay (Ctrl+G) ────────────────────────────────────────────

pub(crate) fn goto_overlay(state: IdeState) -> impl IntoView {
    let open = state.editor.goto_overlay_open;
    let input_sig = state.editor.goto_overlay_input;
    let goto_line = state.editor.goto_line;
    let theme = state.workbench.theme;
    let toast = state.workbench.status_toast;

    let input_view = text_input(input_sig)
        .focus_when_open(move || open.get())
        .placeholder("Line or line:col")
        .style(move |s| {
            let p = theme.get().palette;
            s.width(220.0)
                .font_size(14.0)
                .color(p.text_primary)
                .background(p.bg_panel)
                .border(0.0)
                .padding_horiz(4.0)
        })
        .on_enter(move || {
            let text = input_sig.get_untracked();
            open.set(false);
            input_sig.set(String::new());
            let parts: Vec<&str> = text.trim().splitn(2, ':').collect();
            if let Ok(line) = parts[0].parse::<u32>() {
                goto_line.set(line.saturating_sub(1));
            } else {
                show_toast(toast, format!("Invalid: {text}"));
            }
        });

    let dialog = stack((
        label(|| "Go to Line  ").style(move |s| {
            s.font_size(11.0)
                .color(theme.get().palette.text_muted)
                .font_weight(floem::text::Weight::BOLD)
        }),
        input_view,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .padding_horiz(16.0)
            .padding_vert(10.0)
            .border_radius(8.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .box_shadow_h_offset(0.0)
            .box_shadow_v_offset(8.0)
            .box_shadow_blur(24.0)
            .box_shadow_color(p.glow)
            .box_shadow_spread(0.0)
    });

    container(dialog)
        .style(move |s| {
            s.absolute()
                .inset(0)
                .items_start()
                .justify_center()
                .padding_top(80.0)
                .z_index(ui_const::Z_GOTO)
                .apply_if(!open.get(), |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

// ── Peek Definition overlay (Alt+F12) ────────────────────────────────────────

pub(crate) fn peek_def_overlay(state: IdeState) -> impl IntoView {
    let open = state.editor.peek_def_open;
    let lines = state.editor.peek_def_lines;
    let theme = state.workbench.theme;

    let close_btn = container(label(|| "  Close  "))
        .style(move |s| {
            let p = theme.get().palette;
            s.padding_horiz(10.0)
                .padding_vert(4.0)
                .border_radius(4.0)
                .background(p.bg_elevated)
                .color(p.text_secondary)
                .font_size(11.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .border(1.0)
                .border_color(p.glass_border)
        })
        .on_click_stop(move |_| {
            open.set(false);
            lines.set(vec![]);
        });

    let header = stack((
        label(|| "Peek Definition").style(move |s| {
            s.font_size(12.0)
                .color(theme.get().palette.text_primary)
                .flex_grow(1.0)
        }),
        close_btn,
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(p.glass_border)
    });

    let content = scroll(
        dyn_stack(
            move || {
                safe_get(lines, Vec::new())
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(i, _)| *i,
            move |(_, line_text)| {
                let is_highlight = line_text.starts_with('>');
                let text_val = line_text.clone();
                container(label(move || text_val.clone())).style(move |s| {
                    let p = theme.get().palette;
                    s.font_family("monospace".to_string())
                        .font_size(12.0)
                        .color(if is_highlight {
                            p.text_primary
                        } else {
                            p.text_secondary
                        })
                        .background(if is_highlight {
                            p.accent_dim
                        } else {
                            floem::peniko::Color::TRANSPARENT
                        })
                        .padding_horiz(12.0)
                        .padding_vert(1.0)
                        .width_full()
                })
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.max_height(280.0).width_full());

    let popup = stack((header, content)).style(move |s| {
        let p = theme.get().palette;
        s.flex_col()
            .width(560.0)
            .background(p.bg_panel)
            .border(1.5)
            .border_color(p.glass_border)
            .border_radius(8.0)
    });

    container(popup)
        .style(move |s| {
            let shown = open.get() && !lines.get().is_empty();
            s.absolute()
                .inset(0)
                .items_center()
                .justify_center()
                .z_index(ui_const::Z_PEEK_DEF)
                .background(state.workbench.theme.get().palette.overlay_bg_light)
                .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| open.set(false))
}

#[cfg(test)]
mod picker_row_tests {
    use super::picker_row_key;
    use std::path::PathBuf;

    #[test]
    fn a_row_gets_a_new_key_when_the_file_at_its_position_changes() {
        // Unfiltered, position 0 is AGENTS.md; after typing "cargo.toml" it is
        // Cargo.toml. Position-only keys made these the same row, so the stack
        // kept the old view and the old name on screen.
        let before = (0, PathBuf::from("AGENTS.md"));
        let after = (0, PathBuf::from("Cargo.toml"));
        assert_ne!(picker_row_key(&before), picker_row_key(&after));
    }

    #[test]
    fn an_unchanged_row_keeps_its_key() {
        let row = (3, PathBuf::from("crates/app/Cargo.toml"));
        assert_eq!(picker_row_key(&row), picker_row_key(&row.clone()));
        // The same file at another position is another row: the hover state
        // each row captures is its position.
        assert_ne!(
            picker_row_key(&row),
            picker_row_key(&(4, PathBuf::from("crates/app/Cargo.toml")))
        );
    }
}

#[cfg(test)]
mod inline_edit_tests {
    use super::{
        fit_replacement, inline_edit_prompt, inline_edit_source, review_lines, text_around,
        INLINE_EDIT_AROUND_CHARS, INLINE_EDIT_CONTEXT_CHARS,
    };
    use std::path::Path;

    #[test]
    fn the_code_around_a_selection_goes_along_when_it_is_found_once() {
        let file = "use a;\n\nfn one() {\n    old();\n}\n\nfn two() {}\n";
        let p = inline_edit_prompt("rename old to new", "    old();\n", file);
        assert!(
            p.contains("Code before the selection:\nuse a;\n\nfn one() {\n"),
            "{p}"
        );
        assert!(p.contains("Selected code to rewrite:\n    old();\n"), "{p}");
        assert!(
            p.contains("Code after the selection:\n}\n\nfn two() {}\n"),
            "{p}"
        );

        // Twice in the file (or not at all, when the buffer has unsaved
        // edits): no context, rather than context from the wrong place.
        assert_eq!(text_around("x", "x x"), ("", ""));
        assert_eq!(text_around("missing", file), ("", ""));
        assert_eq!(text_around("  ", file), ("", ""));
    }

    #[test]
    fn the_context_is_capped_and_cut_to_whole_lines() {
        let line = "let filler = 1234567890;\n";
        let pad = line.repeat(400);
        let file = format!("{pad}fn target() {{}}\n{pad}");
        let (before, after) = text_around("fn target() {}\n", &file);
        assert!(!before.is_empty() && before.len() <= INLINE_EDIT_AROUND_CHARS);
        assert!(!after.is_empty() && after.len() <= INLINE_EDIT_AROUND_CHARS);
        assert!(before.starts_with("let filler") && before.ends_with('\n'));
        assert!(after.starts_with("let filler") && after.ends_with('\n'));

        // Multi-byte text right at the cut never splits a char.
        let wide = "é".repeat(INLINE_EDIT_AROUND_CHARS);
        let file = format!("{wide}SEL{wide}");
        let (before, after) = text_around("SEL", &file);
        assert!(before.chars().all(|c| c == 'é'));
        assert!(after.chars().all(|c| c == 'é'));
    }

    #[test]
    fn the_reply_is_fitted_to_the_selection() {
        // A fence the model added anyway is dropped.
        assert_eq!(
            fit_replacement("a()\n", "```rust\nb()\n```").unwrap(),
            "b()\n"
        );
        // Whole lines selected: one trailing line break, never none or two.
        assert_eq!(fit_replacement("a()\n", "b()").unwrap(), "b()\n");
        assert_eq!(fit_replacement("a()\n", "b()\n\n").unwrap(), "b()\n");
        assert_eq!(fit_replacement("a()\r\n", "b()\n").unwrap(), "b()\r\n");
        // Part of a line selected: no line break added, leading ones dropped.
        assert_eq!(fit_replacement("a", "\nb\n").unwrap(), "b");
        // Indentation the model kept stays.
        assert_eq!(
            fit_replacement("    a();\n", "    b();\n    c();").unwrap(),
            "    b();\n    c();\n"
        );
        assert!(fit_replacement("a", "  \n").is_err());
        assert!(fit_replacement("a", "```\n```").is_err());
    }

    #[test]
    fn the_review_lists_only_changed_lines_and_counts_the_rest() {
        let (lines, more) = review_lines("a\nb\nc\n", "a\nB\nc\nd\n", 10);
        assert_eq!(
            lines,
            vec![
                ('-', "b".to_string()),
                ('+', "B".to_string()),
                ('+', "d".to_string())
            ]
        );
        assert_eq!(more, 0);
        let new: String = (0..50).map(|i| format!("{i}\n")).collect();
        let (lines, more) = review_lines("", &new, 40);
        assert_eq!((lines.len(), more), (40, 10));
    }

    #[test]
    fn the_active_tab_wins_over_the_explorer_pick() {
        // After clicking a tab, `open_file` still names the explorer's last
        // pick; the selection comes from the active tab, so that is the file
        // the privacy check (and the file context) must use.
        let active = Path::new("/p/secrets/.env");
        let picked = Path::new("/p/src/main.rs");
        assert_eq!(
            inline_edit_source(Some(active), Some(picked)).as_deref(),
            Some(active)
        );
    }

    #[test]
    fn falls_back_to_the_explorer_pick_without_an_active_tab() {
        let picked = Path::new("/p/src/main.rs");
        assert_eq!(
            inline_edit_source(None, Some(picked)).as_deref(),
            Some(picked)
        );
        assert_eq!(inline_edit_source(None, None), None);
    }

    #[test]
    fn a_selection_scopes_the_prompt_and_leaves_unrelated_file_text_out() {
        let p = inline_edit_prompt("add docs", "fn a() {}", "SHOULD NOT APPEAR");
        assert!(p.contains("Instruction: add docs"), "{p}");
        assert!(p.ends_with("Code to rewrite:\nfn a() {}"), "{p}");
        assert!(!p.contains("SHOULD NOT APPEAR"));
    }

    #[test]
    fn without_a_selection_the_file_text_is_the_context_and_is_capped() {
        let short = inline_edit_prompt("x", "", "let a = 1;");
        assert!(short.ends_with("Code context:\nlet a = 1;"), "{short}");

        // 2-byte chars: the cap must land on a char boundary and never panic.
        let long: String = "é".repeat(INLINE_EDIT_CONTEXT_CHARS);
        let p = inline_edit_prompt("x", "", &long);
        let ctx = p.rsplit("Code context:\n").next().unwrap();
        assert!(ctx.len() <= INLINE_EDIT_CONTEXT_CHARS);
        assert!(ctx.chars().all(|c| c == 'é'));
        assert!(!ctx.is_empty());
    }
}

#[cfg(test)]
mod ex_bar_tests {
    use super::submit_ex_command;
    use crate::app::ex_command::{EditorPaneHandle, SaveOutcome, TabInfo};
    use floem::reactive::{create_rw_signal, RwSignal, SignalGet};
    use std::{cell::RefCell, io, path::PathBuf, rc::Rc};

    /// One main pane with the given `(name, dirty, active)` tabs. `saves` records every tab
    /// written; the write fails when `fail` is set.
    fn pane(
        tabs: &[(&str, bool, bool)],
        saves: Rc<RefCell<Vec<usize>>>,
        fail: bool,
    ) -> EditorPaneHandle {
        let tabs: Vec<TabInfo> = tabs
            .iter()
            .map(|&(name, dirty, active)| TabInfo {
                name: name.to_string(),
                dirty,
                active,
            })
            .collect();
        EditorPaneHandle {
            primary: true,
            tabs: Rc::new(move || tabs.clone()),
            save_tab: Rc::new(move |i| {
                saves.borrow_mut().push(i);
                if fail {
                    SaveOutcome::Failed(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Permission denied",
                    ))
                } else {
                    SaveOutcome::Saved
                }
            }),
            discard_tab: Rc::new(|_| {}),
        }
    }

    struct Bar {
        toasts: Rc<RefCell<Vec<String>>>,
        open_file: RwSignal<Option<PathBuf>>,
        quit_nonce: RwSignal<u64>,
        workspace: PathBuf,
    }

    impl Bar {
        fn new(workspace: PathBuf) -> Self {
            Bar {
                toasts: Rc::default(),
                open_file: create_rw_signal(None),
                quit_nonce: create_rw_signal(0u64),
                workspace,
            }
        }

        fn submit(&self, cmd: &str, panes: &[EditorPaneHandle]) {
            let toasts = self.toasts.clone();
            submit_ex_command(
                cmd,
                panes,
                &self.workspace,
                self.open_file,
                self.quit_nonce,
                move |msg| toasts.borrow_mut().push(msg),
            );
        }

        fn quit_requests(&self) -> u64 {
            self.quit_nonce.get()
        }
    }

    #[test]
    fn w_writes_the_active_tab_and_reports_it_without_quitting() {
        let bar = Bar::new(PathBuf::new());
        let saves = Rc::default();
        let panes = [pane(
            &[("a.rs", true, false), ("b.rs", true, true)],
            Rc::clone(&saves),
            false,
        )];

        bar.submit("w", &panes);

        assert_eq!(*saves.borrow(), vec![1]);
        assert_eq!(*bar.toasts.borrow(), vec!["Saved b.rs".to_string()]);
        assert_eq!(bar.quit_requests(), 0);
    }

    #[test]
    fn a_failed_w_shows_the_reason_and_not_saved() {
        let bar = Bar::new(PathBuf::new());
        let panes = [pane(&[("a.rs", true, true)], Rc::default(), true)];

        bar.submit("w", &panes);

        let toasts = bar.toasts.borrow();
        assert_eq!(toasts.len(), 1);
        assert!(toasts[0].contains("Permission denied"), "{}", toasts[0]);
        assert!(!toasts[0].contains("Saved"), "{}", toasts[0]);
        assert_eq!(bar.quit_requests(), 0);
    }

    #[test]
    fn wq_saves_then_asks_to_quit_exactly_once_and_stays_silent() {
        let bar = Bar::new(PathBuf::new());
        let saves = Rc::default();
        let panes = [pane(&[("a.rs", true, true)], Rc::clone(&saves), false)];

        bar.submit("wq", &panes);

        assert_eq!(*saves.borrow(), vec![0]);
        assert_eq!(bar.quit_requests(), 1);
        assert!(bar.toasts.borrow().is_empty());
    }

    #[test]
    fn a_failed_wq_does_not_ask_to_quit() {
        let bar = Bar::new(PathBuf::new());
        let panes = [pane(&[("a.rs", true, true)], Rc::default(), true)];

        bar.submit("wq", &panes);

        assert_eq!(bar.quit_requests(), 0);
        assert!(bar.toasts.borrow()[0].contains("Permission denied"));
    }

    #[test]
    fn q_over_unsaved_work_refuses_with_a_message_and_does_not_quit() {
        let bar = Bar::new(PathBuf::new());
        let saves = Rc::default();
        let panes = [pane(&[("a.rs", true, true)], Rc::clone(&saves), false)];

        bar.submit("q", &panes);

        assert_eq!(bar.quit_requests(), 0);
        assert!(saves.borrow().is_empty(), ":q never writes");
        assert!(bar.toasts.borrow()[0].contains("No write since last change"));
    }

    #[test]
    fn q_and_q_bang_ask_to_quit_when_they_may() {
        let bar = Bar::new(PathBuf::new());
        bar.submit("q", &[pane(&[("a.rs", false, true)], Rc::default(), false)]);
        assert_eq!(bar.quit_requests(), 1, "clean :q");
        bar.submit("q!", &[pane(&[("a.rs", true, true)], Rc::default(), false)]);
        assert_eq!(bar.quit_requests(), 2, ":q! over unsaved work");
        assert!(bar.toasts.borrow().is_empty());
    }

    #[test]
    fn the_command_is_trimmed_before_it_is_run() {
        let bar = Bar::new(PathBuf::new());
        bar.submit(
            "  wq \n",
            &[pane(&[("a.rs", true, true)], Rc::default(), false)],
        );
        assert_eq!(bar.quit_requests(), 1);
    }

    #[test]
    fn e_opens_an_existing_file_relative_to_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        let bar = Bar::new(dir.path().to_path_buf());

        bar.submit("e  a.rs ", &[]);

        assert_eq!(bar.open_file.get(), Some(dir.path().join("a.rs")));
        assert!(bar.toasts.borrow().is_empty());
        assert_eq!(bar.quit_requests(), 0);
    }

    #[test]
    fn e_of_a_missing_file_says_so_and_opens_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bar = Bar::new(dir.path().to_path_buf());

        bar.submit("e nope.rs", &[]);

        assert_eq!(bar.open_file.get(), None);
        assert_eq!(
            *bar.toasts.borrow(),
            vec!["No such file: nope.rs".to_string()]
        );
    }

    #[test]
    fn unknown_commands_are_reported_and_empty_ones_ignored() {
        let bar = Bar::new(PathBuf::new());
        let panes = [pane(&[("a.rs", true, true)], Rc::default(), false)];

        bar.submit("frobnicate", &panes);
        bar.submit("", &panes);
        bar.submit("   ", &panes);

        assert_eq!(
            *bar.toasts.borrow(),
            vec!["Unknown command: frobnicate".to_string()]
        );
        assert_eq!(bar.quit_requests(), 0);
        assert_eq!(bar.open_file.get(), None);
    }

    #[test]
    fn cd_is_handled_without_a_message() {
        // Change to the directory we are already in: `:cd` moves the whole process.
        let here = std::env::current_dir().unwrap();
        let bar = Bar::new(PathBuf::new());

        bar.submit(&format!("cd {}", here.display()), &[]);

        assert!(bar.toasts.borrow().is_empty());
        assert_eq!(std::env::current_dir().unwrap(), here);
    }
}
