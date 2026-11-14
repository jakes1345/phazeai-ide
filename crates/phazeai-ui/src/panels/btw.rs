//! `/btw` in the chat panel: a side question answered by PhazeAI Local (free,
//! on this machine), shown above the input. It works while a reply is
//! running, and neither the question nor the answer enters the conversation
//! (see `phazeai_core::btw`).

use floem::{
    ext_event::create_signal_from_channel,
    reactive::{create_effect, create_rw_signal, RwSignal, SignalGet, SignalUpdate, SignalWith},
    views::{container, dyn_stack, label, scroll, stack, Decorators},
    IntoView,
};
use phazeai_core::btw::{self, Exchange, SessionLine, Who};
use phazeai_core::Settings;

use super::chat::{ChatMessage, ChatRole};
use crate::theme::PhazeTheme;

/// The question in `input` if it is a `/btw` command (`""` for a bare
/// `/btw`), else `None`.
pub fn btw_question(input: &str) -> Option<&str> {
    let rest = input.trim_start().strip_prefix("/btw")?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None // `/btwx` is something else
    }
}

/// The chat as a side question sees it.
pub fn session_lines(messages: &[ChatMessage]) -> Vec<SessionLine> {
    messages
        .iter()
        .filter(|m| !m.is_error && !m.content.trim().is_empty())
        .map(|m| {
            let who = match m.role {
                ChatRole::User => Who::User,
                ChatRole::Assistant => Who::Assistant,
                ChatRole::Tool => Who::Tool,
            };
            SessionLine::new(who, m.content.clone())
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
struct Item {
    id: u64,
    question: String,
    answer: String,
    pending: bool,
    error: bool,
}

/// One update from the worker: the answer so far, or how it ended.
#[derive(Clone)]
struct Update {
    id: u64,
    text: String,
    done: bool,
    error: bool,
}

/// The side questions of one chat panel.
#[derive(Clone)]
pub struct BtwState {
    items: RwSignal<Vec<Item>>,
    next_id: RwSignal<u64>,
    tx: std::sync::mpsc::SyncSender<Update>,
}

impl Default for BtwState {
    fn default() -> Self {
        Self::new()
    }
}

impl BtwState {
    pub fn new() -> Self {
        let items = create_rw_signal(Vec::<Item>::new());
        let (tx, rx) = std::sync::mpsc::sync_channel::<Update>(256);
        let updates = create_signal_from_channel(rx);
        create_effect(move |_| {
            // Each update carries the whole answer so far, so one that is
            // skipped is harmless; the last one is what stays.
            let Some(u) = updates.get() else { return };
            items.update(|list| {
                if let Some(item) = list.iter_mut().find(|i| i.id == u.id) {
                    item.answer = u.text;
                    item.pending = !u.done;
                    item.error = u.error;
                }
            });
        });
        Self {
            items,
            next_id: create_rw_signal(0),
            tx,
        }
    }

    /// Ask `question` about `session` on PhazeAI Local, off the UI thread.
    pub fn ask(&self, question: &str, session: Vec<SessionLine>) {
        if question.is_empty() {
            self.add(
                "/btw".into(),
                "Ask a quick side question about this chat, e.g. `/btw what does stock_value \
                 return?`. PhazeAI Local answers it for free while the main reply keeps going; \
                 it isn't added to the conversation."
                    .into(),
                false,
            );
            return;
        }
        let earlier: Vec<Exchange> = self.items.with_untracked(|list| {
            list.iter()
                .filter(|i| !i.pending && !i.error && i.question != "/btw")
                .map(|i| Exchange {
                    question: i.question.clone(),
                    answer: i.answer.clone(),
                })
                .collect()
        });
        let id = self.add(question.to_string(), String::new(), true);
        let tx = self.tx.clone();
        let question = question.to_string();
        std::thread::spawn(move || {
            let settings = Settings::load();
            let prompt = btw::prompt(&session, &earlier, &question, btw::budget_chars(&settings));
            let result = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(async {
                    let mut so_far = String::new();
                    btw::ask(&settings, &prompt, |piece| {
                        so_far.push_str(piece);
                        // A full channel only drops an intermediate update.
                        let _ = tx.try_send(Update {
                            id,
                            text: so_far.clone(),
                            done: false,
                            error: false,
                        });
                    })
                    .await
                }),
                Err(e) => Err(e.to_string()),
            };
            let (text, error) = match result {
                Ok(answer) if answer.is_empty() => ("(no answer)".to_string(), false),
                Ok(answer) => (answer, false),
                Err(e) => (e, true),
            };
            let _ = tx.send(Update {
                id,
                text,
                done: true,
                error,
            });
        });
    }

    fn add(&self, question: String, answer: String, pending: bool) -> u64 {
        let id = self.next_id.get_untracked();
        self.next_id.set(id + 1);
        self.items.update(|list| {
            list.push(Item {
                id,
                question,
                answer,
                pending,
                error: false,
            })
        });
        id
    }
}

/// The side answers above the chat input; hidden while there are none.
pub fn btw_panel(state: BtwState, theme: RwSignal<PhazeTheme>) -> impl IntoView {
    let items = state.items;
    let header = stack((
        label(|| "Side answers · PhazeAI Local, free · not in the conversation").style(move |s| {
            s.font_size(10.0)
                .color(theme.get().palette.text_muted)
                .flex_grow(1.0)
        }),
        label(|| "✕")
            .style(move |s| {
                s.font_size(11.0)
                    .padding_horiz(4.0)
                    .color(theme.get().palette.text_secondary)
                    .cursor(floem::style::CursorStyle::Pointer)
            })
            .on_click_stop(move |_| items.set(Vec::new())),
    ))
    .style(|s| s.items_center().width_full().margin_bottom(4.0));

    let list = scroll(
        dyn_stack(
            move || items.get(),
            |item: &Item| (item.id, item.answer.len(), item.pending, item.error),
            move |item: Item| {
                let answer = if item.pending && item.answer.is_empty() {
                    "Thinking… (the first question loads the local model)".to_string()
                } else {
                    item.answer.clone()
                };
                stack((
                    label(move || format!("btw: {}", item.question)).style(move |s| {
                        s.font_size(11.0)
                            .font_weight(floem::text::Weight::BOLD)
                            .color(theme.get().palette.accent)
                    }),
                    label(move || answer.clone()).style(move |s| {
                        let p = theme.get().palette;
                        s.font_size(12.0)
                            .line_height(1.4)
                            .width_full()
                            .min_width(0.0)
                            .color(if item.error { p.error } else { p.text_primary })
                    }),
                ))
                .style(|s| s.flex_col().width_full().gap(2.0).margin_bottom(6.0))
            },
        )
        .style(|s| s.flex_col().width_full()),
    )
    .style(|s| s.width_full().max_height(220.0));

    container(stack((header, list)).style(|s| s.flex_col().width_full())).style(move |s| {
        let p = theme.get().palette;
        s.width_full()
            .padding(8.0)
            .margin_bottom(6.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(p.glass_border)
            .background(p.bg_elevated)
            .apply_if(items.with(|l| l.is_empty()), |s| {
                s.display(floem::style::Display::None)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_btw_commands() {
        assert_eq!(btw_question("/btw what is qty?"), Some("what is qty?"));
        assert_eq!(btw_question("  /btw   spaced  "), Some("spaced"));
        assert_eq!(btw_question("/btw"), Some(""));
        assert_eq!(btw_question("/btwx hi"), None);
        assert_eq!(btw_question("tell me /btw"), None);
        assert_eq!(btw_question("/skill btw"), None);
    }

    #[test]
    fn session_lines_skip_errors_and_empty_rows() {
        let msg = |role, content: &str, is_error| ChatMessage {
            role,
            content: content.into(),
            loading: false,
            is_error,
        };
        let lines = session_lines(&[
            msg(ChatRole::User, "fix it", false),
            msg(ChatRole::Tool, "read_file: ok", false),
            msg(ChatRole::Assistant, "", false),
            msg(ChatRole::Assistant, "Error: boom", true),
            msg(ChatRole::Assistant, "done", false),
        ]);
        assert_eq!(
            lines,
            vec![
                SessionLine::new(Who::User, "fix it"),
                SessionLine::new(Who::Tool, "read_file: ok"),
                SessionLine::new(Who::Assistant, "done"),
            ]
        );
    }
}
