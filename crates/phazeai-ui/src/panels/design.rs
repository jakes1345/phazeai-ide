//! Design panel: describe a screen, get a working HTML/CSS prototype that
//! follows the project's design system.
//!
//! Files live in the project (see [`phazeai_core::design`]):
//! `.phazeai/design-system.md` and `.phazeai/designs/*.html`. Generation never
//! overwrites anything: a revision is saved next to the design it came from,
//! and redrafting the design system keeps the old one as a backup.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use floem::{
    ext_event::create_signal_from_channel,
    peniko::Color,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate, SignalWith},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::design::{
    self, collect_style_sources, design_prompt, extract_html, extract_markdown, extract_prompt,
    list_designs, load_design_system, new_design_path, save_design_system, DesignEntry,
    DESIGN_SYSTEM_FILE, DESIGN_SYSTEM_PROMPT, EXTRACT_SYSTEM_PROMPT,
};
use phazeai_core::Settings;

use crate::components::button::{phaze_button, ButtonVariant};
use crate::custom_theme::color_to_hex;
use crate::domain_state::IdeState;
use crate::theme::PhazeTheme;
use crate::tokens::{font, radius, space, stroke};
use crate::util::open_external_url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    NewDesign,
    Revision,
    DesignSystem,
}

impl Job {
    fn working_label(self) -> &'static str {
        match self {
            Job::NewDesign => "Designing",
            Job::Revision => "Revising",
            Job::DesignSystem => "Drafting the design system",
        }
    }
}

#[derive(Clone, Debug)]
enum DesignUpdate {
    /// Bytes of reply received so far.
    Progress(usize),
    Saved {
        job: Job,
        path: PathBuf,
    },
    Failed(String),
}

/// Send progress at most once per this many bytes of reply.
const PROGRESS_STEP: usize = 512;

/// Run one model request on a background thread and save its result.
fn spawn_job(
    job: Job,
    root: PathBuf,
    request: String,
    revise: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
    tx: std::sync::mpsc::SyncSender<DesignUpdate>,
) {
    std::thread::spawn(move || {
        let result = run_job(job, &root, &request, revise.as_deref(), &cancel, &tx);
        let update = match result {
            Ok(path) => DesignUpdate::Saved { job, path },
            Err(e) => DesignUpdate::Failed(e),
        };
        let _ = tx.send(update);
    });
}

fn run_job(
    job: Job,
    root: &Path,
    request: &str,
    revise: Option<&Path>,
    cancel: &AtomicBool,
    tx: &std::sync::mpsc::SyncSender<DesignUpdate>,
) -> Result<PathBuf, String> {
    let client = Settings::load()
        .build_llm_client()
        .map_err(|e| format!("AI provider isn't ready: {e}. Open Settings to set one up."))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    // Files whose contents go into the prompt, for the privacy audit log.
    // (Style sources and the design system already skip denied files.)
    let mut attached: Vec<PathBuf> = Vec::new();
    let (system, prompt) = match job {
        Job::DesignSystem => {
            let sources = collect_style_sources(root);
            attached.extend(sources.iter().map(|(rel, _)| root.join(rel)));
            (EXTRACT_SYSTEM_PROMPT, extract_prompt(&sources))
        }
        Job::NewDesign | Job::Revision => {
            let current = match revise {
                Some(path) => {
                    phazeai_core::privacy::check_and_record(path, "design revision").map_err(
                        |reason| format!("Not sent to AI: {} is {reason}", path.display()),
                    )?;
                    attached.push(path.to_path_buf());
                    Some(
                        std::fs::read_to_string(path)
                            .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?,
                    )
                }
                None => None,
            };
            let ds = load_design_system(root);
            if ds.is_some() {
                attached.push(design::design_system_path(root));
            }
            (
                DESIGN_SYSTEM_PROMPT,
                design_prompt(request, ds.as_deref(), current.as_deref()),
            )
        }
    };

    let mut last_sent = 0;
    let reply = rt.block_on(phazeai_core::privacy::audit::scope(
        attached,
        design::complete(client.as_ref(), system, &prompt, cancel, |text| {
            if text.len() >= last_sent + PROGRESS_STEP {
                last_sent = text.len();
                let _ = tx.try_send(DesignUpdate::Progress(text.len()));
            }
        }),
    ))?;

    match job {
        Job::DesignSystem => {
            let markdown = extract_markdown(&reply);
            let path = design::design_system_path(root);
            if path.exists() {
                let backup = path.with_extension("previous.md");
                std::fs::copy(&path, &backup)
                    .map_err(|e| format!("Couldn't back up the old design system: {e}"))?;
            }
            save_design_system(root, &markdown).map_err(|e| e.to_string())
        }
        Job::NewDesign | Job::Revision => {
            let html = extract_html(&reply).ok_or_else(|| {
                "The model's reply didn't contain an HTML page. Try rephrasing the request."
                    .to_string()
            })?;
            let name_from = match revise {
                Some(p) => p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| request.to_string()),
                None => request.to_string(),
            };
            let path = new_design_path(root, &name_from);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, html).map_err(|e| e.to_string())?;
            Ok(path)
        }
    }
}

