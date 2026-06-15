//! Floating overlays: file picker, command palette, completions popup,
//! Ctrl+K inline AI edit, hover tooltip, code actions, rename, signature
//! help, toast, workspace symbols, branch picker, vim ex bar, goto, peek
//! definition. Extracted from `app.rs` as part of the Session 11 split.

use floem::{
    event::{Event, EventListener},
    ext_event::create_signal_from_channel,
    keyboard::{Key, NamedKey},
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, dyn_stack, empty, label, scroll, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::constants::ui as ui_const;
use phazeai_core::{Agent, AgentEvent, Settings};

use crate::domain_state::IdeState;
use crate::lsp_bridge::{CodeAction, CompletionEntry, LspCommand};
use crate::util::safe_get;

use super::{all_commands, show_toast, Tab};

// ── File picker overlay (Ctrl+P) ──────────────────────────────────────────────

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
            let files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&root)
                .max_depth(10)
                .into_iter()
                .flatten()
                .filter(|e| e.file_type().is_file())
                .filter(|e| {
                    let p = e.path().to_string_lossy();
                    !p.contains("/target/")
                        && !p.contains("/.git/")
                        && !p.contains("/node_modules/")
                        && !p.contains("/.cache/")
                })
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| e.into_path())
                .take(2000)
                .collect();
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

    let search_box = text_input(query).style(move |s| {
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
        dyn_stack(filtered, |(idx, _)| *idx, {
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

    let search_box = text_input(query).style(move |s| {
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
                            state.editor.pending_completion.set(Some((text, prefix_len)));
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
    Done(String),
    Err(String),
}

pub(crate) fn inline_edit_overlay(state: IdeState) -> impl IntoView {
    let open = state.ai.inline_edit_open;
    let query = state.ai.inline_edit_query;
    let ai_thinking = state.ai.thinking;

    let (update_tx, update_rx) = std::sync::mpsc::sync_channel::<InlineEditUpdate>(64);
    let update_sig = create_signal_from_channel(update_rx);

    {
        let state2 = state.clone();
        create_effect(move |_| {
            let Some(upd) = update_sig.get() else { return };
            match upd {
                InlineEditUpdate::Done(text) => {
                    state2.editor.pending_completion.set(Some((text, 0)));
                    state2.ai.thinking.set(false);
                    state2.ai.inline_edit_open.set(false);
                    state2.ai.inline_edit_query.set(String::new());
                }
                InlineEditUpdate::Err(e) => {
                    eprintln!("[PhazeAI] Ctrl+K error: {e}");
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
        .placeholder("e.g. \"add error handling\", \"convert to async\", \"add JSDoc\"")
        .style(move |s| {
            let t = state.workbench.theme.get();
            let p = &t.palette;
            s.width_full().padding_horiz(10.0).padding_vert(8.0)
             .font_size(14.0)
             .color(p.text_primary)
             .background(p.bg_elevated)
             .border(1.0)
             .border_color(p.border_focus)
             .border_radius(6.0)
        })
        .on_event_stop(EventListener::KeyDown, move |ev| {
            if let Event::KeyDown(e) = ev {
                match &e.key.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        open.set(false);
                        query.set(String::new());
                    }
                    Key::Named(NamedKey::Enter) => {
                        let instruction = query.get();
                        if instruction.is_empty() { return; }
                        ai_thinking.set(true);
                        let selection = state.editor.selected_text.get_untracked();
                        let file_ctx = state.editor.open_file.get()
                            .and_then(|p| std::fs::read_to_string(&p).ok())
                            .unwrap_or_default();
                        let file_ctx = if file_ctx.len() > 4096 {
                            file_ctx[..file_ctx.floor_char_boundary(4096)].to_string()
                        } else {
                            file_ctx
                        };
                        let prompt = if !selection.is_empty() {
                            // Selection-scoped: only rewrite the selected region.
                            format!(
                                "Rewrite ONLY the following code snippet according to the instruction. \
                                 Respond with ONLY the rewritten snippet, no explanation, no markdown fences.\n\n\
                                 Instruction: {instruction}\n\nCode to rewrite:\n{selection}"
                            )
                        } else {
                            format!(
                                "Apply the following edit to the code. \
                                 Respond with ONLY the generated code fragment, no explanation, no markdown fences.\n\n\
                                 Instruction: {instruction}\n\nCode context:\n{file_ctx}"
                            )
                        };
                        let settings = Settings::load();
                        let tx = update_tx.clone();
                        std::thread::spawn(move || {
                            let rt = match tokio::runtime::Builder::new_current_thread()
                                .enable_all().build()
                            {
                                Ok(rt) => rt,
                                Err(e) => { let _ = tx.send(InlineEditUpdate::Err(format!("Runtime: {e}"))); return; }
                            };
                            rt.block_on(async move {
                                let client = match settings.build_llm_client() {
                                    Ok(c) => c,
                                    Err(e) => { let _ = tx.send(InlineEditUpdate::Err(format!("LLM: {e}"))); return; }
                                };
                                let agent = Agent::new(client);
                                let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
                                let run_fut = agent.run_with_events(&prompt, agent_tx);
                                let drain_fut = async {
                                    let mut accumulated = String::new();
                                    while let Some(event) = agent_rx.recv().await {
                                        match event {
                                            AgentEvent::TextDelta(text) => { accumulated.push_str(&text); }
                                            AgentEvent::Complete { .. } => {
                                                let _ = tx.send(InlineEditUpdate::Done(accumulated.clone()));
                                                break;
                                            }
                                            AgentEvent::Error(e) => {
                                                let _ = tx.send(InlineEditUpdate::Err(e));
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                };
                                let _ = tokio::join!(run_fut, drain_fut);
                            });
                        });
                    }
                    _ => {}
                }
            }
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

// ── LSP Hover tooltip overlay ─────────────────────────────────────────────────

pub(crate) fn hover_tooltip(state: IdeState) -> impl IntoView {
    let hover_text = state.editor.hover_text;
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
        s.absolute()
            .inset_bottom(60.0)
            .inset_left(320.0)
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

pub(crate) fn rename_overlay(state: IdeState) -> impl IntoView {
    let open = state.editor.rename_open;
    let query = state.editor.rename_query;
    let target = state.editor.rename_target;
    let lsp_cmd = state.project.lsp_cmd.clone();
    let cursor = state.editor.active_cursor;
    let ws = state.project.workspace_root.get_untracked();

    let input = text_input(query).style(|s| s.width(320.0).padding(8.0).font_size(14.0));

    let confirm = {
        let lsp_cmd2 = lsp_cmd.clone();
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
            .on_click_stop(move |_| {
                let new_name = query.get_untracked();
                let _old = target.get_untracked();
                if let Some((path, line, col)) = cursor.get_untracked() {
                    let _ = lsp_cmd2.send(LspCommand::RequestRename {
                        path,
                        line,
                        col,
                        new_name,
                        workspace_root: ws.clone(),
                    });
                }
                open.set(false);
            })
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
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                match ke.key.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        open.set(false);
                    }
                    Key::Named(NamedKey::Enter) => {
                        open.set(false);
                    }
                    _ => {}
                }
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
                        let out = std::process::Command::new("git")
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

pub(crate) fn vim_ex_overlay(state: IdeState) -> impl IntoView {
    let open = state.editor.vim_ex_open;
    let input_sig = state.editor.vim_ex_input;
    let theme = state.workbench.theme;
    let open_file = state.editor.open_file;
    let toast = state.workbench.status_toast;
    let workspace = state.project.workspace_root;

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
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                match &ke.key.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        open.set(false);
                        input_sig.set(String::new());
                    }
                    Key::Named(NamedKey::Enter) => {
                        let cmd = input_sig.get_untracked();
                        let cmd = cmd.trim().to_string();
                        open.set(false);
                        input_sig.set(String::new());
                        match cmd.as_str() {
                            "w" | "write" => {
                                show_toast(toast, "Saved".to_string());
                            }
                            "q" | "quit" => {
                                std::process::exit(0);
                            }
                            "wq" | "x" => {
                                show_toast(toast, "Saved".to_string());
                                std::process::exit(0);
                            }
                            "wqa" | "qa" => {
                                std::process::exit(0);
                            }
                            _ if cmd.starts_with("e ") => {
                                let path = cmd[2..].trim();
                                let full = workspace.get_untracked().join(path);
                                if full.exists() {
                                    open_file.set(Some(full));
                                } else {
                                    show_toast(toast, format!("No such file: {path}"));
                                }
                            }
                            _ if cmd.starts_with("cd ") => {
                                let dir = cmd[3..].trim();
                                let _ = std::env::set_current_dir(dir);
                            }
                            _ => {
                                if !cmd.is_empty() {
                                    show_toast(toast, format!("Unknown command: {cmd}"));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

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
        .on_event_stop(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(ke) = e {
                match &ke.key.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        open.set(false);
                        input_sig.set(String::new());
                    }
                    Key::Named(NamedKey::Enter) => {
                        let text = input_sig.get_untracked();
                        open.set(false);
                        input_sig.set(String::new());
                        let parts: Vec<&str> = text.trim().splitn(2, ':').collect();
                        if let Ok(line) = parts[0].parse::<u32>() {
                            goto_line.set(line.saturating_sub(1));
                        } else {
                            show_toast(toast, format!("Invalid: {text}"));
                        }
                    }
                    _ => {}
                }
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
