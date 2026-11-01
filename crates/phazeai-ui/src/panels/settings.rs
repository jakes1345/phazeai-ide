use floem::{
    reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::{
    llm::{
        key_setup::{check_api_key, key_help},
        provider::{keyring_delete, keyring_set, ApiKeySource, ProviderConfig, ProviderId},
    },
    Settings,
};

use crate::domain_state::IdeState;
use crate::{
    components::button::{phaze_button, ButtonVariant},
    components::icon::{icons, phaze_icon},
    custom_theme::{export_name_for, export_theme, load_custom_themes, themes_dir, LoadedThemes},
    theme::{PhazeTheme, ThemeVariant},
    tokens::{font, radius, space, stroke},
    util::open_external_url,
};

#[derive(Clone)]
struct ProviderUiStatus {
    available: bool,
    summary: String,
    detail: String,
}

fn provider_name_to_id(name: &str) -> Option<ProviderId> {
    match name {
        "Claude (Anthropic)" => Some(ProviderId::Claude),
        "OpenAI" => Some(ProviderId::OpenAI),
        "Google Gemini" => Some(ProviderId::Gemini),
        "Groq" => Some(ProviderId::Groq),
        "Together.ai" => Some(ProviderId::Together),
        "OpenRouter" => Some(ProviderId::OpenRouter),
        "Ollama (Local)" => Some(ProviderId::Ollama),
        "LM Studio (Local)" => Some(ProviderId::LmStudio),
        "Mistral AI" => Some(ProviderId::Mistral),
        "DeepSeek" => Some(ProviderId::DeepSeek),
        "xAI (Grok)" => Some(ProviderId::Xai),
        "Cohere" => Some(ProviderId::Cohere),
        "Perplexity" => Some(ProviderId::Perplexity),
        "Azure OpenAI" => Some(ProviderId::Azure),
        "Cerebras" => Some(ProviderId::Cerebras),
        "Fireworks AI" => Some(ProviderId::Fireworks),
        "SambaNova" => Some(ProviderId::SambaNova),
        "GitHub Models" => Some(ProviderId::GithubModels),
        "PhazeAI Local (built-in)" => Some(ProviderId::BuiltIn),
        _ => None,
    }
}

fn provider_status(name: &str) -> ProviderUiStatus {
    let Some(provider_id) = provider_name_to_id(name) else {
        return ProviderUiStatus {
            available: false,
            summary: "Unknown provider".into(),
            detail: "This provider name does not map to a configured backend.".into(),
        };
    };

    let settings = Settings::load();
    let registry = settings.build_provider_registry();
    let Some(config) = registry.get_config(&provider_id) else {
        return ProviderUiStatus {
            available: false,
            summary: "Not configured".into(),
            detail: "No provider configuration is available for this backend.".into(),
        };
    };

    if !config.enabled {
        return ProviderUiStatus {
            available: false,
            summary: "Disabled".into(),
            detail: "This provider is disabled in config.toml.".into(),
        };
    }

    if provider_id == ProviderId::BuiltIn {
        let problem = phazeai_core::local_ai::readiness_problem(&settings.local_ai);
        return ProviderUiStatus {
            available: problem.is_none(),
            summary: if problem.is_none() {
                "Ready"
            } else {
                "Set up below"
            }
            .into(),
            detail: problem.unwrap_or_else(|| phazeai_core::local_ai::LOCAL_AI.status().describe()),
        };
    }

    if provider_id.needs_api_key() {
        match config.api_key_source() {
            ApiKeySource::Keyring => {
                return ProviderUiStatus {
                    available: true,
                    summary: "Ready".into(),
                    detail: format!("Stored in OS keyring ({})", config.api_key_env),
                };
            }
            ApiKeySource::Env => {
                return ProviderUiStatus {
                    available: true,
                    summary: "Ready".into(),
                    detail: format!("Using env var {}", config.api_key_env),
                };
            }
            ApiKeySource::None => {
                return ProviderUiStatus {
                    available: false,
                    summary: "Missing API key".into(),
                    detail: format!(
                        "Paste your key below (stored securely) or set {}.",
                        config.api_key_env
                    ),
                };
            }
        }
    }

    ProviderUiStatus {
        available: true,
        summary: "Ready".into(),
        detail: format!("Local endpoint: {}", config.base_url),
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// A thin horizontal rule used to separate sections.
fn divider(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    container(label(|| "")).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.height(1.0)
            .width_full()
            .background(p.border)
            .margin_vert(8.0)
    })
}

/// An uppercase section header label.
fn section_header(text: &'static str, state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    label(move || text).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.font_size(10.0)
            .color(p.accent)
            .font_weight(floem::text::Weight::BOLD)
            .margin_bottom(10.0)
    })
}

