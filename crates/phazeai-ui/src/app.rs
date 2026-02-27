use std::path::PathBuf;

use floem::{
    event::{Event, EventListener},
    ext_event::{create_ext_action, create_signal_from_channel},
    keyboard::{Key, Modifiers, NamedKey},
    peniko::kurbo::Size,
    reactive::{create_effect, create_rw_signal, RwSignal, Scope, SignalGet, SignalUpdate},
    views::{canvas, container, dyn_stack, empty, label, scroll, stack, text_input, Decorators},
    window::WindowConfig,
    Application, IntoView, Renderer,
};
use phazeai_core::Settings;

use crate::lsp_bridge::{start_lsp_bridge, CompletionEntry, DiagEntry, DiagSeverity, LspCommand};

use crate::{
    components::icon::{icons, phaze_icon},
    panels::{
        ai_panel::ai_panel,
        chat::chat_panel,
        editor::editor_panel,
        explorer::explorer_panel,
        git::git_panel,
        search,
        settings::settings_panel,
        terminal::terminal_panel,
    },
    theme::{PhazeTheme, ThemeVariant},
};
use std::time::Duration;

/// Global IDE state shared across all panels via Floem reactive system.
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub path: std::path::PathBuf,
    pub line: usize,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Explorer,
    Search,
    Git,
    AI,
    Settings,
    Terminal,
    Chat,
    Extensions,
    Account,
    Debug,
    Remote,
    Containers,
    Makefile,
    GitHub,
    Problems,
    Output,
    Ports,
    DebugConsole,
}

#[derive(Clone, Debug)]
pub struct IdeState {
    pub theme: RwSignal<PhazeTheme>,
    pub left_panel_tab: RwSignal<Tab>,
    pub bottom_panel_tab: RwSignal<Tab>,
    pub show_left_panel: RwSignal<bool>,
    pub show_right_panel: RwSignal<bool>,
    pub show_bottom_panel: RwSignal<bool>,
    pub open_file: RwSignal<Option<PathBuf>>,
    pub workspace_root: RwSignal<PathBuf>,
    /// Set to `true` while the AI chat panel is processing a request.
    /// Shared with the editor's sentient gutter so it glows during inference.
    pub ai_thinking: RwSignal<bool>,
    /// Rendered width of the left sidebar (260.0 when open, 0.0 when closed).
    /// TODO: drive this through a smooth animation loop once Floem exposes a
    /// stable `create_animation` / `spring` API.  For now it snaps immediately.
    pub left_panel_width: RwSignal<f64>,
    /// Real git branch — populated async from `git rev-parse --abbrev-ref HEAD`.
    pub git_branch: RwSignal<String>,
    /// Whether the command palette overlay is visible.
    pub command_palette_open: RwSignal<bool>,
    /// Search text typed in the command palette.
    pub command_palette_query: RwSignal<String>,
    /// Whether the Ctrl+P file picker overlay is visible.
    pub file_picker_open: RwSignal<bool>,
    /// Query text in the file picker.
    pub file_picker_query: RwSignal<String>,
    /// All workspace files, populated async when picker opens.
    pub file_picker_files: RwSignal<Vec<std::path::PathBuf>>,
    // Search
    pub search_query: RwSignal<String>,
    pub search_results: RwSignal<Vec<SearchResult>>,
    // LSP — populated async by start_lsp_bridge()
    pub diagnostics: RwSignal<Vec<DiagEntry>>,
    pub lsp_cmd: tokio::sync::mpsc::UnboundedSender<LspCommand>,
    /// Latest completion list from the LSP server (set after RequestCompletions).
    pub completions: RwSignal<Vec<CompletionEntry>>,
    /// Whether the completion popup is visible.
    pub completion_open: RwSignal<bool>,
    /// Index of the highlighted item in the completion popup.
    pub completion_selected: RwSignal<usize>,
    /// Active cursor position: (path, 0-based line, 0-based col).
    /// Written by the editor; read by Ctrl+Space handler to know where to request.
    pub active_cursor: RwSignal<Option<(PathBuf, u32, u32)>>,
    // Panel resize drag state (used by the divider + overlay)
    pub panel_drag_active: RwSignal<bool>,
    pub panel_drag_start_x: RwSignal<f64>,
    pub panel_drag_start_width: RwSignal<f64>,
}

