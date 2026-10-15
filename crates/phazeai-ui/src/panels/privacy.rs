//! Settings › Privacy, plus the small hooks other panels use to honour the
//! project's AI privacy policy (`phazeai_core::privacy`).
//!
//! - [`install`] keeps the core's active project in sync with the workspace
//!   root and refreshes the views when the policy file or audit log changes.
//! - [`allow_editor_content`] gates editor AI actions on the active file.
//! - [`attach_for_next_send`] / [`take_attachments`] tell the chat which
//!   file an injected editor prompt came from, for the audit log.
//! - [`privacy_section`] is the settings UI; [`status_indicator`] the status-bar chip.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use floem::{
    reactive::{create_effect, create_rw_signal, RwSignal, Scope, SignalGet, SignalUpdate},
    views::{dyn_container, dyn_stack, label, stack, Decorators},
    IntoView,
};
use phazeai_core::privacy::{self, audit, PrivacyPolicy, BUILTIN_DENY, POLICY_FILE};

use crate::components::button::{phaze_button, ButtonVariant};
use crate::domain_state::IdeState;
use crate::theme::PhazeTheme;
use crate::tokens::{font, radius, space, stroke};

/// How many audit entries the settings view lists.
const AUDIT_ROWS: usize = 50;

thread_local! {
    static REVISION: Cell<Option<RwSignal<u64>>> = const { Cell::new(None) };
    static PENDING: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Bumped whenever the policy or the audit log may have changed. UI-thread only.
fn revision() -> RwSignal<u64> {
    REVISION.with(|cell| {
        if let Some(sig) = cell.get() {
            return sig;
        }
        // A root scope of its own: lives for the whole app, never disposed.
        let sig = Scope::new().create_rw_signal(0u64);
        cell.set(Some(sig));
        sig
    })
}

fn bump() {
    revision().update(|r| *r += 1);
}

/// Wire the active privacy project to the workspace root and start watching
/// the policy file / audit log for changes. Call once from the root view.
pub fn install(state: &IdeState) {
    let root = state.project.workspace_root;
    create_effect(move |_| {
        privacy::set_active_project(root.get());
        bump();
    });

    // Poll every 2 s off the UI thread; only signal when something changed.
    let (tx, rx) = std::sync::mpsc::sync_channel::<()>(1);
    let changed = floem::ext_event::create_signal_from_channel(rx);
    create_effect(move |_| {
        if changed.get().is_some() {
            bump();
        }
    });
    std::thread::spawn(move || {
        let mut last = (0u64, 0usize);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let now = (audit::latest_id(), Arc::as_ptr(&privacy::active()) as usize);
            if now != last {
                last = now;
                if tx.try_send(()).is_err() && tx.send(()).is_err() {
                    return; // UI gone
                }
            }
        }
    });
}

/// Whether the AI may see content from `path` (the active editor file).
/// On refusal, records the attempt and shows a toast explaining why.
pub fn allow_editor_content(
    status_toast: RwSignal<Option<String>>,
    path: &Path,
    via: &str,
) -> bool {
    match privacy::check_and_record(path, via) {
        Ok(()) => true,
        Err(reason) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            crate::app::show_toast(status_toast, format!("Not sent to AI: {name} is {reason}"));
            bump();
            false
        }
    }
}

/// Note that the next chat send carries content from `path` (for the audit log).
pub fn attach_for_next_send(path: &Path) {
    PENDING.with(|p| p.borrow_mut().push(path.to_path_buf()));
}

/// Files noted with [`attach_for_next_send`] since the last send.
pub fn take_attachments() -> Vec<PathBuf> {
    PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// Status-bar chip shown while the project is local-only.
pub fn status_indicator(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let rev = revision();
    label(move || {
        rev.get();
        let p = privacy::active();
        if p.load_error().is_some() {
            "🔒 Local only (privacy.toml error)  ".to_string()
        } else if p.local_only() {
            "🔒 Local only  ".to_string()
        } else {
            String::new()
        }
    })
    .style(move |s| {
        rev.get();
        let p = theme.get().palette;
        let policy = privacy::active();
        let color = if policy.load_error().is_some() {
            p.warning
        } else {
            p.success
        };
        s.color(color)
            .font_size(font::CAPTION)
            .margin_left(space::LG)
            .apply_if(!policy.local_only(), |s| {
                s.display(floem::style::Display::None)
            })
    })
}

// ── Settings section ──────────────────────────────────────────────────────────

fn heading(text: &'static str, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    label(move || text).style(move |s| {
        s.font_size(10.0)
            .color(theme.get().palette.accent)
            .font_weight(floem::text::Weight::BOLD)
            .margin_bottom(space::LG)
    })
}

fn subheading(text: &'static str, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    label(move || text).style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.text_secondary)
            .font_weight(floem::text::Weight::BOLD)
            .margin_top(space::LG)
            .margin_bottom(space::XS)
    })
}

fn muted(text: impl Fn() -> String + 'static, theme: RwSignal<PhazeTheme>) -> floem::views::Label {
    label(text).style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.text_muted)
    })
}