/// A small +/- stepper button.
fn stepper_btn(
    text: &'static str,
    state: IdeState,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let theme = state.workbench.theme;
    let is_hovered = create_rw_signal(false);

    container(label(move || text).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let color = if is_hovered.get() {
            p.accent
        } else {
            p.text_secondary
        };
        s.font_size(16.0)
            .color(color)
            .font_weight(floem::text::Weight::BOLD)
    }))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let bg = if is_hovered.get() {
            p.bg_elevated
        } else {
            p.bg_surface
        };
        s.width(24.0)
            .height(24.0)
            .border(1.0)
            .border_color(p.border)
            .border_radius(4.0)
            .background(bg)
            .items_center()
            .justify_center()
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| on_click())
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
        is_hovered.set(true);
    })
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
        is_hovered.set(false);
    })
}

/// A labelled row with a stepper (label | spacer | − · value · +).
fn stepper_row(
    row_label: &'static str,
    value: floem::reactive::RwSignal<u32>,
    min: u32,
    max: u32,
    state: IdeState,
) -> impl IntoView {
    let theme = state.workbench.theme;
    let dec = stepper_btn("-", state.clone(), move || {
        value.update(|v| {
            if *v > min {
                *v -= 1;
            }
        });
    });

    let inc = stepper_btn("+", state.clone(), move || {
        value.update(|v| {
            if *v < max {
                *v += 1;
            }
        });
    });

    let value_display = container(label(move || value.get().to_string()).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.font_size(13.0).color(p.text_primary)
    }))
    .style(|s| s.width(28.0).items_center().justify_center());

    let controls = stack((dec, value_display, inc)).style(|s| s.flex_row().items_center().gap(2.0));

    stack((
        label(move || row_label).style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
        }),
        controls,
    ))
    .style(|s| s.flex_row().items_center().width_full().padding_vert(4.0))
}

/// A labelled ON/OFF switch bound to `sig`. `note`, when given, is a muted
/// line under it saying what the setting does.
fn toggle_row(
    label_text: &'static str,
    note: Option<&'static str>,
    sig: RwSignal<bool>,
    theme: RwSignal<PhazeTheme>,
) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let hov = create_rw_signal(false);
    let switch = container(label(move || if sig.get() { "ON" } else { "OFF" }))
        .style(move |s| {
            let p = theme.get().palette;
            let on = sig.get();
            s.font_size(font::CAPTION)
                .padding_horiz(space::MD)
                .padding_vert(space::XXS + 1.0)
                .border_radius(radius::SM)
                .color(p.bg_base)
                .background(if on { p.success } else { p.bg_elevated })
                .border(stroke::HAIRLINE)
                .border_color(if on { p.success } else { p.border })
                .cursor(floem::style::CursorStyle::Pointer)
                .apply_if(hov.get() && !on, |s| s.background(p.bg_panel))
        })
        .on_click_stop(move |_| {
            sig.update(|v| *v = !*v);
        })
        .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
            hov.set(true)
        })
        .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
            hov.set(false)
        });
    let row = container(
        stack((
            label(move || label_text).style(move |s| {
                let p = theme.get().palette;
                s.font_size(font::BODY).color(p.text_primary).flex_grow(1.0)
            }),
            switch,
        ))
        .style(|s| s.flex_row().items_center().padding_vert(space::XS)),
    )
    .style(|s| s.width_full().padding_horiz(space::XS));
    let note_line = label(move || note.unwrap_or_default()).style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.text_muted)
            .padding_horiz(space::XS)
            .margin_bottom(space::XS)
            .apply_if(note.is_none(), |s| s.display(floem::style::Display::None))
    });
    stack((row, note_line)).style(|s| s.flex_col().width_full())
}

// ─── theme tiles ─────────────────────────────────────────────────────────────

fn theme_tile(name: String, state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let theme = state.workbench.theme;
    let is_hovered = create_rw_signal(false);
    let label_name = name.clone();
    let style_name = name.clone();

    container(label(move || label_name.clone()).style({
        let name = name.clone();
        move |s| {
            let t = theme.get();
            let p = &t.palette;
            let active = t.display_name() == name;
            let color = if active { p.accent } else { p.text_secondary };
            s.font_size(font::BODY).color(color)
        }
    }))
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let active = t.display_name() == style_name;
        let hovered = is_hovered.get();
        let border_color = if active {
            p.accent
        } else if hovered {
            p.border_focus
        } else {
            p.border
        };
        let bg = if active {
            p.accent_dim
        } else if hovered {
            p.bg_elevated
        } else {
            p.bg_surface
        };
        s.min_width(100.0)
            .padding_vert(space::SM + 1.0)
            .padding_horiz(space::MD)
            .background(bg)
            .border(if active {
                stroke::FOCUS
            } else {
                stroke::HAIRLINE
            })
            .border_color(border_color)
            .border_radius(radius::MD)
            .items_center()
            .justify_center()
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| {
        theme.set(PhazeTheme::from_name(&name));
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
        is_hovered.set(true);
    })
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
        is_hovered.set(false);
    })
}

// ─── sections ────────────────────────────────────────────────────────────────