impl IdeState {
    pub fn new(settings: &Settings) -> Self {
        let _theme = PhazeTheme::from_str(&settings.editor.theme);
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let git_branch = create_rw_signal("main".to_string());

        // Spawn a background thread to read the real git branch and push it to
        // the signal via a sync channel + create_signal_from_channel.
        let (branch_tx, branch_rx) = std::sync::mpsc::sync_channel::<String>(1);
        std::thread::spawn(move || {
            let branch = std::process::Command::new("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .output()
                .ok()
                .and_then(|out| {
                    if out.status.success() {
                        String::from_utf8(out.stdout)
                            .ok()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "main".to_string());
            let _ = branch_tx.send(branch);
        });

        // create_signal_from_channel hooks the mpsc receiver into Floem's reactive
        // system: whenever the channel receives a value the returned signal updates.
        let branch_signal = create_signal_from_channel(branch_rx);
        // Propagate the channel signal into the writable git_branch signal via effect.
        let git_branch_clone = git_branch;
        create_effect(move |_| {
            if let Some(b) = branch_signal.get() {
                git_branch_clone.set(b);
            }
        });

        // Create open_file early so we can wire the LSP effect before returning Self.
        let open_file: RwSignal<Option<PathBuf>> = create_rw_signal(None);

        // Start LSP bridge — background tokio thread running LspManager.
        // Must be called in a Floem reactive scope (we're inside the window callback).
        let (lsp_cmd, diagnostics, completions) = start_lsp_bridge(workspace.clone());

        // Wire: whenever the active file changes, send did_open to the LSP server.
        {
            let lsp_tx = lsp_cmd.clone();
            create_effect(move |_| {
                if let Some(path) = open_file.get() {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        let _ = lsp_tx.send(LspCommand::OpenFile { path, text });
                    }
                }
            });
        }

        Self {
            theme: create_rw_signal(PhazeTheme::from_variant(ThemeVariant::MidnightBlue)),
            left_panel_tab: create_rw_signal(Tab::Explorer),
            bottom_panel_tab: create_rw_signal(Tab::Terminal),
            show_left_panel: create_rw_signal(true),
            show_right_panel: create_rw_signal(true),
            show_bottom_panel: create_rw_signal(false),
            open_file,
            workspace_root: create_rw_signal(workspace),
            ai_thinking: create_rw_signal(false),
            left_panel_width: create_rw_signal(300.0), // Consistent with activity_bar_btn change
            git_branch,
            command_palette_open: create_rw_signal(false),
            command_palette_query: create_rw_signal(String::new()),
            file_picker_open: create_rw_signal(false),
            file_picker_query: create_rw_signal(String::new()),
            file_picker_files: create_rw_signal(Vec::new()),
            search_query: create_rw_signal("".to_string()),
            search_results: create_rw_signal(Vec::new()),
            diagnostics,
            lsp_cmd,
            completions,
            completion_open: create_rw_signal(false),
            completion_selected: create_rw_signal(0usize),
            active_cursor: create_rw_signal(None),
            panel_drag_active: create_rw_signal(false),
            panel_drag_start_x: create_rw_signal(0.0),
            panel_drag_start_width: create_rw_signal(0.0),
        }
    }
}

// ── Command palette commands ──────────────────────────────────────────────────

#[derive(Clone)]
struct PaletteCommand {
    label: &'static str,
    action: fn(IdeState),
}

fn all_commands() -> Vec<PaletteCommand> {
    vec![
        PaletteCommand {
            label: "Open File…",
            action: |s| {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    s.open_file.set(Some(path));
                    s.show_left_panel.set(true);
                    s.left_panel_width.set(260.0);
                }
            },
        },
        PaletteCommand {
            label: "Open Folder…",
            action: |s| {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    s.workspace_root.set(folder);
                    // Clear file picker cache so it re-walks on next open
                    s.file_picker_files.set(Vec::new());
                    s.show_left_panel.set(true);
                    s.left_panel_width.set(300.0);
                    s.left_panel_tab.set(crate::app::Tab::Explorer);
                }
            },
        },
        PaletteCommand {
            label: "Toggle Terminal",
            action: |s| { s.show_bottom_panel.update(|v| *v = !*v); },
        },
        PaletteCommand {
            label: "Toggle Explorer",
            action: |s| {
                s.show_left_panel.update(|v| *v = !*v);
                let open = s.show_left_panel.get();
                s.left_panel_width.set(if open { 260.0 } else { 0.0 });
            },
        },
        PaletteCommand {
            label: "Toggle AI Chat",
            action: |s| { s.show_right_panel.update(|v| *v = !*v); },
        },
        PaletteCommand {
            label: "Show AI Panel",
            action: |s| {
                s.left_panel_tab.set(Tab::AI);
                s.show_left_panel.set(true);
                s.left_panel_width.set(260.0);
            },
        },
        // ── All 12 themes ────────────────────────────────────────────────────
        PaletteCommand {
            label: "Theme: Midnight Blue",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::MidnightBlue)); },
        },
        PaletteCommand {
            label: "Theme: Cyberpunk 2077",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Cyberpunk)); },
        },
        PaletteCommand {
            label: "Theme: Synthwave '84",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Synthwave84)); },
        },
        PaletteCommand {
            label: "Theme: Andromeda",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Andromeda)); },
        },
        PaletteCommand {
            label: "Theme: Dark",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dark)); },
        },
        PaletteCommand {
            label: "Theme: Dracula",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Dracula)); },
        },
        PaletteCommand {
            label: "Theme: Tokyo Night",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::TokyoNight)); },
        },
        PaletteCommand {
            label: "Theme: Monokai",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Monokai)); },
        },
        PaletteCommand {
            label: "Theme: Nord Dark",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::NordDark)); },
        },
        PaletteCommand {
            label: "Theme: Matrix Green",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::MatrixGreen)); },
        },
        PaletteCommand {
            label: "Theme: Root Shell",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::RootShell)); },
        },
        PaletteCommand {
            label: "Theme: Light",
            action: |s| { s.theme.set(PhazeTheme::from_variant(ThemeVariant::Light)); },
        },
    ]
}

// ── File picker overlay (Ctrl+P) ──────────────────────────────────────────────