fn chip(text: String, project: bool, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    label(move || text.clone()).style(move |s| {
        let p = theme.get().palette;
        let accent = if project { p.accent } else { p.text_secondary };
        s.font_size(font::CAPTION)
            .font_family("JetBrains Mono, Fira Code, monospace".to_string())
            .color(accent)
            .padding_horiz(space::SM)
            .padding_vert(space::XXS)
            .margin_right(space::XS)
            .margin_bottom(space::XS)
            .border(stroke::HAIRLINE)
            .border_color(accent.with_alpha(0.4))
            .border_radius(radius::SM)
    })
}

fn chips(
    items: impl Fn() -> Vec<String> + 'static,
    project: bool,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    dyn_stack(
        move || items().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, s)| (*i, s.clone()),
        move |(_, s)| chip(s, project, theme),
    )
    .style(|s| {
        s.flex_row()
            .flex_wrap(floem::style::FlexWrap::Wrap)
            .width_full()
    })
}

fn policy_now(rev: RwSignal<u64>) -> Arc<PrivacyPolicy> {
    rev.get();
    privacy::active()
}

fn file_status_text(p: &PrivacyPolicy) -> String {
    match (p.root(), p.file_exists()) {
        (None, _) => "No project open — only the built-in secrets list applies.".to_string(),
        (Some(root), true) => format!("Policy: {}", root.join(POLICY_FILE).display()),
        (Some(_), false) => format!(
            "No {POLICY_FILE} in this project — the built-in secrets list applies and cloud AI is allowed."
        ),
    }
}

fn audit_row(entry: audit::AuditEntry, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    let blocked = entry.is_blocked();
    let time = entry.time.format("%H:%M:%S").to_string();
    let (title, detail) = match &entry.kind {
        audit::AuditKind::Request {
            provider,
            model,
            chars,
            files,
        } => (
            format!("Request to {provider} / {model} · ~{chars} chars"),
            if files.is_empty() {
                "No project file contents attached".to_string()
            } else {
                format!("Files: {}", files.join(", "))
            },
        ),
        audit::AuditKind::BlockedFile { path, reason, via } => {
            (format!("Blocked {path} ({via})"), reason.clone())
        }
        audit::AuditKind::BlockedProvider {
            provider, model, ..
        } => (
            format!("Blocked request to {provider} / {model}"),
            "Project is local-only".to_string(),
        ),
    };
    stack((
        label(move || time.clone()).style(move |s| {
            s.font_size(font::CAPTION)
                .color(theme.get().palette.text_muted)
                .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                .width(64.0)
                .flex_shrink(0.0)
        }),
        stack((
            label(move || title.clone()).style(move |s| {
                let p = theme.get().palette;
                s.font_size(font::BODY)
                    .color(if blocked { p.error } else { p.text_primary })
            }),
            label(move || detail.clone()).style(move |s| {
                s.font_size(font::CAPTION)
                    .color(theme.get().palette.text_muted)
                    .min_width(0.0)
            }),
        ))
        .style(|s| s.flex_col().gap(space::XXS).flex_grow(1.0).min_width(0.0)),
    ))
    .style(move |s| {
        let p = theme.get().palette;
        s.flex_row()
            .width_full()
            .padding_vert(space::XS)
            .padding_horiz(space::SM)
            .border_radius(radius::SM)
            .border_bottom(stroke::HAIRLINE)
            .border_color(p.border.with_alpha(0.2))
            .apply_if(blocked, |s| s.background(p.error.with_alpha(0.08)))
    })
}