fn tile_grid(names: impl Fn() -> Vec<String> + 'static, state: IdeState) -> impl IntoView {
    dyn_stack(
        move || names().into_iter().enumerate().collect::<Vec<_>>(),
        |(i, name)| (*i, name.clone()),
        move |(_i, name)| theme_tile(name, state.clone()),
    )
    .style(|s| {
        s.flex_row()
            .flex_wrap(floem::style::FlexWrap::Wrap)
            .gap(space::SM)
            .width_full()
    })
}

fn theme_section(state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let theme = state.workbench.theme;

    let built_in = tile_grid(
        || {
            ThemeVariant::all()
                .iter()
                .map(|v| v.name().to_string())
                .collect()
        },
        state.clone(),
    );

    // Custom themes are re-read from disk on Reload and after an export, so
    // edits made in another editor show up without restarting.
    let loaded = create_rw_signal(load_custom_themes());
    let custom_names = move || {
        loaded.with(|l| {
            l.themes
                .iter()
                .map(|t| t.display_name().to_string())
                .collect::<Vec<_>>()
        })
    };
    let custom = tile_grid(custom_names, state.clone());
    let status = create_rw_signal(String::new());

    let themes_folder = themes_dir();
    let help = label(move || {
        let count = loaded.with(|l| l.themes.len());
        if count == 0 {
            "Your themes: none yet. Export the current theme to get an editable copy.".to_string()
        } else {
            format!("Your themes ({count})")
        }
    })
    .style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.text_muted)
            .margin_top(space::LG)
    });

    let errors = label(move || {
        loaded.with(|l| {
            l.errors
                .iter()
                .map(|(path, err)| {
                    let file = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    format!("{}: {err}", file.unwrap_or_default())
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
    })
    .style(move |s| {
        let has_errors = loaded.with(|l| !l.errors.is_empty());
        s.font_size(font::CAPTION)
            .color(theme.get().palette.error)
            .apply_if(!has_errors, |s| s.display(floem::style::Display::None))
    });

    let actions = stack((
        phaze_button(
            "Export current",
            ButtonVariant::Secondary,
            theme,
            move || {
                let current = theme.get_untracked();
                let name = export_name_for(&current);
                match export_theme(&current, &name) {
                    Ok(path) => {
                        loaded.set(load_custom_themes());
                        theme.set(PhazeTheme::from_name(&name));
                        status.set(format!("Saved \"{name}\" to {}", path.display()));
                    }
                    Err(e) => status.set(format!("Could not export theme: {e}")),
                }
            },
        ),
        phaze_button("Open folder", ButtonVariant::Ghost, theme, {
            let dir = themes_folder.clone();
            move || {
                let _ = std::fs::create_dir_all(&dir);
                if let Err(e) = open_external_url(&dir.to_string_lossy()) {
                    status.set(format!("Could not open {}: {e}", dir.display()));
                }
            }
        }),
        phaze_button("Reload", ButtonVariant::Ghost, theme, move || {
            let fresh = load_custom_themes();
            let (next, problem) = theme_after_reload(theme.get_untracked(), &fresh);
            loaded.set(fresh);
            theme.set(next);
            status.set(problem.unwrap_or_default());
        }),
    ))
    .style(|s| {
        s.flex_row()
            .flex_wrap(floem::style::FlexWrap::Wrap)
            .gap(space::SM)
            .margin_top(space::MD)
    });

    let status_line = label(move || status.get()).style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.text_muted)
            .apply_if(status.with(|m| m.is_empty()), |s| {
                s.display(floem::style::Display::None)
            })
    });

    let folder_hint =
        label(move || format!("Theme files: {}", themes_folder_hint())).style(move |s| {
            s.font_size(font::CAPTION)
                .color(theme.get().palette.text_disabled)
        });

    stack((
        section_header("THEME", state.clone()),
        built_in,
        help,
        custom,
        errors,
        actions,
        status_line,
        folder_hint,
    ))
    .style(|s| s.flex_col().width_full().gap(space::XS))
}

fn themes_folder_hint() -> String {
    themes_dir().display().to_string()
}

/// The theme to show after the themes folder was re-read, and a message for
/// the status line when the active theme couldn't be refreshed.
///
/// A custom theme whose file still loads comes back fresh, so edits take
/// effect. One that no longer loads (a syntax error, a deleted file) stays as
/// it is on screen: falling back to the built-in Dark theme would silently
/// throw away the user's choice over a typo in the file.
pub(crate) fn theme_after_reload(
    current: PhazeTheme,
    loaded: &LoadedThemes,
) -> (PhazeTheme, Option<String>) {
    let Some(name) = current.custom_name.clone() else {
        return (current, None);
    };
    if let Some(fresh) = loaded
        .themes
        .iter()
        .find(|t| t.display_name().eq_ignore_ascii_case(&name))
    {
        return (fresh.clone(), None);
    }
    let problems: Vec<String> = loaded
        .errors
        .iter()
        .map(|(path, err)| {
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned());
            format!("{}: {err}", file.unwrap_or_default())
        })
        .collect();
    let message = if problems.is_empty() {
        format!("\"{name}\" is no longer in the themes folder. Keeping it until you pick another theme.")
    } else {
        format!(
            "\"{name}\" didn't reload ({}). Keeping the current colours.",
            problems.join("; ")
        )
    };
    (current, Some(message))
}