fn file_picker(state: IdeState) -> impl IntoView {
    let query = state.file_picker_query;
    let all_files = state.file_picker_files;
    let hovered: RwSignal<Option<usize>> = create_rw_signal(None);

    // When picker opens, walk workspace asynchronously (re-walk when root changes)
    let last_root: RwSignal<Option<std::path::PathBuf>> = create_rw_signal(None);
    create_effect(move |_| {
        if !state.file_picker_open.get() { return; }
        let root = state.workspace_root.get();
        if last_root.get().as_ref() == Some(&root) { return; }
        last_root.set(Some(root.clone()));
        let scope = Scope::new();
        let send = create_ext_action(scope, move |files: Vec<std::path::PathBuf>| {
            all_files.set(files);
        });
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
            send(files);
        });
    });

    let filtered = move || -> Vec<(usize, std::path::PathBuf)> {
        let q = query.get().to_lowercase();
        all_files
            .get()
            .into_iter()
            .filter(|p| {
                if q.is_empty() { return true; }
                let name = p.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                name.contains(&q) || p.to_string_lossy().to_lowercase().contains(&q)
            })
            .take(50)
            .enumerate()
            .collect()
    };

    let search_box = text_input(query).style(move |s| {
        let t = state.theme.get();
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
        dyn_stack(
            filtered,
            |(idx, _)| *idx,
            {
                let state = state.clone();
                move |(idx, path)| {
                    let path_clone = path.clone();
                    let root = state.workspace_root.get();
                    let display = path.strip_prefix(&root)
                        .ok()
                        .map(|r| r.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());
                    let display2 = display.clone();
                    let hov = hovered;
                    let state = state.clone();
                    container(
                        stack((
                            label(move || {
                                path_clone.file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default()
                            })
                            .style({
                                let state = state.clone();
                                move |s| {
                                    s.font_size(13.0)
                                     .color(state.theme.get().palette.text_primary)
                                }
                            }),
                            label(move || format!("  {}", display2))
                                .style({
                                    let state = state.clone();
                                    move |s| {
                                        s.font_size(11.0)
                                         .color(state.theme.get().palette.text_muted)
                                         .flex_grow(1.0)
                                    }
                                }),
                        ))
                        .style(|s| s.items_center()),
                    )
                    .style({
                        let state = state.clone();
                        move |s| {
                            let t = state.theme.get();
                            let p = &t.palette;
                            s.width_full()
                             .padding_horiz(12.0)
                             .padding_vert(7.0)
                             .border_radius(4.0)
                             .background(if hov.get() == Some(idx) { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                             .cursor(floem::style::CursorStyle::Pointer)
                        }
                    })
                    .on_click_stop({
                        let state = state.clone();
                        let path2 = path.clone();
                        move |_| {
                            state.open_file.set(Some(path2.clone()));
                            state.file_picker_open.set(false);
                            state.file_picker_query.set(String::new());
                        }
                    })
                    .on_event_stop(EventListener::PointerEnter, move |_| { hov.set(Some(idx)); })
                    .on_event_stop(EventListener::PointerLeave, move |_| { hov.set(None); })
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(360.0));

    let empty_hint = container(
        label(|| "Searching workspace files…")
            .style(move |s| {
                s.font_size(12.0).color(state.theme.get().palette.text_muted)
            }),
    )
    .style(move |s| {
        let empty = all_files.get().is_empty() && state.file_picker_open.get();
        s.width_full().padding_vert(12.0).items_center().justify_center()
         .apply_if(!empty, |s| s.display(floem::style::Display::None))
    });

    let picker_box = stack((search_box, empty_hint, items_view))
        .style({
            let state = state.clone();
            move |s| {
                let t = state.theme.get();
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
                        state.file_picker_open.set(false);
                        state.file_picker_query.set(String::new());
                    }
                }
            }
        });

    container(picker_box)
        .style({
            let state = state.clone();
            move |s| {
                let shown = state.file_picker_open.get();
                s.absolute()
                 .inset(0)
                 .items_start()
                 .justify_center()
                 .padding_top(80.0)
                 .background(floem::peniko::Color::from_rgba8(0, 0, 0, 160))
                 .z_index(200)
                 .apply_if(!shown, |s| s.display(floem::style::Display::None))
            }
        })
        .on_click_stop({
            let state = state.clone();
            move |_| {
                state.file_picker_open.set(false);
                state.file_picker_query.set(String::new());
            }
        })
}

// ── Command palette overlay ───────────────────────────────────────────────────

fn command_palette(state: IdeState) -> impl IntoView {
    let query = state.command_palette_query;

    // Build a filtered list of matching commands driven by the query signal.
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
        let t = state.theme.get();
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
        dyn_stack(
            commands_list,
            |(idx, lbl, _action)| (*idx, *lbl),
            {
                let state = state.clone();
                move |(idx, cmd_label, cmd_action)| {
                    let hovered = row_hovered;
                    let state = state.clone();
                    container(
                        label(move || cmd_label)
                            .style({
                                let state = state.clone();
                                move |s| {
                                    s.font_size(13.0)
                                     .color(state.theme.get().palette.text_primary)
                                }
                            })
                    )
                    .style({
                        let state = state.clone();
                        move |s| {
                            let t = state.theme.get();
                            let p = &t.palette;
                            let is_hov = hovered.get() == Some(idx);
                            s.width_full()
                             .padding_horiz(12.0)
                             .padding_vert(8.0)
                             .border_radius(4.0)
                             .background(if is_hov { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
                             .cursor(floem::style::CursorStyle::Pointer)
                        }
                    })
                    .on_click_stop({
                        let state = state.clone();
                        move |_| {
                            cmd_action(state.clone());
                            state.command_palette_open.set(false);
                            state.command_palette_query.set(String::new());
                        }
                    })
                    .on_event_stop(EventListener::PointerEnter, move |_| { hovered.set(Some(idx)); })
                    .on_event_stop(EventListener::PointerLeave, move |_| { hovered.set(None); })
                }
            }
        )
        .style(|s| s.flex_col().width_full())
    )
    .style(|s| s.width_full().max_height(320.0));

    let palette_box = stack((search_box, items_view))
        .style({
            let state = state.clone();
            move |s| {
                let t = state.theme.get();
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
    // Escape key closes the palette when it has focus
    .on_event_stop(EventListener::KeyDown, {
        let state = state.clone();
        move |event| {
            if let Event::KeyDown(e) = event {
                if e.key.logical_key == Key::Named(floem::keyboard::NamedKey::Escape) {
                    state.command_palette_open.set(false);
                    state.command_palette_query.set(String::new());
                }
            }
        }
    });

    // Dimmed full-screen backdrop — centers the palette box.
    // Clicking the backdrop (outside the palette) dismisses it.
    container(palette_box)
        .style({
            let state = state.clone();
            move |s| {
                let shown = state.command_palette_open.get();
                s.absolute()
                 .inset(0)
                 .items_center()
                 .justify_center()
                 .background(floem::peniko::Color::from_rgba8(0, 0, 0, 180))
                 .z_index(100)
                 .apply_if(!shown, |s| s.display(floem::style::Display::None))
            }
        })
        .on_click_stop({
            let state = state.clone();
            move |_| {
                state.command_palette_open.set(false);
                state.command_palette_query.set(String::new());
            }
        })
}

/// Cosmic nebula canvas — absolute-positioned behind all UI panels.
/// Uses blurred fills to paint soft gaseous clouds in electric purple/indigo.
fn cosmic_bg_canvas(theme: RwSignal<PhazeTheme>) -> impl IntoView {
    canvas(move |cx, size| {
        let t = theme.get();
        let p = &t.palette;
        let w = size.width;
        let h = size.height;

        // 1. Deep-space base fill (near black)
        cx.fill(
            &floem::kurbo::Rect::ZERO.with_size(size),
            // Use a color even deeper than bg_base for the canvas
            floem::peniko::Color::from_rgb8(2, 2, 8),
            0.0,
        );

        if !t.is_cosmic() {
            return;
        }

        // 2. Technical Hexagonal Grid
        // Very subtle, dark blue lines to give it a "engineered" feel
        let hex_size = 40.0;
        let grid_color = p.accent.with_alpha(0.08); // Slightly more visible grid
        let horiz_dist = hex_size * 3.0f64.sqrt();
        let vert_dist = hex_size * 1.5;

        for row in 0..((h / vert_dist) as i32 + 2) {
            for col in 0..((w / horiz_dist) as i32 + 2) {
                let x_offset = if row % 2 == 1 { horiz_dist / 2.0 } else { 0.0 };
                let x = col as f64 * horiz_dist + x_offset;
                let y = row as f64 * vert_dist;

                // Draw a small hex-point
                cx.fill(
                    &floem::kurbo::Circle::new(floem::kurbo::Point::new(x, y), 0.8),
                    grid_color,
                    0.0,
                );
            }
        }

        // 3. Targeted Neon Glows (The "Light Leaks")
        // Top-Left: Deep Purple nebula glow
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.1, h * 0.1), 450.0),
            floem::peniko::Color::from_rgba8(80, 40, 200, 70),
            140.0,
        );

        // Right-Center: Cyber Cyan Glow (matches mockup light leak)
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.95, h * 0.4), 350.0),
            floem::peniko::Color::from_rgba8(0, 180, 255, 75),
            120.0,
        );

        // Bottom-Right: Deep Indigo/Blue
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.8, h * 0.8), 400.0),
            floem::peniko::Color::from_rgba8(20, 10, 150, 65),
            130.0,
        );

        // Center-Bottom: Indigo/Violet cloud (4th nebula — grounds the composition)
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.45, h * 0.85), 320.0),
            floem::peniko::Color::from_rgba8(100, 30, 200, 55),
            120.0,
        );

        // Subtle radial vignette overlay — darkens corners to focus the eye centrally
        cx.fill(
            &floem::kurbo::Circle::new(floem::kurbo::Point::new(w * 0.5, h * 0.5), (w.max(h)) * 0.65),
            floem::peniko::Color::from_rgba8(0, 0, 8, 40),
            80.0,
        );
    })
    // Absolute-positioned so it doesn't participate in flex layout but
    // covers the full parent container — rendered below all siblings.
    .style(|s| s.absolute().inset(0))
}

