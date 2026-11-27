//! What the editor remembers between typing pauses, so that a pause doesn't
//! always cost an inline completion request.
//!
//! Two cases need no new request:
//! - **Typing what the suggestion shows.** The rest of the suggestion is
//!   still the answer, so it stays on screen, shortened by what was typed.
//! - **Coming back to text already asked about.** Backspacing over a typo,
//!   or an undo, returns the text around the cursor to a state the model has
//!   already answered for, so its answer (even an empty one) is used again.
//!
//! Copilot and other editors keep a suggestion while it is typed the same
//! way. The editor keeps one [`SuggestionMemory`] per tab.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

/// How many answers are kept for reuse.
const REMEMBERED: usize = 32;

/// What to do at a typing pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtPause {
    /// Show this suggestion; no request is needed.
    Show(String),
    /// The model already answered nothing for this text; ask nothing.
    Nothing,
    /// Ask the model.
    Ask,
}

/// The text around the cursor, as hashes of what is before and after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    prefix: u64,
    suffix: u64,
}

impl Key {
    fn of(prefix: &str, suffix: &str) -> Key {
        Key {
            prefix: hash(prefix),
            suffix: hash(suffix),
        }
    }
}

/// The suggestion on screen and the text it was shown for.
#[derive(Debug, Clone)]
struct Shown {
    prefix_len: usize,
    key: Key,
    text: String,
}

/// Recent answers, and the suggestion on screen. See the module docs.
#[derive(Debug, Default)]
pub struct SuggestionMemory {
    shown: Option<Shown>,
    recent: VecDeque<(Key, String)>,
}

impl SuggestionMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// What to do at a typing pause, with `prefix` and `suffix` the whole
    /// text before and after the cursor.
    pub fn at_pause(&mut self, prefix: &str, suffix: &str) -> AtPause {
        let key = Key::of(prefix, suffix);
        if let Some(rest) = self.typed_through(prefix, key) {
            self.show(prefix, key, rest.clone());
            return AtPause::Show(rest);
        }
        self.shown = None;
        match self.recent.iter().find(|(k, _)| *k == key) {
            Some((_, text)) if text.is_empty() => AtPause::Nothing,
            Some((_, text)) => {
                let text = text.clone();
                self.show(prefix, key, text.clone());
                AtPause::Show(text)
            }
            None => AtPause::Ask,
        }
    }

    /// Keep the model's answer for `prefix` and `suffix` (an empty answer
    /// too: asking again would get nothing again). `shown` says whether the
    /// editor put it on screen, which only happens while the cursor is still
    /// where it was asked for.
    pub fn remember(&mut self, prefix: &str, suffix: &str, answer: &str, shown: bool) {
        let key = Key::of(prefix, suffix);
        self.recent.retain(|(k, _)| *k != key);
        if self.recent.len() == REMEMBERED {
            self.recent.pop_front();
        }
        self.recent.push_back((key, answer.to_string()));
        if shown && !answer.is_empty() {
            self.show(prefix, key, answer.to_string());
        }
    }

    /// The suggestion on screen was taken (accepted) or dismissed.
    pub fn clear_shown(&mut self) {
        self.shown = None;
    }

    fn show(&mut self, prefix: &str, key: Key, text: String) {
        self.shown = Some(Shown {
            prefix_len: prefix.len(),
            key,
            text,
        });
    }

    /// The rest of the suggestion on screen, when the text before the cursor
    /// is what it was shown for plus the start of the suggestion, and the
    /// text after the cursor is unchanged. `None` once nothing but
    /// whitespace would be left: the suggestion has been typed in full.
    fn typed_through(&self, prefix: &str, key: Key) -> Option<String> {
        let shown = self.shown.as_ref()?;
        if key.suffix != shown.key.suffix
            || prefix.len() <= shown.prefix_len
            || !prefix.is_char_boundary(shown.prefix_len)
            || hash(&prefix[..shown.prefix_len]) != shown.key.prefix
        {
            return None;
        }
        let rest = shown.text.strip_prefix(&prefix[shown.prefix_len..])?;
        (!rest.trim().is_empty()).then(|| rest.to_string())
    }
}

