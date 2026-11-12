//! Settings › Code search: match code by meaning with a local embedding model.
//!
//! The model and address are saved under `[search]` in config.toml and take
//! effect at once: the engine is told, and the chunks of the index are sent to
//! the model in the background. The status line follows the engine.

use std::time::Duration;

use floem::{
    action::exec_after,
    reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate},
    views::{label, stack, text_input, Decorators},
    IntoView,
};
use phazeai_core::config::SearchSettings;
use phazeai_core::search::{EmbeddingStatus, DEFAULT_EMBEDDING_URL};
use phazeai_core::Settings;

use crate::components::button::{phaze_button, ButtonVariant};
use crate::domain_state::IdeState;
use crate::tokens::font;

/// How often the status line reads the engine.
const POLL: Duration = Duration::from_secs(1);

/// The status line for meaning-based matching.
pub(crate) fn embedding_status_text(enabled: bool, status: Option<&EmbeddingStatus>) -> String {
    if !enabled {
        return "Code search is turned off ([search] enabled = false).".into();
    }
    let Some(s) = status else {
        return "Off: code search matches keywords only.".into();
    };
    if let Some(e) = &s.error {
        return format!("Not working: {e}");
    }
    if s.total == 0 {
        return format!("{}: waits for the code index to be built.", s.model);
    }
    if s.syncing || s.ready < s.total {
        return format!(
            "{}: {} of {} chunks embedded{}",
            s.model,
            s.ready,
            s.total,
            if s.syncing { "…" } else { "" }
        );
    }
    format!("{}: all {} chunks matched by meaning.", s.model, s.total)
}

/// Fill `search` from the two boxes: an empty model turns matching by meaning
/// off; an empty address means Ollama's default. Returns an error for a model
/// with spaces in it, which no server accepts.
pub(crate) fn apply_fields(
    search: &mut SearchSettings,
    model: &str,
    url: &str,
) -> Result<(), String> {
    let model = model.trim();
    let url = url.trim();
    if model.chars().any(char::is_whitespace) {
        return Err("A model name has no spaces, e.g. nomic-embed-text".into());
    }
    if !(url.is_empty() || url.starts_with("http://") || url.starts_with("https://")) {
        return Err("The address starts with http:// or https://".into());
    }
    search.embedding_model = (!model.is_empty()).then(|| model.to_string());
    search.embedding_url = (!url.is_empty()).then(|| url.to_string());
    Ok(())
}

fn input_style(
    theme: RwSignal<crate::theme::PhazeTheme>,
) -> impl Fn(floem::style::Style) -> floem::style::Style {
    move |s| {
        let p = theme.get().palette;
        s.width(200.0)
            .background(p.bg_elevated)
            .border(1.0)
            .border_color(p.border_focus)
            .border_radius(4.0)
            .color(p.text_primary)
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .font_size(12.0)
            .min_width(0.0)
    }
}

fn row(
    name: &'static str,
    input: impl IntoView + 'static,
    theme: RwSignal<crate::theme::PhazeTheme>,
) -> impl IntoView {
    stack((
        label(move || name).style(move |s| {
            s.font_size(13.0)
                .color(theme.get().palette.text_primary)
                .flex_grow(1.0)
        }),
        input,
    ))
    .style(|s| s.flex_row().items_center().width_full().padding_vert(4.0))
}

/// Re-read the engine's status every [`POLL`] for as long as the app runs
/// (the settings panel is built once).
fn follow(state: IdeState, status: RwSignal<String>) {
    exec_after(POLL, move |_| {
        let p = &state.project;
        let text = embedding_status_text(
            p.code_search_enabled,
            p.code_search.embedding_status().as_ref(),
        );
        if status.get_untracked() != text {
            status.set(text);
        }
        follow(state, status);
    });
}