fn activity_bar_btn(
    icon_svg: &'static str,
    tab: Tab,
    state: IdeState,
) -> impl IntoView {
    let is_hovered = create_rw_signal(false);
    let active = move || state.left_panel_tab.get() == tab && state.show_left_panel.get();
    
    let icon_color = move |p: &crate::theme::PhazePalette| {
        if active() { p.accent } else { p.text_secondary }
    };

    container(phaze_icon(icon_svg, 22.0, icon_color, state.theme))
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        let is_active = active();
        let is_hov = is_hovered.get();
        
        s.width(42.0)
         .height(42.0)
         .border_radius(10.0)
         .items_center()
         .justify_center()
         .cursor(floem::style::CursorStyle::Pointer)
         .margin_bottom(6.0)
         .transition(floem::style::Background, floem::style::Transition::linear(Duration::from_millis(150)))
         .apply_if(is_active, |s| {
             s.background(p.accent_dim)
              .box_shadow_blur(16.0)
              .box_shadow_color(p.glow)
              .box_shadow_spread(1.0)
         })
         .apply_if(is_hov && !is_active, |s| {
             s.background(p.bg_surface.with_alpha(0.3))
         })
    })
    .on_click_stop(move |_| {
        if state.left_panel_tab.get() == tab && state.show_left_panel.get() {
            state.show_left_panel.set(false);
            state.left_panel_width.set(0.0);
        } else {
            state.left_panel_tab.set(tab);
            state.show_left_panel.set(true);
            state.left_panel_width.set(300.0); // Slightly wider sidebar for premium feel
        }
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
        is_hovered.set(true);
    })
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
        is_hovered.set(false);
    })
}

fn activity_bar(state: IdeState) -> impl IntoView {
    stack((
        activity_bar_btn(icons::EXPLORER, Tab::Explorer, state.clone()),
        activity_bar_btn(icons::SEARCH, Tab::Search, state.clone()),
        activity_bar_btn(icons::SOURCE_CONTROL, Tab::Git, state.clone()), // Use SOURCE_CONTROL icon for Git
        activity_bar_btn(icons::AI, Tab::AI, state.clone()),
        activity_bar_btn(icons::DEBUG, Tab::Debug, state.clone()),
        activity_bar_btn(icons::REMOTE, Tab::Remote, state.clone()),
        activity_bar_btn(icons::CONTAINER, Tab::Containers, state.clone()),
        activity_bar_btn(icons::LIST_CHECKS, Tab::Makefile, state.clone()),
        activity_bar_btn(icons::GITHUB, Tab::GitHub, state.clone()),
        stack((
            activity_bar_btn(icons::EXTENSIONS, Tab::Extensions, state.clone()),
            activity_bar_btn(icons::SETTINGS, Tab::Settings, state.clone()),
            activity_bar_btn(icons::ACCOUNT, Tab::Account, state.clone()),
        ))
        .style(|s| s.flex_col().margin_top(floem::unit::PxPctAuto::Auto)),
    ))
    .style(|s| s.flex_col().padding(8.0).gap(2.0))
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.flex_col()
         .width(48.0)
         .height_full()
         .background(p.glass_bg)
         .border_right(1.0)
         .border_color(p.glass_border)
         .justify_between()
         .box_shadow_h_offset(3.0)
         .box_shadow_v_offset(0.0)
         .box_shadow_blur(16.0)
         .box_shadow_color(p.glow)
         .box_shadow_spread(0.0)
    })
}