fn hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_known_asks() {
        let mut m = SuggestionMemory::new();
        assert_eq!(m.at_pause("fn main() {", "\n}"), AtPause::Ask);
    }

    #[test]
    fn typing_what_is_shown_keeps_the_rest_on_screen() {
        let mut m = SuggestionMemory::new();
        m.remember("let total = ", ";", "items.len()", true);
        assert_eq!(
            m.at_pause("let total = it", ";"),
            AtPause::Show("ems.len()".into())
        );
        // And again from there.
        assert_eq!(
            m.at_pause("let total = items.", ";"),
            AtPause::Show("len()".into())
        );
    }

    #[test]
    fn typing_something_else_asks() {
        let mut m = SuggestionMemory::new();
        m.remember("let total = ", ";", "items.len()", true);
        assert_eq!(m.at_pause("let total = co", ";"), AtPause::Ask);
        // The suggestion is gone from the screen, so typing it later asks too.
        assert_eq!(m.at_pause("let total = coit", ";"), AtPause::Ask);
    }

    #[test]
    fn typing_the_whole_suggestion_asks_for_the_next() {
        let mut m = SuggestionMemory::new();
        m.remember("x = ", "", "1;", true);
        assert_eq!(m.at_pause("x = 1;", ""), AtPause::Ask);
        m.remember("x = ", "", "1;\n", true);
        assert_eq!(m.at_pause("x = 1;", ""), AtPause::Ask);
    }

    #[test]
    fn a_change_after_the_cursor_asks() {
        let mut m = SuggestionMemory::new();
        m.remember("let total = ", ";", "items.len()", true);
        assert_eq!(m.at_pause("let total = it", "; // count"), AtPause::Ask);
    }

    #[test]
    fn a_change_before_the_shown_text_asks() {
        let mut m = SuggestionMemory::new();
        m.remember("let total = ", ";", "items.len()", true);
        assert_eq!(m.at_pause("let totals = it", ";"), AtPause::Ask);
    }

    #[test]
    fn an_answer_that_arrived_too_late_to_show_is_not_typed_through() {
        let mut m = SuggestionMemory::new();
        m.remember("let total = ", ";", "items.len()", false);
        assert_eq!(m.at_pause("let total = it", ";"), AtPause::Ask);
        // But coming back to the text it was for shows it.
        assert_eq!(
            m.at_pause("let total = ", ";"),
            AtPause::Show("items.len()".into())
        );
    }

    #[test]
    fn backspacing_to_text_already_asked_about_reuses_the_answer() {
        let mut m = SuggestionMemory::new();
        m.remember("let total = ", ";", "items.len()", true);
        assert_eq!(m.at_pause("let total = zz", ";"), AtPause::Ask);
        m.remember("let total = zz", ";", "", true);
        assert_eq!(
            m.at_pause("let total = ", ";"),
            AtPause::Show("items.len()".into())
        );
        // An empty answer is reused as nothing to ask.
        assert_eq!(m.at_pause("let total = zz", ";"), AtPause::Nothing);
    }

    #[test]
    fn a_dismissed_suggestion_is_not_typed_through() {
        let mut m = SuggestionMemory::new();
        m.remember("let total = ", ";", "items.len()", true);
        m.clear_shown();
        assert_eq!(m.at_pause("let total = it", ";"), AtPause::Ask);
    }

    #[test]
    fn typing_through_works_across_multibyte_text() {
        let mut m = SuggestionMemory::new();
        m.remember("let s = \"é", "\";", "tude\"", true);
        assert_eq!(
            m.at_pause("let s = \"ét", "\";"),
            AtPause::Show("ude\"".into())
        );
    }

    #[test]
    fn only_the_newest_answers_are_kept() {
        let mut m = SuggestionMemory::new();
        for i in 0..=REMEMBERED {
            m.remember(&format!("line {i}"), "", "x", false);
        }
        assert_eq!(m.at_pause("line 0", ""), AtPause::Ask);
        assert_eq!(
            m.at_pause(&format!("line {REMEMBERED}"), ""),
            AtPause::Show("x".into())
        );
    }
}