fn editor_section(state: IdeState) -> impl IntoView {
    // Use the shared font_size and tab_size signals from IdeState.
    let font_size = state.editor.font_size;
    let tab_size = state.editor.tab_size;

    let auto_save = state.editor.auto_save;
    let word_wrap = state.editor.word_wrap;
    let organize_imports = state.editor.organize_imports_on_save;
    let code_lens_vis = state.editor.code_lens_visible;
    let inlay_hints = state.editor.inlay_hints_toggle;
    let relative_ln = state.editor.relative_line_numbers;
    let ai_completions = state.editor.ai_completions;
    let theme = state.workbench.theme;

    stack((
        section_header("EDITOR", state.clone()),
        stepper_row("Font Size", font_size, 8, 48, state.clone()),
        stepper_row("Tab Size", tab_size, 1, 16, state.clone()),
        toggle_row("Auto Save (1.5 s delay)", None, auto_save, theme),
        toggle_row("Word Wrap  (Alt+Z)", None, word_wrap, theme),
        toggle_row("Organize Imports on Save", None, organize_imports, theme),
        toggle_row("Show Code Lens", None, code_lens_vis, theme),
        toggle_row("Inlay Hints  (Ctrl+Alt+I)", None, inlay_hints, theme),
        toggle_row("Relative Line Numbers", None, relative_ln, theme),
        toggle_row(
            "AI Completions",
            Some(
                "Sends the code around the cursor to your AI provider while you \
                 edit. Off: nothing is sent.",
            ),
            ai_completions,
            theme,
        ),
    ))
    .style(|s| s.flex_col().width_full())
}

/// A clickable provider option tile.
fn provider_tile(name: &'static str, state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let theme = state.workbench.theme;
    let provider = state.ai.provider;
    let is_hovered = create_rw_signal(false);

    container(
        stack((
            label(move || name).style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let active = provider.get() == name;
                let status = provider_status(name);
                let color = if active {
                    p.accent
                } else if status.available {
                    p.text_primary
                } else {
                    p.text_secondary
                };
                s.font_size(11.0).color(color)
            }),
            label(move || provider_status(name).summary).style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                let status = provider_status(name);
                let color = if status.available {
                    p.success
                } else {
                    p.warning
                };
                s.font_size(10.0).color(color)
            }),
        ))
        .style(|s| s.flex_col().items_center().gap(2.0)),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let active = provider.get() == name;
        let hovered = is_hovered.get();
        let status = provider_status(name);
        let border_color = if active {
            p.accent
        } else if !status.available {
            p.warning
        } else if hovered {
            p.border_focus
        } else {
            p.border
        };
        let bg = if active {
            p.accent_dim
        } else if !status.available {
            p.bg_panel
        } else if hovered {
            p.bg_elevated
        } else {
            p.bg_surface
        };
        s.padding_vert(5.0)
            .padding_horiz(8.0)
            .min_width(118.0)
            .background(bg)
            .border(if active { 2.0 } else { 1.0 })
            .border_color(border_color)
            .border_radius(5.0)
            .items_center()
            .justify_center()
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_click_stop(move |_| {
        provider.set(name.to_string());
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
        is_hovered.set(true);
    })
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
        is_hovered.set(false);
    })
}

fn ai_section(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let ai_provider = state.ai.provider;
    let ai_model = state.ai.model;

    // Provider tiles — ordered: flagship → fast/cheap → specialized → local
    const PROVIDERS: &[&str] = &[
        "Claude (Anthropic)",
        "OpenAI",
        "Google Gemini",
        "Mistral AI",
        "DeepSeek",
        "xAI (Grok)",
        "Groq",
        "Cerebras",
        "Together.ai",
        "Fireworks AI",
        "SambaNova",
        "OpenRouter",
        "Perplexity",
        "Cohere",
        "Azure OpenAI",
        "GitHub Models",
        "PhazeAI Local (built-in)",
        "Ollama (Local)",
        "LM Studio (Local)",
    ];

    let provider_tiles = dyn_stack(
        move || {
            PROVIDERS
                .iter()
                .enumerate()
                .map(|(i, n)| (i, *n))
                .collect::<Vec<_>>()
        },
        |(i, _)| *i,
        {
            let state = state.clone();
            move |(_i, name)| provider_tile(name, state.clone())
        },
    )
    .style(|s| {
        s.flex_row()
            .flex_wrap(floem::style::FlexWrap::Wrap)
            .gap(4.0)
            .width_full()
    });

    let provider_section = stack((
        label(|| "Provider").style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.font_size(12.0).color(p.text_muted).margin_bottom(6.0)
        }),
        label(move || {
            let selected = ai_provider.get();
            provider_status(&selected).detail
        })
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let selected = ai_provider.get();
            let status = provider_status(&selected);
            let color = if status.available {
                p.text_muted
            } else {
                p.warning
            };
            s.font_size(11.0)
                .color(color)
                .margin_bottom(6.0)
                .line_height(1.3)
        }),
        provider_tiles,
    ))
    .style(|s| s.flex_col().width_full().padding_vert(4.0));

    // Model row — free-text input
    let model_row = stack((
        label(|| "Model").style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.font_size(13.0).color(p.text_primary).flex_grow(1.0)
        }),
        text_input(ai_model)
            .placeholder("e.g. claude-sonnet-4-6")
            .style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.width(160.0)
                    .background(p.bg_elevated)
                    .border(1.0)
                    .border_color(p.border_focus)
                    .border_radius(4.0)
                    .color(p.text_primary)
                    .padding_horiz(8.0)
                    .padding_vert(4.0)
                    .font_size(12.0)
                    .min_width(0.0)
            }),
    ))
    .style(|s| s.flex_row().items_center().width_full().padding_vert(4.0));

    let api_key_row = api_key_input_row(state.clone());

    stack((
        section_header("AI", state.clone()),
        provider_section,
        model_row,
        api_key_row,
    ))
    .style(|s| s.flex_col().width_full())
}