fn placeholder_tab(tab_name: &'static str, state: IdeState) -> impl IntoView {
    container(
        label(move || tab_name)
            .style(move |s| {
                s.color(state.theme.get().palette.text_muted)
                 .font_size(12.0)
                 .padding(16.0)
            }),
    )
    .style(move |s| {
        let t = state.theme.get();
        s.width_full().height_full().background(t.palette.glass_bg)
    })
}

fn left_panel(state: IdeState) -> impl IntoView {
    let explorer = explorer_panel(
        state.workspace_root,
        state.open_file,
        state.theme,
    );

    let explorer_wrap = container(explorer).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Explorer, |s| s.display(floem::style::Display::None))
        }
    });

    let search_wrap = container(search::search_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Search, |s| s.display(floem::style::Display::None))
        }
    });

    let git_wrap = container(git_panel(state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Git, |s| s.display(floem::style::Display::None))
        }
    });

    let debug_wrap = container(placeholder_tab("Run and Debug", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Debug, |s| s.display(floem::style::Display::None))
        }
    });

    let extensions_wrap = container(placeholder_tab("Extensions", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Extensions, |s| s.display(floem::style::Display::None))
        }
    });

    let remote_wrap = container(placeholder_tab("Remote Explorer", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Remote, |s| s.display(floem::style::Display::None))
        }
    });

    let container_wrap = container(placeholder_tab("Containers", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Containers, |s| s.display(floem::style::Display::None))
        }
    });

    let makefile_wrap = container(placeholder_tab("Makefile", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Makefile, |s| s.display(floem::style::Display::None))
        }
    });

    let github_wrap = container(placeholder_tab("GitHub Actions", state.clone())).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::GitHub, |s| s.display(floem::style::Display::None))
        }
    });

    let ai_wrap = container(ai_panel(state.theme)).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::AI, |s| s.display(floem::style::Display::None))
        }
    });

    let settings_wrap = container(settings_panel(state.theme)).style({
        let state = state.clone();
        move |s| {
            s.width_full().height_full()
             .apply_if(state.left_panel_tab.get() != Tab::Settings, |s| s.display(floem::style::Display::None))
        }
    });

    container(
        stack((
            explorer_wrap,
            search_wrap,
            git_wrap,
            debug_wrap,
            extensions_wrap,
            remote_wrap,
            container_wrap,
            makefile_wrap,
            github_wrap,
            ai_wrap,
            settings_wrap,
        ))
        .style(|s| s.width_full().height_full()),
    )
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        let show = state.show_left_panel.get();
        let width = if show { state.left_panel_width.get() } else { 0.0 };
        s.width(width)
         .height_full()
         .background(p.glass_bg)
         .border_right(1.0)
         .border_color(p.glass_border)
         .box_shadow_h_offset(6.0)
         .box_shadow_v_offset(0.0)
         .box_shadow_blur(12.0)
         .box_shadow_color(p.glow)
         .box_shadow_spread(0.0)
         .apply_if(!show, |s| s.display(floem::style::Display::None))
    })
}

fn bottom_panel_tab(
    label_str: &'static str,
    tab: Tab,
    state: IdeState,
) -> impl IntoView {
    let is_hovered = create_rw_signal(false);
    container(label(move || label_str))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            let active = state.bottom_panel_tab.get() == tab;
            let hovered = is_hovered.get();
            s.padding_horiz(12.0)
             .padding_vert(6.0)
             .font_size(11.0)
             .color(if active { p.accent } else { p.text_muted })
             .background(if active { p.bg_surface } else if hovered { p.bg_elevated } else { floem::peniko::Color::TRANSPARENT })
             .cursor(floem::style::CursorStyle::Pointer)
             .apply_if(active, |s| s.border_top(2.0).border_color(p.accent))
        })
        .on_click_stop(move |_| {
            state.bottom_panel_tab.set(tab);
            state.show_bottom_panel.set(true);
        })
        .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
            is_hovered.set(true);
        })
        .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
            is_hovered.set(false);
        })
}

fn status_bar(state: IdeState) -> impl IntoView {
    let left = stack((
        phaze_icon(icons::BRANCH, 12.0, move |p| p.accent, state.theme),
        // Real git branch — updated async on startup via git rev-parse.
        label(move || format!(" {}", state.git_branch.get()))
            .style(move |s| s.color(state.theme.get().palette.text_secondary).font_size(11.0)),
        label(|| "   ")
            .style(|s| s.font_size(11.0)),
        phaze_icon(icons::BRANCH, 12.0, move |p| p.accent, state.theme), // Using BRANCH for now as robot emoji placeholder
        label(move || {
            let s = Settings::load();
            format!(" {}", s.llm.model)
        })
        .style(move |s| s.color(state.theme.get().palette.text_secondary).font_size(11.0)),
    ))
    .style(|s| s.items_center().padding_horiz(8.0));

    let right = stack((
        // LSP diagnostic counts — live from the reactive diagnostics signal.
        label(move || {
            let diags = state.diagnostics.get();
            let errs = diags.iter().filter(|d| d.severity == DiagSeverity::Error).count();
            let warns = diags.iter().filter(|d| d.severity == DiagSeverity::Warning).count();
            if errs == 0 && warns == 0 {
                String::new()
            } else {
                format!("⊗ {errs}  ⚠ {warns}  ")
            }
        })
        .style(move |s| {
            let p = state.theme.get().palette;
            let has_errs = state.diagnostics.get().iter().any(|d| d.severity == DiagSeverity::Error);
            s.font_size(11.0).color(if has_errs { p.error } else { p.warning })
        }),
        label(|| "AI Ready  ")
            .style(move |s| s.color(state.theme.get().palette.success).font_size(11.0)),
        label(|| "UTF-8  ")
            .style(move |s| s.color(state.theme.get().palette.text_muted).font_size(11.0)),
        label(move || {
            state.open_file.get()
                .as_ref()
                .and_then(|p| p.extension())
                .map(|e| match e.to_str().unwrap_or("") {
                    "rs" => "Rust  ",
                    "py" => "Python  ",
                    "js" | "ts" => "TypeScript  ",
                    "toml" => "TOML  ",
                    "md" => "Markdown  ",
                    _ => "Text  ",
                })
                .unwrap_or("  ")
                .to_string()
        })
        .style(move |s| s.color(state.theme.get().palette.text_muted).font_size(11.0)),
    ))
    .style(|s| s.items_center().padding_horiz(8.0));

    stack((left, right))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            s.height(24.0)
             .width_full()
             .background(p.glass_bg)
             .border_top(1.0)
             .border_color(p.glass_border)
             .items_center()
             .justify_between()
             // Neon top-edge glow on status bar
             .box_shadow_h_offset(0.0)
             .box_shadow_v_offset(-2.0)
             .box_shadow_blur(14.0)
             .box_shadow_color(p.glow)
             .box_shadow_spread(0.0)
        })
}

