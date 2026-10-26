//! Keyboard focus for text boxes inside overlays.
//!
//! Floem delivers key presses to the focused view only, and nothing focuses a
//! text input when it appears. An overlay opened from the keyboard (Ctrl+P,
//! Ctrl+T, Ctrl+K …) therefore ignored typing until it was clicked, and the
//! keys went to whatever had focus before it, such as the editor.

use std::time::Duration;

use floem::{action::exec_after, reactive::create_effect, View, ViewId};

/// Delay between an overlay opening and focusing its input. The overlay is
/// `display: none` while closed; this lets it be shown and laid out first.
const FOCUS_DELAY: Duration = Duration::from_millis(20);

/// Give `id` keyboard focus each time `open` turns true, and release it when
/// `open` turns false so a closed overlay can't keep swallowing keystrokes.
pub fn focus_while_open(id: ViewId, open: impl Fn() -> bool + 'static) {
    create_effect(move |was_open: Option<bool>| {
        let is_open = open();
        if is_open && was_open != Some(true) {
            exec_after(FOCUS_DELAY, move |_| id.request_focus());
        } else if !is_open && was_open == Some(true) {
            id.clear_focus();
        }
        is_open
    });
}

/// `.focus_when_open(…)` for any view, so an overlay's text input can opt in
/// where it is built.
pub trait FocusWhenOpen: View + Sized {
    /// Focus this view while `open` is true (see [`focus_while_open`]).
    fn focus_when_open(self, open: impl Fn() -> bool + 'static) -> Self {
        focus_while_open(self.id(), open);
        self
    }
}

impl<V: View> FocusWhenOpen for V {}