/// Where the Test button sends the key of `cfg` (from
/// `settings.build_provider_registry()`): the address its clients use, which
/// for the active provider is `[llm] base_url` when set. An address the
/// clients refuse (`Settings::check_base_url`) is refused here too, with
/// their message, before the key goes anywhere.
fn key_test_url(settings: &Settings, cfg: &ProviderConfig) -> Result<String, String> {
    if cfg.id == settings.llm.provider.to_provider_id() {
        settings.check_base_url().map_err(|e| e.to_string())?;
    }
    Ok(cfg.base_url.clone())
}

/// API key input row — masked text input with Save / Clear buttons. Saves to
/// the OS keyring under the active provider's `api_key_env` entry name.
fn api_key_input_row(state: IdeState) -> impl IntoView {
    use floem::reactive::{create_effect, create_rw_signal};
    let theme = state.workbench.theme;
    let ai_provider = state.ai.provider;

    let key_input = create_rw_signal(String::new());
    let feedback = create_rw_signal(String::new());

    // Clear the input (and any stale feedback) when the active provider changes,
    // so one provider's key can't accidentally be saved under another.
    let key_input_eff = key_input;
    let feedback_eff = feedback;
    let ai_provider_eff = ai_provider;
    create_effect(move |_| {
        let _ = ai_provider_eff.get();
        key_input_eff.set(String::new());
        feedback_eff.set(String::new());
    });

    // Helper: map the currently selected provider to (entry_name, needs_key).
    let active_entry = move || -> Option<(String, bool)> {
        let name = ai_provider.get();
        let id = provider_name_to_id(&name)?;
        let settings = Settings::load();
        let registry = settings.build_provider_registry();
        let cfg = registry.get_config(&id)?;
        Some((cfg.api_key_env.clone(), id.needs_api_key()))
    };

    let save_fn = {
        move || {
            let Some((entry, _)) = active_entry() else {
                feedback.set("Unknown provider.".into());
                return;
            };
            let val = key_input.get().trim().to_string();
            if val.is_empty() {
                feedback.set("Paste a key first.".into());
                return;
            }
            match keyring_set(&entry, &val) {
                Ok(()) => {
                    key_input.set(String::new());
                    feedback.set("Saved to keyring.".into());
                }
                Err(e) => feedback.set(format!("Save failed: {e}")),
            }
        }
    };

    let clear_fn = {
        move || {
            let Some((entry, _)) = active_entry() else {
                feedback.set("Unknown provider.".into());
                return;
            };
            match keyring_delete(&entry) {
                Ok(()) => feedback.set("Key cleared.".into()),
                Err(e) => feedback.set(format!("Clear failed: {e}")),
            }
        }
    };

    let test_fn = {
        move || {
            use floem::{ext_event::create_ext_action, reactive::Scope};
            let Some(id) = provider_name_to_id(&ai_provider.get()) else {
                feedback.set("Unknown provider.".into());
                return;
            };
            let settings = Settings::load();
            let registry = settings.build_provider_registry();
            let Some(cfg) = registry.get_config(&id) else {
                feedback.set("Unknown provider.".into());
                return;
            };
            // Test what is typed; fall back to the key already saved.
            let typed = key_input.get().trim().to_string();
            let key = if typed.is_empty() {
                cfg.api_key().unwrap_or_default()
            } else {
                typed
            };
            if key.is_empty() {
                feedback.set("Paste a key first.".into());
                return;
            }
            let base_url = match key_test_url(&settings, cfg) {
                Ok(url) => url,
                Err(refusal) => {
                    feedback.set(refusal);
                    return;
                }
            };
            feedback.set("Testing…".into());
            let send = create_ext_action(Scope::new(), move |msg: String| feedback.set(msg));
            std::thread::spawn(move || {
                let msg = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(check_api_key(&id, &base_url, &key)).message(),
                    Err(e) => format!("Could not start the check: {e}"),
                };
                send(msg);
            });
        }
    };

    let input = text_input(key_input)
        .placeholder("Paste API key — cleared after Save")
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let needs = provider_name_to_id(&ai_provider.get())
                .map(|id| id.needs_api_key())
                .unwrap_or(false);
            s.flex_grow(1.0)
                .min_width(0.0)
                .background(p.bg_elevated)
                .border(1.0)
                .border_color(p.border_focus)
                .border_radius(4.0)
                .color(p.text_primary)
                .padding_horiz(8.0)
                .padding_vert(4.0)
                .font_size(12.0)
                .apply_if(!needs, |s| s.display(floem::style::Display::None))
        });

    let save_btn = container(label(|| "Save"))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.padding_horiz(10.0)
                .padding_vert(4.0)
                .font_size(12.0)
                .color(p.text_primary)
                .background(p.accent_dim)
                .border(1.0)
                .border_color(p.accent)
                .border_radius(4.0)
                .margin_left(6.0)
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| save_fn());

    let test_btn = container(label(|| "Test"))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.padding_horiz(10.0)
                .padding_vert(4.0)
                .font_size(12.0)
                .color(p.text_primary)
                .background(p.bg_surface)
                .border(1.0)
                .border_color(p.border)
                .border_radius(4.0)
                .margin_left(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| test_fn());

    let clear_btn = container(label(|| "Clear"))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.padding_horiz(10.0)
                .padding_vert(4.0)
                .font_size(12.0)
                .color(p.text_muted)
                .background(p.bg_surface)
                .border(1.0)
                .border_color(p.border)
                .border_radius(4.0)
                .margin_left(4.0)
                .cursor(floem::style::CursorStyle::Pointer)
        })
        .on_click_stop(move |_| clear_fn());

    let input_row = stack((input, save_btn, test_btn, clear_btn))
        .style(|s| s.flex_row().items_center().width_full());

    let feedback_line = label(move || feedback.get()).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.font_size(11.0).color(p.text_muted).margin_top(4.0)
    });

    // Where to get a key, and what the free tier costs you (limits, training).
    let help_text = move || {
        provider_name_to_id(&ai_provider.get())
            .and_then(|id| key_help(&id))
            .map(|h| {
                if h.free_tier {
                    format!("Free tier available. {}", h.note)
                } else {
                    h.note.to_string()
                }
            })
            .unwrap_or_default()
    };
    let get_key_btn = container(label(|| "Get a key ↗"))
        .style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            let has_link = provider_name_to_id(&ai_provider.get())
                .and_then(|id| key_help(&id))
                .is_some();
            s.padding_horiz(10.0)
                .padding_vert(3.0)
                .font_size(12.0)
                .color(p.accent)
                .border(1.0)
                .border_color(p.accent)
                .border_radius(4.0)
                .margin_top(6.0)
                .cursor(floem::style::CursorStyle::Pointer)
                .apply_if(!has_link, |s| s.display(floem::style::Display::None))
        })
        .on_click_stop(move |_| {
            if let Some(h) = provider_name_to_id(&ai_provider.get()).and_then(|id| key_help(&id)) {
                if let Err(e) = crate::util::open_external_url(h.signup_url) {
                    feedback.set(format!("Could not open the browser: {e}"));
                }
            }
        });
    let help_line = label(help_text).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.font_size(11.0).color(p.text_muted).margin_top(6.0)
    });

    stack((
        label(|| "API Key").style(move |s| {
            let t = theme.get();
            let p = &t.palette;
            s.font_size(13.0).color(p.text_primary).margin_bottom(4.0)
        }),
        input_row,
        feedback_line,
        help_line,
        get_key_btn,
    ))
    .style(move |s| {
        let needs = provider_name_to_id(&ai_provider.get())
            .map(|id| id.needs_api_key())
            .unwrap_or(false);
        s.flex_col()
            .width_full()
            .padding_vert(4.0)
            .apply_if(!needs, |s| s.display(floem::style::Display::None))
    })
}