fn problems_view(state: IdeState) -> impl IntoView {
    let diags = state.diagnostics;
    let theme = state.theme;

    let empty_msg = container(
        label(move || {
            let n = diags.get().len();
            if n == 0 { "No problems detected.".to_string() }
            else { format!("{n} problem(s)") }
        })
        .style(move |s| s.font_size(12.0).color(theme.get().palette.text_muted)),
    )
    .style(move |s| {
        s.width_full()
         .padding(16.0)
         .apply_if(!diags.get().is_empty(), |s| s.display(floem::style::Display::None))
    });

    let list = scroll(
        dyn_stack(
            move || diags.get().into_iter().enumerate().collect::<Vec<_>>(),
            |(idx, _)| *idx,
            {
                let theme = state.theme;
                move |(_, entry): (usize, DiagEntry)| {
                    let (icon, sev) = match entry.severity {
                        DiagSeverity::Error   => ("●", entry.severity),
                        DiagSeverity::Warning => ("▲", entry.severity),
                        DiagSeverity::Info    => ("ℹ", entry.severity),
                        DiagSeverity::Hint    => ("○", entry.severity),
                    };
                    let filename = entry.path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "?".to_string());
                    let text = format!("{icon} {filename}:{} — {}", entry.line, entry.message);
                    label(move || text.clone())
                        .style(move |s| {
                            let p = theme.get().palette;
                            let color = match sev {
                                DiagSeverity::Error   => p.error,
                                DiagSeverity::Warning => p.warning,
                                _                     => p.text_muted,
                            };
                            s.font_size(12.0).color(color).padding_horiz(12.0).padding_vert(3.0).width_full()
                        })
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(move |s| {
        s.width_full().flex_grow(1.0)
         .apply_if(diags.get().is_empty(), |s| s.display(floem::style::Display::None))
    });

    stack((empty_msg, list))
        .style(|s| s.flex_col().width_full().height_full())
}

fn bottom_panel(state: IdeState) -> impl IntoView {
    let current_tab = state.bottom_panel_tab;

    container(
        stack((
            // Tab bar
            stack((
                bottom_panel_tab("TERMINAL", Tab::Terminal, state.clone()),
                bottom_panel_tab("PROBLEMS", Tab::Problems, state.clone()),
                bottom_panel_tab("OUTPUT", Tab::Output, state.clone()),
                bottom_panel_tab("DEBUG CONSOLE", Tab::DebugConsole, state.clone()),
                bottom_panel_tab("PORTS", Tab::Ports, state.clone()),
                
                // Close button
                phaze_icon(icons::CLOSE, 12.0, move |p| p.text_muted, state.theme)
                    .style(move |s| {
                        s.margin_left(floem::unit::PxPctAuto::Auto)
                         .padding(4.0)
                         .cursor(floem::style::CursorStyle::Pointer)
                    })
                    .on_click_stop(move |_| {
                        state.show_bottom_panel.set(false);
                    })
            ))
            .style(move |s| {
                let t = state.theme.get();
                s.width_full()
                 .height(32.0)
                 .background(t.palette.bg_elevated)
                 .border_bottom(1.0)
                 .border_color(t.palette.border)
                 .items_center()
                 .padding_horiz(12.0)
                 .gap(16.0)
            }),
            
            // Content
            stack((
                container(terminal_panel(state.theme))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::Terminal, |s| s.display(floem::style::Display::None))
                    }),
                container(problems_view(state.clone()))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::Problems, |s| s.display(floem::style::Display::None))
                    }),
                container(label(|| "Output content..."))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::Output, |s| s.display(floem::style::Display::None))
                    }),
                container(label(|| "Debug Console content..."))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::DebugConsole, |s| s.display(floem::style::Display::None))
                    }),
                container(label(|| "Ports content..."))
                    .style(move |s| {
                        s.width_full()
                         .height_full()
                         .apply_if(current_tab.get() != Tab::Ports, |s| s.display(floem::style::Display::None))
                    }),
            ))
            .style(|s| s.flex_grow(1.0).width_full())
        ))
        .style(|s| s.flex_col().width_full().height_full())
    )
    .style(move |s| {
        let t = state.theme.get();
        let p = &t.palette;
        s.flex_col()
         .height(240.0)
         .width_full()
         .background(p.glass_bg)
         .border_top(1.0)
         .border_color(p.glass_border)
         .box_shadow_h_offset(0.0)
         .box_shadow_v_offset(-4.0)
         .box_shadow_blur(10.0)
         .box_shadow_color(p.glow)
         .box_shadow_spread(0.0)
         .apply_if(!state.show_bottom_panel.get(), |s| s.display(floem::style::Display::None))
    })
}