/// A design system document describing the IDE's current theme, so designs
/// can match the editor's look without any project styles.
pub fn design_system_from_theme(theme: &PhazeTheme) -> String {
    let p = &theme.palette;
    let hex = |c| color_to_hex(c);
    // Cosmic themes paint translucent "glass" over an animated backdrop. A web
    // page has no backdrop, so flatten the background colours onto bg_deep.
    let backdrop = flatten(
        p.bg_deep,
        if theme.is_dark() {
            Color::BLACK
        } else {
            Color::WHITE
        },
    );
    let bg = |c| hex(flatten(c, backdrop));
    let mode = if theme.is_dark() { "dark" } else { "light" };
    format!(
        "# Design system — {name}\n\n\
         ## Overview\n\
         A {mode}, focused interface: calm surfaces, one strong accent for primary actions \
         and state, generous contrast for text.\n\n\
         ## Colour\n\
         | Token | Value | Use |\n|---|---|---|\n\
         | `--bg-deep` | {bg_deep} | Page background |\n\
         | `--bg-base` | {bg_base} | App background |\n\
         | `--bg-surface` | {bg_surface} | Cards, inputs |\n\
         | `--bg-elevated` | {bg_elevated} | Menus, popovers, hover |\n\
         | `--text` | {text} | Primary text |\n\
         | `--text-secondary` | {text2} | Secondary text |\n\
         | `--text-muted` | {muted} | Hints, metadata |\n\
         | `--accent` | {accent} | Primary actions, links, focus |\n\
         | `--accent-hover` | {accent_hover} | Hover on accent elements |\n\
         | `--accent-dim` | {accent_dim} | Selected backgrounds |\n\
         | `--border` | {border} | Dividers, control outlines |\n\
         | `--success` | {success} | Success states |\n\
         | `--warning` | {warning} | Warnings |\n\
         | `--error` | {error} | Errors, destructive actions |\n\
         | `--info` | {info} | Informational states |\n\n\
         Text on `--accent` uses {on_accent}.\n\n\
         ## Typography\n\
         - UI: system sans-serif (`system-ui, -apple-system, Segoe UI, sans-serif`).\n\
         - Code: monospace (`ui-monospace, SFMono-Regular, Consolas, monospace`).\n\
         - Sizes: caption {cap}px, body {body}px, UI {ui}px, title {title}px, heading {head}px; \
         larger page headings may scale up from heading.\n\
         - Weights: 400 regular, 600 for titles and buttons.\n\n\
         ## Spacing & layout\n\
         - Scale: {s_xxs}, {s_xs}, {s_sm}, {s_md}, {s_lg}, {s_xl}, {s_xxl}px.\n\
         - Content max width 1200px; single column below 720px.\n\n\
         ## Shape & elevation\n\
         - Radii: {r_sm}px chips, {r_md}px buttons and inputs, {r_lg}px cards and dialogs.\n\
         - Borders: {hair}px `--border`; focus ring {focus}px `--accent`.\n\
         - Prefer borders and surface colour over heavy shadows.\n\n\
         ## Components\n\
         - **Primary button**: `--accent` fill, {on_accent} text, `--accent-hover` on hover.\n\
         - **Secondary button**: `--bg-surface` fill, `--border` outline, `--text`.\n\
         - **Ghost button**: no fill, `--accent` text, `--accent-dim` on hover.\n\
         - **Danger button**: `--error` text on a faint `--error` tint.\n\
         - **Input**: `--bg-surface`, `--border` outline, `--accent` focus ring.\n\
         - **Card**: `--bg-surface`, `--border`, {r_lg}px radius, {s_xl}px padding.\n\n\
         ## Voice\n\
         Short, direct, friendly. Say what happened and what to do next.\n",
        name = theme.display_name(),
        bg_deep = bg(p.bg_deep),
        bg_base = bg(p.bg_base),
        bg_surface = bg(p.bg_surface),
        bg_elevated = bg(p.bg_elevated),
        text = hex(p.text_primary),
        text2 = hex(p.text_secondary),
        muted = hex(p.text_muted),
        accent = hex(p.accent),
        accent_hover = hex(p.accent_hover),
        accent_dim = hex(p.accent_dim),
        border = hex(p.border),
        success = hex(p.success),
        warning = hex(p.warning),
        error = hex(p.error),
        info = hex(p.info),
        on_accent = hex(p.button_primary_fg),
        cap = font::CAPTION,
        body = font::BODY,
        ui = font::UI,
        title = font::TITLE,
        head = font::HEADING,
        s_xxs = space::XXS,
        s_xs = space::XS,
        s_sm = space::SM,
        s_md = space::MD,
        s_lg = space::LG,
        s_xl = space::XL,
        s_xxl = space::XXL,
        r_sm = radius::SM,
        r_md = radius::MD,
        r_lg = radius::LG,
        hair = stroke::HAIRLINE,
        focus = stroke::FOCUS,
    )
}