fn about_section(state: IdeState) -> impl IntoView {
    use floem::reactive::create_rw_signal;
    let theme = state.workbench.theme;
    let is_link_hovered = create_rw_signal(false);

    let icon_row = stack((
        phaze_icon(icons::AI, 22.0, move |p| p.accent, theme)
            .style(move |s: floem::style::Style| s.margin_right(8.0)),
        stack((
            label(|| "PhazeAI IDE").style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(15.0)
                    .color(p.text_primary)
                    .font_weight(floem::text::Weight::BOLD)
            }),
            label(|| "Version 0.1.0").style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(11.0).color(p.text_muted)
            }),
        ))
        .style(|s| s.flex_col().gap(2.0)),
    ))
    .style(|s| s.flex_row().items_center().padding_vert(4.0));

    let link = container(label(|| "github.com/phazeai/ide").style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        let color = if is_link_hovered.get() {
            p.accent_hover
        } else {
            p.accent
        };
        s.font_size(12.0).color(color)
    }))
    .style(move |s| {
        s.padding_vert(4.0)
            .cursor(floem::style::CursorStyle::Pointer)
    })
    .on_event_stop(floem::event::EventListener::PointerEnter, move |_| {
        is_link_hovered.set(true);
    })
    .on_event_stop(floem::event::EventListener::PointerLeave, move |_| {
        is_link_hovered.set(false);
    })
    .on_click_stop(move |_| {
        #[cfg(target_os = "linux")]
        let _ = phazeai_core::process::std_command("xdg-open")
            .arg("https://github.com/phazeai/ide")
            .spawn();
        #[cfg(target_os = "macos")]
        let _ = phazeai_core::process::std_command("open")
            .arg("https://github.com/phazeai/ide")
            .spawn();
        #[cfg(target_os = "windows")]
        let _ = phazeai_core::process::std_command("cmd")
            .args(["/c", "start", "https://github.com/phazeai/ide"])
            .spawn();
    });

    let update_check = toggle_row(
        "Check for Updates at Start",
        Some(
            "Asks GitHub's releases API for a newer version when PhazeAI starts. \
             Skipped when the folder PhazeAI starts in is local-only. A change \
             applies at the next start.",
        ),
        state.workbench.check_updates_at_start,
        theme,
    );

    stack((
        section_header("ABOUT", state.clone()),
        icon_row,
        update_check,
        link,
    ))
    .style(|s| s.flex_col().width_full())
}