fn completion_popup(state: IdeState) -> impl IntoView {
    let items    = state.completions;
    let selected = state.completion_selected;

    let list = scroll(
        dyn_stack(
            move || items.get().into_iter().enumerate().collect::<Vec<_>>(),
            |(idx, _)| *idx,
            {
                let state = state.clone();
                move |(idx, entry): (usize, CompletionEntry)| {
                    let is_sel = move || selected.get() == idx;
                    let item_detail = entry.detail.clone().unwrap_or_default();
                    let item_label  = entry.label.clone();
                    stack((
                        label(move || item_label.clone())
                            .style(move |s: floem::style::Style| {
                                let p = state.theme.get().palette;
                                s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
                            }),
                        label(move || item_detail.clone())
                            .style(move |s: floem::style::Style| {
                                let p = state.theme.get().palette;
                                s.font_size(11.0).color(p.text_muted).margin_left(8.0)
                            }),
                    ))
                    .style(move |s: floem::style::Style| {
                        let p = state.theme.get().palette;
                        s.items_center()
                         .width_full()
                         .padding_horiz(12.0)
                         .padding_vert(5.0)
                         .border_radius(4.0)
                         .cursor(floem::style::CursorStyle::Pointer)
                         .background(if is_sel() { p.accent_dim } else { floem::peniko::Color::TRANSPARENT })
                    })
                    .on_click_stop({
                        let state = state.clone();
                        move |_| {
                            selected.set(idx);
                            state.completion_open.set(false);
                        }
                    })
                    .on_event_stop(EventListener::PointerEnter, move |_| selected.set(idx))
                }
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(280.0));

    let header = stack((
        label(|| "Completions")
            .style(move |s| {
                s.font_size(11.0)
                 .color(state.theme.get().palette.text_muted)
                 .flex_grow(1.0)
            }),
        container(label(|| "Esc"))
            .style(move |s| {
                let p = state.theme.get().palette;
                s.font_size(10.0)
                 .color(p.text_muted)
                 .background(p.bg_elevated)
                 .padding_horiz(5.0)
                 .padding_vert(2.0)
                 .border_radius(3.0)
                 .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| state.completion_open.set(false)),
    ))
    .style(move |s| {
        s.items_center()
         .width_full()
         .padding_horiz(12.0)
         .padding_vert(6.0)
         .border_bottom(1.0)
         .border_color(state.theme.get().palette.border)
         .margin_bottom(4.0)
    });

    let empty_hint = label(|| "No completions")
        .style(move |s| {
            s.font_size(12.0)
             .color(state.theme.get().palette.text_muted)
             .padding(12.0)
             .apply_if(!items.get().is_empty(), |s| s.display(floem::style::Display::None))
        });

    let popup_box = stack((header, empty_hint, list))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            s.flex_col()
             .width(420.0)
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
                    Key::Named(floem::keyboard::NamedKey::Escape) => {
                        state.completion_open.set(false);
                    }
                    Key::Named(floem::keyboard::NamedKey::ArrowDown) => {
                        let max = items.get().len().saturating_sub(1);
                        selected.update(|v| *v = (*v + 1).min(max));
                    }
                    Key::Named(floem::keyboard::NamedKey::ArrowUp) => {
                        selected.update(|v| *v = v.saturating_sub(1));
                    }
                    _ => {}
                }
            }
        });

    container(popup_box)
        .style(move |s| {
            let shown = state.completion_open.get();
            s.absolute()
             .inset(0)
             .items_start()
             .justify_center()
             .padding_top(120.0)
             .z_index(300)
             .background(floem::peniko::Color::TRANSPARENT)
             .apply_if(!shown, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| state.completion_open.set(false))
}

fn ide_root(state: IdeState) -> impl IntoView {
    let editor = editor_panel(
        state.open_file,
        state.theme,
        state.ai_thinking,
        state.lsp_cmd.clone(),
        state.active_cursor,
    );
    let chat = chat_panel(state.theme, state.ai_thinking);

    let chat_wrap = container(chat)
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            s.height_full()
             .background(p.glass_bg)
             .border_left(1.0)
             .border_color(p.glass_border)
             // Left-edge glow from chat panel
             .box_shadow_h_offset(-6.0)
             .box_shadow_v_offset(0.0)
             .box_shadow_blur(12.0)
             .box_shadow_color(p.glow)
             .box_shadow_spread(0.0)
             .apply_if(!state.show_right_panel.get(), |s| s.display(floem::style::Display::None))
        });

    // Drag handle between left panel and editor.
    let divider = {
        let style_s = state.clone();
        let down_s  = state.clone();
        container(empty())
            .style(move |s| {
                let t = style_s.theme.get();
                let active = style_s.panel_drag_active.get();
                let shown  = style_s.show_left_panel.get();
                s.width(4.0)
                 .height_full()
                 .cursor(floem::style::CursorStyle::ColResize)
                 .background(t.palette.glass_border.with_alpha(if active { 0.8 } else { 0.0 }))
                 .apply_if(!shown, |s| s.display(floem::style::Display::None))
            })
            .on_event_stop(EventListener::PointerDown, move |e| {
                if let Event::PointerDown(pe) = e {
                    down_s.panel_drag_active.set(true);
                    down_s.panel_drag_start_x.set(pe.pos.x);
                    down_s.panel_drag_start_width.set(down_s.left_panel_width.get());
                }
            })
    };

    // Main content row: activity bar + left panel + resize handle + editor + chat
    let content_row = stack((
        activity_bar(state.clone()),
        left_panel(state.clone()),
        divider,
        editor,
        chat_wrap,
    ))
    .style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    // Bottom panel (terminal etc.)
    let bottom = bottom_panel(state.clone());
    let state_for_status = state.clone();

    stack((content_row, bottom, status_bar(state_for_status)))
        .style(move |s| {
            let t = state.theme.get();
            let p = &t.palette;
            s.flex_col()
             .width_full()
             .height_full()
             .background(if t.is_cosmic() { floem::peniko::Color::TRANSPARENT } else { p.bg_base })
             .color(p.text_primary)
        })
}