/// The Privacy section of the settings panel.
pub fn privacy_section(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let rev = revision();
    // Result of the last button action (error or confirmation).
    let action_msg: RwSignal<Option<(bool, String)>> = create_rw_signal(None);

    let intro = muted(
        || {
            "Control what project content may be sent to AI providers. Denied files are \
             refused by the agent's tools and left out of @mentions, editor AI actions, \
             inline edit and completions. Shell commands are only checked by the words \
             they contain, and MCP servers aren't restricted — review those approvals."
                .to_string()
        },
        theme,
    )
    .style(|s| s.margin_bottom(space::MD));

    let file_status = muted(move || file_status_text(&policy_now(rev)), theme);

    let load_error = label(move || {
        policy_now(rev)
            .load_error()
            .map(|e| {
                format!("⚠ {e}\nCloud AI is blocked for this project until the file is fixed.")
            })
            .unwrap_or_default()
    })
    .style(move |s| {
        let p = theme.get().palette;
        let has_err = policy_now(rev).load_error().is_some();
        s.font_size(font::BODY)
            .color(p.error)
            .padding(space::MD)
            .margin_top(space::SM)
            .border(stroke::HAIRLINE)
            .border_color(p.error.with_alpha(0.5))
            .border_radius(radius::MD)
            .background(p.error.with_alpha(0.08))
            .apply_if(!has_err, |s| s.display(floem::style::Display::None))
    });

    // ── local_only toggle ────────────────────────────────────────────────
    let toggle_state = state.clone();
    let local_toggle = dyn_container(
        move || {
            let p = policy_now(rev);
            (p.local_only(), p.load_error().is_some(), p.root().is_some())
        },
        move |(on, broken, has_root)| {
            let st = toggle_state.clone();
            let text = if on { "Turn off" } else { "Turn on" };
            let variant = if on {
                ButtonVariant::Secondary
            } else {
                ButtonVariant::Primary
            };
            let button = phaze_button(text, variant, theme, move || {
                let root = st.project.workspace_root.get_untracked();
                // The On/Off status shows success; only failures need a message.
                action_msg.set(
                    privacy::write_local_only(&root, !on)
                        .err()
                        .map(|e| (false, e)),
                );
                bump();
            })
            .style(move |s| {
                s.apply_if(broken || !has_root, |s| {
                    s.display(floem::style::Display::None)
                })
            });
            let status = label(move || {
                if broken {
                    "On — forced by the file error".to_string()
                } else if on {
                    "On".to_string()
                } else {
                    "Off".to_string()
                }
            })
            .style(move |s| {
                let p = theme.get().palette;
                let color = if broken {
                    p.warning
                } else if on {
                    p.success
                } else {
                    p.text_secondary
                };
                s.font_size(font::UI)
                    .font_weight(floem::text::Weight::BOLD)
                    .color(color)
                    .flex_grow(1.0)
            });
            stack((
                label(|| "Local models only").style(move |s| {
                    s.font_size(font::UI)
                        .color(theme.get().palette.text_primary)
                }),
                label(|| {
                    "Only Ollama, LM Studio or other localhost endpoints may be used for this project."
                })
                .style(move |s| {
                    s.font_size(font::CAPTION)
                        .color(theme.get().palette.text_muted)
                }),
                stack((status, button))
                    .style(|s| s.flex_row().items_center().width_full().margin_top(space::SM)),
            ))
            .style(|s| s.flex_col().gap(space::XXS).width_full())
            .into_any()
        },
    )
    .style(|s| s.width_full().margin_top(space::LG));

    // ── actions ──────────────────────────────────────────────────────────
    let edit_state = state.clone();
    let edit_btn = phaze_button(
        "Edit privacy.toml",
        ButtonVariant::Secondary,
        theme,
        move || {
            let root = edit_state.project.workspace_root.get_untracked();
            match privacy::ensure_policy_file(&root) {
                Ok(path) => {
                    edit_state.editor.open_file.set(Some(path));
                    action_msg.set(None);
                }
                Err(e) => action_msg.set(Some((false, e))),
            }
            bump();
        },
    );
    let actions = stack((edit_btn,)).style(|s| s.flex_row().gap(space::MD).margin_top(space::LG));

    let action_feedback = label(move || action_msg.get().map(|(_, m)| m).unwrap_or_default())
        .style(move |s| {
            let p = theme.get().palette;
            let msg = action_msg.get();
            let ok = msg.as_ref().is_some_and(|(ok, _)| *ok);
            s.font_size(font::CAPTION)
                .margin_top(space::SM)
                .color(if ok { p.success } else { p.error })
                .apply_if(msg.is_none(), |s| s.display(floem::style::Display::None))
        });

    // ── deny lists ───────────────────────────────────────────────────────
    let builtin = chips(
        || BUILTIN_DENY.iter().map(|s| s.to_string()).collect(),
        false,
        theme,
    );
    let project_list = chips(
        move || policy_now(rev).project_patterns().to_vec(),
        true,
        theme,
    );
    let project_empty = muted(
        move || {
            if policy_now(rev).project_patterns().is_empty() {
                "None — add patterns to `deny` in privacy.toml.".to_string()
            } else {
                String::new()
            }
        },
        theme,
    )
    .style(move |s| {
        let has = !policy_now(rev).project_patterns().is_empty();
        s.apply_if(has, |s| s.display(floem::style::Display::None))
    });

    // ── audit log ────────────────────────────────────────────────────────
    let audit_list = dyn_stack(
        move || {
            rev.get();
            audit::recent(AUDIT_ROWS)
        },
        |e| e.id,
        move |e| audit_row(e, theme),
    )
    .style(|s| s.flex_col().width_full());
    let audit_empty = muted(
        move || {
            rev.get();
            if audit::latest_id() == 0 {
                "No requests yet this session.".to_string()
            } else {
                String::new()
            }
        },
        theme,
    )
    .style(move |s| {
        rev.get();
        s.apply_if(audit::latest_id() != 0, |s| {
            s.display(floem::style::Display::None)
        })
    });

    stack((
        heading("PRIVACY", theme),
        intro,
        file_status,
        load_error,
        local_toggle,
        actions,
        action_feedback,
        subheading("NEVER SENT — BUILT-IN (ALWAYS ON)", theme),
        builtin,
        subheading("NEVER SENT — THIS PROJECT", theme),
        project_list,
        project_empty,
        subheading("RECENT AI REQUESTS (NEWEST FIRST)", theme),
        muted(
            || {
                "This session only. Logged as each request leaves (even if the provider then \
                 fails). ~chars counts message text, tool calls and tool schemas; files are \
                 those whose contents were attached since the previous request."
                    .to_string()
            },
            theme,
        ),
        audit_empty,
        audit_list,
    ))
    .style(|s| s.flex_col().width_full())
}