// ─── keybindings reference ───────────────────────────────────────────────────────

fn keybindings_section(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;

    const BINDINGS: &[(&str, &str)] = &[
        // File
        ("Ctrl+O", "Open File"),
        ("Ctrl+S", "Save File"),
        ("Ctrl+P", "File Picker"),
        ("Ctrl+Shift+P", "Command Palette"),
        // Navigation
        ("Ctrl+G", "Go to Line"),
        ("F12", "Go to Definition"),
        ("Shift+F12", "Find All References"),
        ("F2", "Rename Symbol"),
        ("Ctrl+T", "Workspace Symbols"),
        // Editing
        ("Ctrl+/", "Toggle Comment"),
        ("Ctrl+D", "Select Next Occurrence"),
        ("Alt+Up", "Move Line Up"),
        ("Alt+Down", "Move Line Down"),
        ("Alt+Shift+Down", "Duplicate Line"),
        ("Ctrl+Shift+[", "Fold Block"),
        ("Ctrl+Shift+]", "Unfold Block"),
        ("Ctrl+Shift+K", "Delete Line"),
        ("Tab", "Accept Completion / Ghost Text"),
        // Search
        ("Ctrl+F", "Find in File"),
        ("Ctrl+H", "Find and Replace"),
        // View
        ("Ctrl+B", "Toggle Explorer"),
        ("Ctrl+J", "Toggle Terminal"),
        ("Ctrl+\\", "Toggle AI Chat"),
        ("Ctrl+Shift+Z", "Zen Mode"),
        ("Alt+Z", "Word Wrap"),
        ("Ctrl+= / Ctrl+-", "Zoom In / Out"),
        ("Ctrl+0", "Reset Zoom"),
        // AI
        ("Ctrl+K", "Inline AI Edit"),
        ("Ctrl+Space", "LSP Completions"),
        ("Ctrl+Shift+Space", "Signature Help"),
        ("Ctrl+.", "Code Actions"),
    ];

    let binding_rows = dyn_stack(
        move || {
            BINDINGS
                .iter()
                .enumerate()
                .map(|(i, b)| (i, b.0, b.1))
                .collect::<Vec<_>>()
        },
        |(i, _, _)| *i,
        move |(_i, shortcut, description)| {
            stack((
                container(label(move || shortcut)).style(move |s| {
                    let p = theme.get().palette;
                    s.width(140.0)
                        .font_size(11.0)
                        .color(p.accent)
                        .font_family("JetBrains Mono, Fira Code, monospace".to_string())
                        .padding_vert(3.0)
                }),
                label(move || description).style(move |s| {
                    let p = theme.get().palette;
                    s.font_size(12.0).color(p.text_secondary).flex_grow(1.0)
                }),
            ))
            .style(move |s| {
                let p = theme.get().palette;
                s.flex_row()
                    .items_center()
                    .width_full()
                    .padding_vert(2.0)
                    .padding_horiz(4.0)
                    .border_bottom(1.0)
                    .border_color(p.border.with_alpha(0.15))
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((section_header("KEYBINDINGS", state.clone()), binding_rows))
        .style(|s| s.flex_col().width_full())
}

// ─── public entry point ──────────────────────────────────────────────────────

/// The settings panel. Accepts IdeState so that theme/font_size/tab_size are
/// the shared signals — changes here propagate to the rest of the IDE and are
/// persisted to disk via reactive effects wired in IdeState::new().
pub fn settings_panel(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;

    // Panel header
    let header = container(
        stack((
            phaze_icon(icons::SETTINGS, 14.0, move |p| p.accent, theme),
            label(|| "  SETTINGS").style(move |s| {
                let t = theme.get();
                let p = &t.palette;
                s.font_size(11.0)
                    .color(p.accent)
                    .font_weight(floem::text::Weight::BOLD)
            }),
        ))
        .style(|s| s.flex_row().items_center()),
    )
    .style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.padding(12.0)
            .border_bottom(1.0)
            .border_color(p.border)
            .width_full()
    });

    // Scrollable body containing all sections
    let body = stack((
        theme_section(state.clone()),
        divider(state.clone()),
        editor_section(state.clone()),
        divider(state.clone()),
        ai_section(state.clone()),
        divider(state.clone()),
        crate::panels::local_ai::local_ai_section(state.clone()),
        crate::panels::privacy::privacy_section(state.clone()),
        divider(state.clone()),
        keybindings_section(state.clone()),
        divider(state.clone()),
        about_section(state.clone()),
        // Bottom breathing room
        container(label(|| "")).style(|s| s.height(24.0)),
    ))
    .style(|s| s.flex_col().width_full().padding(16.0).gap(0.0));

    let scrollable_body = scroll(body).style(|s| s.flex_grow(1.0).min_height(0.0).width_full());

    stack((header, scrollable_body)).style(move |s| {
        let t = theme.get();
        let p = &t.palette;
        s.flex_col()
            .width_full()
            .height_full()
            .background(p.bg_panel)
    })
}

#[cfg(test)]
mod tests {
    use super::theme_after_reload;
    use crate::custom_theme::{parse_hex_color, parse_theme, LoadedThemes};
    use crate::theme::{PhazeTheme, ThemeVariant};

    fn ember(accent: &str) -> PhazeTheme {
        parse_theme(&format!(
            "name = \"Ember\"\n[colors]\naccent = \"{accent}\"\n"
        ))
        .unwrap()
    }

    #[test]
    fn reload_keeps_a_custom_theme_whose_file_broke() {
        let current = ember("#ff7a18");
        let broken = LoadedThemes {
            themes: Vec::new(),
            errors: vec![(
                std::path::PathBuf::from("/themes/ember.toml"),
                "colors.accent: `red` is not a #rrggbb colour".to_string(),
            )],
        };
        let (next, problem) = theme_after_reload(current.clone(), &broken);
        assert_eq!(next.display_name(), "Ember");
        assert_eq!(next.palette.accent, current.palette.accent);
        let problem = problem.unwrap();
        assert!(problem.contains("ember.toml"), "{problem}");
        assert!(problem.contains("not a #rrggbb colour"), "{problem}");
    }

    #[test]
    fn reload_applies_a_custom_theme_that_still_loads() {
        let fresh = LoadedThemes {
            themes: vec![ember("#00ff00")],
            errors: Vec::new(),
        };
        let (next, problem) = theme_after_reload(ember("#ff7a18"), &fresh);
        assert_eq!(problem, None);
        assert_eq!(next.palette.accent, parse_hex_color("#00ff00").unwrap());
    }

    #[test]
    fn reload_leaves_built_in_themes_alone_and_reports_missing_files() {
        let dracula = PhazeTheme::from_variant(ThemeVariant::Dracula);
        let (next, problem) = theme_after_reload(dracula, &LoadedThemes::default());
        assert_eq!(next.variant, ThemeVariant::Dracula);
        assert_eq!(problem, None);

        let (next, problem) = theme_after_reload(ember("#ff7a18"), &LoadedThemes::default());
        assert_eq!(next.display_name(), "Ember");
        assert!(problem.unwrap().contains("no longer"));
    }

    #[test]
    fn the_key_test_refuses_an_address_the_clients_refuse() {
        use super::key_test_url;
        use phazeai_core::{config::LlmProvider, llm::provider::ProviderId, Settings};

        // What an older build left behind: an Ollama on another machine as
        // base_url, provider since switched to Claude.
        let mut s = Settings::default();
        s.llm.provider = LlmProvider::Claude;
        s.llm.base_url = Some("http://192.0.2.2:11434".into());
        let registry = s.build_provider_registry();
        let claude = registry.get_config(&ProviderId::Claude).unwrap();
        let refusal = key_test_url(&s, claude).expect_err("the key must not be sent there");
        assert!(
            refusal.contains("base_url") && refusal.contains("unencrypted"),
            "{refusal}"
        );

        // Another provider's key is tested at that provider's own address.
        let openai = registry.get_config(&ProviderId::OpenAI).unwrap();
        assert_eq!(
            key_test_url(&s, openai).unwrap(),
            ProviderId::OpenAI.default_base_url()
        );

        // Over https, the configured address is the one tested.
        s.llm.base_url = Some("https://gateway.example/anthropic".into());
        let registry = s.build_provider_registry();
        let claude = registry.get_config(&ProviderId::Claude).unwrap();
        assert_eq!(
            key_test_url(&s, claude).unwrap(),
            "https://gateway.example/anthropic"
        );
    }
}