/// Launch the PhazeAI IDE.
pub fn launch_phaze_ide() {
    let settings = Settings::load();

    Application::new()
        .window(
            move |_| {
                let state = IdeState::new(&settings);

                // Overlay layers — rendered after IDE content so they paint on top.
                let palette    = command_palette(state.clone());
                let picker     = file_picker(state.clone());
                let completions_popup = completion_popup(state.clone());

                // Full-window drag capture overlay — only visible while a panel
                // resize is in progress (panel_drag_active == true).  By covering
                // the entire window it intercepts PointerMove/PointerUp even when
                // the cursor has moved past the divider into the editor area.
                let drag_overlay = {
                    let style_s = state.clone();
                    let move_s  = state.clone();
                    let up_s    = state.clone();
                    container(empty())
                        .style(move |s| {
                            let active = style_s.panel_drag_active.get();
                            s.absolute()
                             .inset(0)
                             .z_index(50)
                             .cursor(floem::style::CursorStyle::ColResize)
                             .apply_if(!active, |s| s.display(floem::style::Display::None))
                        })
                        .on_event_stop(EventListener::PointerMove, move |e| {
                            if let Event::PointerMove(pe) = e {
                                let delta = pe.pos.x - move_s.panel_drag_start_x.get();
                                let new_w = (move_s.panel_drag_start_width.get() + delta)
                                    .max(80.0)
                                    .min(700.0);
                                move_s.left_panel_width.set(new_w);
                                move_s.show_left_panel.set(true);
                            }
                        })
                        .on_event_stop(EventListener::PointerUp, move |_| {
                            up_s.panel_drag_active.set(false);
                        })
                };

                // Root: cosmic canvas + IDE + overlays (overlays use z_index)
                stack((
                    cosmic_bg_canvas(state.theme),
                    ide_root(state.clone()),
                    palette,           // z_index(100)
                    picker,            // z_index(200) — on top of palette
                    completions_popup, // z_index(300) — above palette/picker
                    drag_overlay,      // z_index(50)  — only shown during resize
                ))
                .style(move |s| {
                    let t = state.theme.get();
                    let p = &t.palette;
                    s.width_full().height_full().background(p.bg_base)
                })
                .on_event_stop(EventListener::KeyDown, {
                    let state = state.clone();
                    move |event| {
                        if let Event::KeyDown(key_event) = event {
                            let ctrl  = key_event.modifiers.contains(Modifiers::CONTROL);
                            let shift = key_event.modifiers.contains(Modifiers::SHIFT);
                            let alt   = key_event.modifiers.contains(Modifiers::ALT);

                            // ── Named keys ───────────────────────────────────────
                            if let Key::Named(ref named) = key_event.key.logical_key {
                                if *named == floem::keyboard::NamedKey::Escape {
                                    if state.completion_open.get() {
                                        state.completion_open.set(false);
                                        return;
                                    }
                                    if state.file_picker_open.get() {
                                        state.file_picker_open.set(false);
                                        state.file_picker_query.set(String::new());
                                        return;
                                    }
                                    if state.command_palette_open.get() {
                                        state.command_palette_open.set(false);
                                        state.command_palette_query.set(String::new());
                                        return;
                                    }
                                }
                            }

                            // Ctrl+Space → request LSP completions and open popup
                            if ctrl && key_event.key.logical_key == Key::Named(floem::keyboard::NamedKey::Space) {
                                if let Some((path, line, col)) = state.active_cursor.get() {
                                    let _ = state.lsp_cmd.send(LspCommand::RequestCompletions {
                                        path, line, col,
                                    });
                                }
                                state.completion_selected.set(0);
                                state.completion_open.set(true);
                                return;
                            }

                            // Ctrl+P → file picker; Ctrl+Shift+P → command palette
                            if ctrl && key_event.key.logical_key == Key::Character("p".into()) {
                                if shift {
                                    // Ctrl+Shift+P: command palette
                                    let open = state.command_palette_open.get();
                                    state.command_palette_open.set(!open);
                                } else {
                                    // Ctrl+P: file picker
                                    let open = state.file_picker_open.get();
                                    state.file_picker_open.set(!open);
                                    if !open {
                                        state.file_picker_query.set(String::new());
                                    }
                                }
                                return;
                            }

                            // ── Character key combos ─────────────────────────────
                            if let Key::Character(ref ch) = key_event.key.logical_key {
                                let ch = ch.clone();

                                if ctrl && !shift && !alt {
                                    match ch.as_str() {
                                        // Ctrl+B — toggle left sidebar
                                        // Also updates left_panel_width so the
                                        // width-based transition in left_panel() picks
                                        // up the correct value immediately.
                                        "b" => {
                                            state.show_left_panel.update(|v| *v = !*v);
                                            let now_open = state.show_left_panel.get();
                                            state.left_panel_width.set(if now_open { 260.0 } else { 0.0 });
                                        }
                                        // Ctrl+J — toggle bottom terminal panel
                                        "j" => {
                                            state.show_bottom_panel.update(|v| *v = !*v);
                                        }
                                        // Ctrl+\ — toggle right chat panel
                                        "\\" => {
                                            state.show_right_panel.update(|v| *v = !*v);
                                        }
                                        _ => {}
                                    }
                                }

                                // Ctrl+Alt+M — toggle line comment
                                // TODO: Proper cursor tracking requires LSP / editor
                                // integration — needs a `cursor_line: RwSignal<usize>`
                                // wired from the text_editor widget.  For now we log
                                // the intent so the keybinding is discoverable.
                                if ctrl && alt && !shift && ch.as_str() == "m" {
                                    eprintln!(
                                        "[PhazeAI] Ctrl+Alt+M: comment toggle requested \
                                         (file={:?}) — cursor tracking not yet wired; \
                                         implement via editor cursor_line signal + LSP.",
                                        state.open_file.get()
                                    );
                                }
                            }
                        }
                    }
                })
            },
            Some(
                WindowConfig::default()
                    .title("PhazeAI IDE")
                    .size(Size::new(1400.0, 900.0)),
            ),
        )
        .run();
}