/// `color` composited over the opaque `under`.
fn flatten(color: Color, under: Color) -> Color {
    let c = color.to_rgba8();
    let u = under.to_rgba8();
    let a = c.a as f32 / 255.0;
    let mix = |top: u8, bottom: u8| (top as f32 * a + bottom as f32 * (1.0 - a)).round() as u8;
    Color::from_rgb8(mix(c.r, u.r), mix(c.g, u.g), mix(c.b, u.b))
}

fn caption(text: impl Fn() -> String + 'static, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    label(text).style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.text_muted)
    })
}

fn heading(text: &'static str, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    label(move || text).style(move |s| {
        s.font_size(font::CAPTION)
            .font_weight(floem::text::Weight::BOLD)
            .color(theme.get().palette.accent)
            .margin_top(space::LG)
    })
}

fn card(child: impl IntoView + 'static, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    container(child).style(move |s| {
        let p = theme.get().palette;
        s.width_full()
            .padding(space::LG)
            .gap(space::SM)
            .background(p.bg_surface)
            .border(stroke::HAIRLINE)
            .border_color(p.border)
            .border_radius(radius::LG)
    })
}

fn design_row(
    entry: DesignEntry,
    selected: RwSignal<Option<PathBuf>>,
    state: IdeState,
    status: RwSignal<String>,
) -> impl IntoView {
    let theme = state.workbench.theme;
    let path = entry.path.clone();
    let is_selected = {
        let path = path.clone();
        move || selected.with(|s| s.as_deref() == Some(path.as_path()))
    };
    let name = entry.name.clone();
    let title = label(move || name.clone()).style({
        let is_selected = is_selected.clone();
        move |s| {
            let p = theme.get().palette;
            s.flex_grow(1.0)
                .min_width(0.0)
                .font_size(font::BODY)
                .color(if is_selected() {
                    p.accent
                } else {
                    p.text_primary
                })
                .text_ellipsis()
        }
    });
    let open_path = path.clone();
    let code_path = path.clone();
    let select_path = path.clone();
    stack((
        title,
        phaze_button("Open", ButtonVariant::Ghost, theme, move || {
            if let Err(e) = open_external_url(&open_path.to_string_lossy()) {
                status.set(format!("Couldn't open the browser: {e}"));
            }
        }),
        phaze_button("Code", ButtonVariant::Ghost, theme, {
            let open_file = state.editor.open_file;
            move || open_file.set(Some(code_path.clone()))
        }),
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .items_center()
            .width_full()
            .gap(space::XS)
            .padding_horiz(space::MD)
            .padding_vert(space::XXS)
            .border_radius(radius::MD)
            .apply_if(is_selected(), |s| s.background(p.accent_dim))
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| {
        selected.update(|s| {
            *s = if s.as_deref() == Some(select_path.as_path()) {
                None
            } else {
                Some(select_path.clone())
            }
        });
    })
}

pub fn design_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let root_sig = state.project.workspace_root;

    let request = create_rw_signal(String::new());
    let designs = create_rw_signal(Vec::<DesignEntry>::new());
    let selected = create_rw_signal(None::<PathBuf>);
    let has_ds = create_rw_signal(false);
    let busy = create_rw_signal(None::<Job>);
    let status = create_rw_signal(String::new());
    let cancel = create_rw_signal(None::<Arc<AtomicBool>>);

    let refresh = move || {
        let root = root_sig.get_untracked();
        designs.set(list_designs(&root));
        has_ds.set(load_design_system(&root).is_some());
    };
    create_effect(move |_| {
        let _ = root_sig.get();
        refresh();
    });

    let (tx, rx) = std::sync::mpsc::sync_channel::<DesignUpdate>(64);
    let updates = create_signal_from_channel(rx);
    create_effect(move |_| {
        let Some(update) = updates.get() else {
            return;
        };
        match update {
            DesignUpdate::Progress(bytes) => {
                if let Some(job) = busy.get_untracked() {
                    status.set(format!(
                        "{}… {:.1} KB received",
                        job.working_label(),
                        bytes as f64 / 1024.0
                    ));
                }
            }
            DesignUpdate::Saved { job, path } => {
                busy.set(None);
                cancel.set(None);
                refresh();
                let shown = path
                    .strip_prefix(root_sig.get_untracked())
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                match job {
                    Job::DesignSystem => {
                        status.set(format!(
                            "Saved {shown}. Review and edit it — every design follows it."
                        ));
                        state.editor.open_file.set(Some(path));
                    }
                    Job::NewDesign | Job::Revision => {
                        status.set(format!("Saved {shown}. Opening it in your browser."));
                        if let Err(e) = open_external_url(&path.to_string_lossy()) {
                            status
                                .set(format!("Saved {shown}, but couldn't open the browser: {e}"));
                        }
                        selected.set(Some(path));
                    }
                }
            }
            DesignUpdate::Failed(e) => {
                busy.set(None);
                cancel.set(None);
                status.set(if e == design::CANCELLED {
                    "Stopped.".to_string()
                } else {
                    e
                });
            }
        }
    });

    let start = {
        let tx = tx.clone();
        move |job: Job| {
            if busy.get_untracked().is_some() {
                return;
            }
            let text = request.get_untracked().trim().to_string();
            let revise = match job {
                Job::Revision => match selected.get_untracked() {
                    Some(p) => Some(p),
                    None => {
                        status.set("Select a design below to revise it.".to_string());
                        return;
                    }
                },
                _ => None,
            };
            if job != Job::DesignSystem && text.is_empty() {
                status.set("Describe what you want first.".to_string());
                return;
            }
            let flag = Arc::new(AtomicBool::new(false));
            cancel.set(Some(flag.clone()));
            busy.set(Some(job));
            status.set(format!("{}…", job.working_label()));
            spawn_job(
                job,
                root_sig.get_untracked(),
                text,
                revise,
                flag,
                tx.clone(),
            );
        }
    };

    // ── Design system card ──────────────────────────────────────────────────
    let ds_status = caption(
        move || {
            if has_ds.get() {
                format!("Designs follow {DESIGN_SYSTEM_FILE}.")
            } else {
                "No design system yet, so each design picks its own style. Draft one from \
                 this project's styles, or start from the IDE theme."
                    .to_string()
            }
        },
        theme,
    );
    let ds_buttons = stack((
        phaze_button("Draft from project", ButtonVariant::Secondary, theme, {
            let start = start.clone();
            move || start(Job::DesignSystem)
        }),
        phaze_button("From IDE theme", ButtonVariant::Ghost, theme, move || {
            let root = root_sig.get_untracked();
            if has_ds.get_untracked() {
                let path = design::design_system_path(&root);
                let _ = std::fs::copy(&path, path.with_extension("previous.md"));
            }
            let md = design_system_from_theme(&theme.get_untracked());
            match save_design_system(&root, &md) {
                Ok(path) => {
                    refresh();
                    status.set(format!(
                        "Wrote {DESIGN_SYSTEM_FILE} from the current theme."
                    ));
                    state.editor.open_file.set(Some(path));
                }
                Err(e) => status.set(format!("Couldn't write the design system: {e}")),
            }
        }),
        container(phaze_button(
            "Edit",
            ButtonVariant::Ghost,
            theme,
            move || {
                state
                    .editor
                    .open_file
                    .set(Some(design::design_system_path(&root_sig.get_untracked())));
            },
        ))
        .style(move |s| s.apply_if(!has_ds.get(), |s| s.display(floem::style::Display::None))),
    ))
    .style(|s| {
        s.flex_row()
            .flex_wrap(floem::style::FlexWrap::Wrap)
            .gap(space::SM)
    });
    let ds_card = card(
        stack((ds_status, ds_buttons)).style(|s| s.flex_col().gap(space::SM).width_full()),
        theme,
    );

    // ── Request ─────────────────────────────────────────────────────────────
    let input = text_input(request)
        .placeholder("e.g. A pricing page with three plans and an FAQ")
        .style(move |s| {
            let p = theme.get().palette;
            s.width_full()
                .min_width(0.0)
                .padding(space::MD)
                .font_size(font::BODY)
                .background(p.bg_surface)
                .color(p.text_primary)
                .border(stroke::HAIRLINE)
                .border_color(p.border)
                .border_radius(radius::MD)
        });
    let actions = stack((
        container(phaze_button("Generate", ButtonVariant::Primary, theme, {
            let start = start.clone();
            move || start(Job::NewDesign)
        }))
        .style(move |s| {
            s.apply_if(busy.get().is_some(), |s| {
                s.display(floem::style::Display::None)
            })
        }),
        container(phaze_button(
            "Revise selected",
            ButtonVariant::Secondary,
            theme,
            {
                let start = start.clone();
                move || start(Job::Revision)
            },
        ))
        .style(move |s| {
            s.apply_if(
                busy.get().is_some() || selected.with(|s| s.is_none()),
                |s| s.display(floem::style::Display::None),
            )
        }),
        container(phaze_button(
            "Stop",
            ButtonVariant::Danger,
            theme,
            move || {
                if let Some(flag) = cancel.get_untracked() {
                    flag.store(true, Ordering::Relaxed);
                    status.set("Stopping…".to_string());
                }
            },
        ))
        .style(move |s| {
            s.apply_if(busy.get().is_none(), |s| {
                s.display(floem::style::Display::None)
            })
        }),
    ))
    .style(|s| s.flex_row().gap(space::SM));

    let status_line = label(move || status.get()).style(move |s| {
        let p = theme.get().palette;
        s.font_size(font::CAPTION)
            .color(p.text_secondary)
            .apply_if(status.with(|m| m.is_empty()), |s| {
                s.display(floem::style::Display::None)
            })
    });

    // ── Designs list ────────────────────────────────────────────────────────
    let list = dyn_stack(move || designs.get(), |entry| entry.path.clone(), {
        let state = state.clone();
        move |entry| design_row(entry, selected, state.clone(), status)
    })
    .style(|s| s.flex_col().width_full().gap(space::XXS));
    let empty = caption(
        || "Nothing yet. Generated designs are saved in .phazeai/designs/.".to_string(),
        theme,
    )
    .style(move |s| {
        s.apply_if(designs.with(|d| !d.is_empty()), |s| {
            s.display(floem::style::Display::None)
        })
    });

    let body = stack((
        label(|| "DESIGN".to_string()).style(move |s| {
            s.font_size(font::CAPTION)
                .font_weight(floem::text::Weight::BOLD)
                .color(theme.get().palette.accent)
        }),
        caption(
            || "Describe a screen and get a working HTML prototype that follows your design system."
                .to_string(),
            theme,
        ),
        heading("DESIGN SYSTEM", theme),
        ds_card,
        heading("NEW DESIGN", theme),
        input,
        actions,
        status_line,
        stack((
            heading("DESIGNS", theme),
            container(phaze_button("Refresh", ButtonVariant::Ghost, theme, refresh))
                .style(|s| s.margin_left(floem::unit::PxPctAuto::Auto).margin_top(space::SM)),
        ))
        .style(|s| s.flex_row().items_center().width_full()),
        empty,
        list,
    ))
    .style(|s| s.flex_col().width_full().gap(space::SM).padding(space::LG));

    scroll(body).style(move |s| {
        s.width_full()
            .height_full()
            .background(theme.get().palette.bg_panel)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeVariant;

    #[test]
    fn theme_design_system_uses_live_palette_values() {
        let theme = PhazeTheme::from_variant(ThemeVariant::Dracula);
        let md = design_system_from_theme(&theme);
        assert!(md.starts_with("# Design system — Dracula"));
        assert!(md.contains(&color_to_hex(theme.palette.accent)));
        assert!(md.contains("A dark,"));
        assert!(md.contains("A dark,"));
        // Glass themes: every background is a solid #rrggbb colour.
        let cosmic = design_system_from_theme(&PhazeTheme::midnight_blue());
        for token in ["--bg-deep", "--bg-base", "--bg-surface", "--bg-elevated"] {
            let line = cosmic.lines().find(|l| l.contains(token)).unwrap();
            let value = line.split('|').nth(2).unwrap().trim();
            assert_eq!(value.len(), 7, "{token} = {value}");
        }
        let light = design_system_from_theme(&PhazeTheme::light());
        assert!(light.contains("A light,"));
    }
}