pub fn code_search_section(state: IdeState) -> impl IntoView {
    let theme = state.workbench.theme;
    let initial = Settings::load().search;
    let model = create_rw_signal(initial.embedding_model.clone().unwrap_or_default());
    let url = create_rw_signal(initial.embedding_url.clone().unwrap_or_default());
    let message: RwSignal<Option<(bool, String)>> = create_rw_signal(None);
    let status = create_rw_signal(embedding_status_text(
        state.project.code_search_enabled,
        state.project.code_search.embedding_status().as_ref(),
    ));
    follow(state.clone(), status);

    let intro = label(|| {
        "Match code by meaning as well as by keywords, with an embedding model. Each \
         chunk of code is sent to the model once and its vector kept on disk; privacy \
         deny rules and local_only apply. With Ollama: ollama pull nomic-embed-text."
            .to_string()
    })
    .style(move |s| {
        s.font_size(font::CAPTION)
            .color(theme.get().palette.text_muted)
            .margin_bottom(6.0)
            .line_height(1.3)
    });

    let model_row = row(
        "Embedding model",
        text_input(model)
            .placeholder("e.g. nomic-embed-text")
            .style(input_style(theme)),
        theme,
    );
    let url_row = row(
        "Address",
        text_input(url)
            .placeholder(DEFAULT_EMBEDDING_URL)
            .style(input_style(theme)),
        theme,
    );

    let apply_state = state.clone();
    let apply = phaze_button("Apply", ButtonVariant::Primary, theme, move || {
        let (m, u) = (model.get_untracked(), url.get_untracked());
        let result = {
            let _guard = crate::app::SETTINGS_WRITE_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut settings = Settings::load();
            apply_fields(&mut settings.search, &m, &u).and_then(|()| {
                settings
                    .save()
                    .map(|()| settings.search.clone())
                    .map_err(|e| format!("Could not save settings: {e}"))
            })
        };
        match result {
            Ok(search) => {
                let engine = &apply_state.project.code_search;
                engine.set_embeddings(search.embedding_config());
                engine.start_embeddings();
                let text = if search.embedding_model.is_some() {
                    "Saved. Embedding starts after the code index is built."
                } else {
                    "Saved. Code search matches keywords only."
                };
                message.set(Some((false, text.into())));
            }
            Err(e) => message.set(Some((true, e))),
        }
    });

    let status_line = label(move || status.get()).style(move |s| {
        let p = theme.get().palette;
        let broken = status.get().starts_with("Not working");
        s.font_size(font::CAPTION)
            .color(if broken { p.warning } else { p.text_muted })
            .margin_top(4.0)
            .line_height(1.3)
    });
    let message_line =
        label(move || message.get().map(|(_, t)| t).unwrap_or_default()).style(move |s| {
            let p = theme.get().palette;
            let m = message.get();
            let is_err = m.as_ref().is_some_and(|(e, _)| *e);
            s.font_size(font::CAPTION)
                .color(if is_err { p.error } else { p.text_muted })
                .apply_if(m.is_none(), |s| s.display(floem::style::Display::None))
        });

    stack((
        crate::panels::settings::section_header_pub("Code search", state.clone()),
        intro,
        model_row,
        url_row,
        stack((apply,)).style(|s| s.flex_row().padding_vert(4.0)),
        message_line,
        status_line,
    ))
    .style(|s| s.flex_col().width_full())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(ready: usize, total: usize, syncing: bool, error: Option<&str>) -> EmbeddingStatus {
        EmbeddingStatus {
            model: "nomic-embed-text".into(),
            ready,
            total,
            syncing,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn the_status_line_says_what_is_happening() {
        assert!(embedding_status_text(false, None).contains("turned off"));
        assert!(embedding_status_text(true, None).contains("keywords only"));
        assert_eq!(
            embedding_status_text(true, Some(&status(40, 100, true, None))),
            "nomic-embed-text: 40 of 100 chunks embedded…"
        );
        assert!(
            embedding_status_text(true, Some(&status(100, 100, false, None)))
                .contains("all 100 chunks")
        );
        assert!(
            embedding_status_text(true, Some(&status(0, 0, false, None)))
                .contains("waits for the code index")
        );
        assert!(embedding_status_text(
            true,
            Some(&status(3, 10, false, Some("connection refused")))
        )
        .starts_with("Not working: connection refused"));
    }

    #[test]
    fn the_fields_fill_the_search_settings() {
        let mut s = SearchSettings::default();
        apply_fields(&mut s, " nomic-embed-text ", "").unwrap();
        assert_eq!(s.embedding_model.as_deref(), Some("nomic-embed-text"));
        assert_eq!(s.embedding_url, None);
        assert!(s.embedding_config().unwrap().is_local());

        apply_fields(&mut s, "e5", "http://localhost:8080/v1").unwrap();
        assert_eq!(
            s.embedding_config().unwrap().endpoint(),
            "http://localhost:8080/v1/embeddings"
        );

        apply_fields(&mut s, "", "").unwrap();
        assert!(s.embedding_config().is_none());

        assert!(apply_fields(&mut s, "two words", "").is_err());
        assert!(apply_fields(&mut s, "m", "localhost:11434").is_err());
    }
}
