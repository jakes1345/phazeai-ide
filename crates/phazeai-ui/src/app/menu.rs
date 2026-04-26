//! Top-of-window menu bar (File / Edit / View / Go / Run / Help).
//! Extracted from `app.rs` as part of the Session 11 split.

use floem::{
    action::show_context_menu,
    menu::{Menu, MenuItem},
    reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{container, label, stack, Decorators},
    IntoView,
};

use crate::domain_state::IdeState;
use crate::lsp_bridge::LspCommand;
use crate::theme::{PhazeTheme, ThemeVariant};

use super::Tab;

pub(crate) fn menu_bar(state: IdeState) -> impl IntoView {
    // Helper: a hoverable menu-bar label.
    let make_item = |label_text: &'static str, theme: RwSignal<PhazeTheme>| {
        let hovered = create_rw_signal(false);
        container(label(move || label_text))
            .style(move |sty| {
                let t = theme.get();
                let p = &t.palette;
                sty.padding_horiz(16.0)
                    .height(28.0)
                    .items_center()
                    .justify_center()
                    .cursor(floem::style::CursorStyle::Pointer)
                    .font_size(13.0)
                    .color(p.text_secondary)
                    .apply_if(hovered.get(), |s| {
                        s.background(p.bg_elevated).color(p.text_primary)
                    })
            })
            .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
                hovered.set(true);
            })
            .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
                hovered.set(false);
            })
    };

    // ── File menu ────────────────────────────────────────────────────────────
    let file_item = {
        let s = state.clone();
        make_item("File", state.workbench.theme).on_click_stop(move |_| {
            let s2 = s.clone();
            let s3 = s.clone();
            let s4 = s.clone();
            let menu = Menu::new("File")
                .entry(MenuItem::new("Open File…\tCtrl+O").action(move || {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        s2.editor.open_file.set(Some(path));
                    }
                }))
                .entry(MenuItem::new("Open Folder…").action(move || {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        s3.project.workspace_root.set(folder);
                        s3.workbench.file_picker_files.set(Vec::new());
                        s3.workbench.show_left_panel.set(true);
                        s3.workbench.left_panel_tab.set(Tab::Explorer);
                    }
                }))
                .separator()
                .entry(MenuItem::new("Exit").action(move || {
                    let _ = s4.clone();
                    std::process::exit(0);
                }));
            show_context_menu(menu, None);
        })
    };

    // ── Edit menu ────────────────────────────────────────────────────────────
    let edit_item = {
        let s = state.clone();
        make_item("Edit", state.workbench.theme).on_click_stop(move |_| {
            let s2 = s.clone();
            let s3 = s.clone();
            let s4 = s.clone();
            let menu = Menu::new("Edit")
                .entry(MenuItem::new("Toggle Comment\tCtrl+/").action(move || {
                    s2.editor.comment_toggle_nonce.update(|v| *v += 1);
                }))
                .separator()
                .entry(MenuItem::new("Inline AI Edit\tCtrl+K").action(move || {
                    s3.ai.inline_edit_open.set(true);
                    s3.ai.inline_edit_query.set(String::new());
                }))
                .separator()
                .entry(
                    MenuItem::new("Command Palette\tCtrl+Shift+P").action(move || {
                        s4.workbench.command_palette_open.set(true);
                    }),
                );
            show_context_menu(menu, None);
        })
    };

    // ── View menu ────────────────────────────────────────────────────────────
    let view_item = {
        let s = state.clone();
        make_item("View", state.workbench.theme).on_click_stop(move |_| {
            let s_exp = s.clone();
            let s_term = s.clone();
            let s_chat = s.clone();
            let s_zen = s.clone();
            let s_zin = s.clone();
            let s_zout = s.clone();
            let theme_menu = Menu::new("Theme")
                .entry(MenuItem::new("Midnight Blue").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::MidnightBlue));
                    }
                }))
                .entry(MenuItem::new("Cyberpunk 2077").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::Cyberpunk));
                    }
                }))
                .entry(MenuItem::new("Synthwave '84").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::Synthwave84));
                    }
                }))
                .entry(MenuItem::new("Andromeda").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::Andromeda));
                    }
                }))
                .entry(MenuItem::new("Dark").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme.set(PhazeTheme::from_variant(ThemeVariant::Dark));
                    }
                }))
                .entry(MenuItem::new("Dracula").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme.set(PhazeTheme::from_variant(ThemeVariant::Dracula));
                    }
                }))
                .entry(MenuItem::new("Tokyo Night").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::TokyoNight));
                    }
                }))
                .entry(MenuItem::new("Monokai").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme.set(PhazeTheme::from_variant(ThemeVariant::Monokai));
                    }
                }))
                .entry(MenuItem::new("Nord Dark").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::NordDark));
                    }
                }))
                .entry(MenuItem::new("Matrix Green").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::MatrixGreen));
                    }
                }))
                .entry(MenuItem::new("Root Shell").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme
                            .set(PhazeTheme::from_variant(ThemeVariant::RootShell));
                    }
                }))
                .entry(MenuItem::new("Light").action({
                    let s = s.clone();
                    move || {
                        s.workbench.theme.set(PhazeTheme::from_variant(ThemeVariant::Light));
                    }
                }));

            let menu = Menu::new("View")
                .entry(MenuItem::new("Explorer\tCtrl+B").action(move || {
                    s_exp.workbench.show_left_panel.update(|v| *v = !*v);
                    let open = s_exp.workbench.show_left_panel.get();
                    s_exp.workbench.left_panel_width.set(if open { 260.0 } else { 0.0 });
                }))
                .entry(MenuItem::new("Terminal\tCtrl+J").action(move || {
                    s_term.workbench.show_bottom_panel.update(|v| *v = !*v);
                }))
                .entry(MenuItem::new("AI Chat\tCtrl+\\").action(move || {
                    s_chat.workbench.show_right_panel.update(|v| *v = !*v);
                }))
                .separator()
                .entry(MenuItem::new("Zoom In\tCtrl+=").action(move || {
                    s_zin.editor.font_size.update(|v| *v = (*v + 1).min(32));
                }))
                .entry(MenuItem::new("Zoom Out\tCtrl+-").action(move || {
                    s_zout.editor.font_size.update(|v| *v = v.saturating_sub(1).max(8));
                }))
                .separator()
                .entry(MenuItem::new("Zen Mode\tCtrl+Shift+Z").action(move || {
                    s_zen.workbench.zen_mode.update(|v| *v = !*v);
                }))
                .separator()
                .entry(theme_menu);
            show_context_menu(menu, None);
        })
    };

    // ── Go menu ──────────────────────────────────────────────────────────────
    let go_item = {
        let s = state.clone();
        make_item("Go", state.workbench.theme).on_click_stop(move |_| {
            let s_def = s.clone();
            let s_sym = s.clone();
            let s_fp = s.clone();
            let menu = Menu::new("Go")
                .entry(MenuItem::new("Go to Definition\tF12").action(move || {
                    if let Some((path, line, col)) = s_def.editor.active_cursor.get() {
                        let _ =
                            s_def
                                .project.lsp_cmd
                                .send(LspCommand::RequestDefinition { path, line, col });
                    }
                }))
                .entry(
                    MenuItem::new("Find All References\tShift+F12").action(move || {
                        if let Some((path, line, col)) = s_sym.editor.active_cursor.get() {
                            let _ = s_sym.project.lsp_cmd.send(LspCommand::RequestReferences {
                                path,
                                line,
                                col,
                            });
                            s_sym.editor.references_visible.set(true);
                            s_sym.workbench.show_bottom_panel.set(true);
                            s_sym.workbench.bottom_panel_tab.set(Tab::References);
                        }
                    }),
                )
                .entry(MenuItem::new("Workspace Symbols\tCtrl+T").action(move || {
                    s_fp.editor.ws_syms_open.set(true);
                    s_fp.editor.ws_syms_query.set(String::new());
                    let _ = s_fp.project.lsp_cmd.send(LspCommand::RequestWorkspaceSymbols {
                        query: String::new(),
                    });
                }));
            show_context_menu(menu, None);
        })
    };

    // ── Run menu ─────────────────────────────────────────────────────────────
    let run_item = {
        let s = state.clone();
        make_item("Run", state.workbench.theme).on_click_stop(move |_| {
            let s_run = s.clone();
            let s_build = s.clone();
            let s_test = s.clone();
            let menu = Menu::new("Run")
                .entry(MenuItem::new("Open Terminal\tCtrl+J").action(move || {
                    s_run.workbench.show_bottom_panel.set(true);
                    s_run.workbench.bottom_panel_tab.set(Tab::Terminal);
                }))
                .separator()
                .entry(MenuItem::new("Show Build Output").action(move || {
                    s_build.workbench.show_bottom_panel.set(true);
                    s_build.workbench.bottom_panel_tab.set(Tab::Output);
                }))
                .entry(
                    MenuItem::new("Show Problems\tCtrl+Shift+M").action(move || {
                        s_test.workbench.show_bottom_panel.set(true);
                        s_test.workbench.bottom_panel_tab.set(Tab::Problems);
                    }),
                );
            show_context_menu(menu, None);
        })
    };

    // ── Help menu ────────────────────────────────────────────────────────────
    let help_item = {
        let s = state.clone();
        make_item("Help", state.workbench.theme).on_click_stop(move |_| {
            let s2 = s.clone();
            let menu = Menu::new("Help")
                .entry(
                    MenuItem::new("Command Palette\tCtrl+Shift+P").action(move || {
                        s2.workbench.command_palette_open.set(true);
                    }),
                )
                .separator()
                .entry(MenuItem::new("About PhazeAI IDE").action(|| {
                    rfd::MessageDialog::new()
                        .set_title("About PhazeAI IDE")
                        .set_description(format!(
                            "PhazeAI IDE v{}\n\n\
                            AI-native code editor built in Rust.\n\n\
                            MIT License\n\n\
                            https://github.com/jakes1345/phazeai-ide",
                            env!("CARGO_PKG_VERSION")
                        ))
                        .set_buttons(rfd::MessageButtons::Ok)
                        .show();
                }));
            show_context_menu(menu, None);
        })
    };

    // ── Bar layout ───────────────────────────────────────────────────────────
    let bar_state = state.clone();
    stack((
        file_item, edit_item, view_item, go_item, run_item, help_item,
    ))
    .style(move |s| {
        let t = bar_state.workbench.theme.get();
        let p = &t.palette;
        s.flex_row()
            .width_full()
            .height(28.0)
            .min_height(28.0)
            .background(p.bg_deep)
            .border_bottom(1.0)
            .border_color(p.glass_border.with_alpha(0.25))
            .items_center()
            .padding_left(4.0)
            .z_index(10)
    })
}
