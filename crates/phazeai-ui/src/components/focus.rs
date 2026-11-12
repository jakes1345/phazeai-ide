//! Keyboard focus for text boxes inside overlays.
//!
//! Floem delivers key presses to the focused view only, and nothing focuses a
//! text input when it appears. An overlay opened from the keyboard (Ctrl+P,
//! Ctrl+T, Ctrl+K …) therefore ignored typing until it was clicked, and the
//! keys went to whatever had focus before it, such as the editor.

use std::time::Duration;

use floem::{
    action::exec_after,
    reactive::{create_effect, RwSignal, Scope, SignalUpdate},
    View, ViewId,
};

/// Delay between an overlay opening and focusing its input. The overlay is
/// `display: none` while closed; this lets it be shown and laid out first.
pub const FOCUS_DELAY: Duration = Duration::from_millis(20);

thread_local! {
    /// Bumped when focus should go back to the editor; see
    /// [`editor_refocus_requests`]. In a root scope, so it lives as long as
    /// the UI thread.
    static EDITOR_REFOCUS: RwSignal<u64> = Scope::new().create_rw_signal(0);
}

thread_local! {
    /// Bumped by an Escape no overlay used; see [`unclaimed_escapes`].
    static UNCLAIMED_ESCAPE: RwSignal<u64> = Scope::new().create_rw_signal(0);
}

/// Report an Escape press that closed no overlay. A focused text box takes
/// Escape itself (Floem only drops its focus), so a bar whose input had focus,
/// such as the editor's find bar, never sees the key; it watches
/// [`unclaimed_escapes`] instead.
pub fn escape_unclaimed() {
    UNCLAIMED_ESCAPE.with(|s| s.update(|n| *n = n.wrapping_add(1)));
}

/// The signal [`escape_unclaimed`] bumps.
pub fn unclaimed_escapes() -> RwSignal<u64> {
    UNCLAIMED_ESCAPE.with(|s| *s)
}

/// Ask the active editor to take keyboard focus.
pub fn refocus_editor() {
    EDITOR_REFOCUS.with(|s| s.update(|n| *n = n.wrapping_add(1)));
}

/// The signal [`refocus_editor`] bumps; the editor watches it.
pub fn editor_refocus_requests() -> RwSignal<u64> {
    EDITOR_REFOCUS.with(|s| *s)
}

/// Give `id` keyboard focus each time `open` turns true, and release it when
/// `open` turns false so a closed overlay can't keep swallowing keystrokes.
pub fn focus_while_open(id: ViewId, open: impl Fn() -> bool + 'static) {
    create_effect(move |was_open: Option<bool>| {
        let is_open = open();
        if is_open && was_open != Some(true) {
            exec_after(FOCUS_DELAY, move |_| id.request_focus());
        } else if !is_open && was_open == Some(true) {
            id.clear_focus();
            // Otherwise nothing has focus once the overlay is gone, and the
            // editor ignores typing until it is clicked.
            refocus_editor();
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

#[cfg(test)]
mod tests {
    use super::*;
    use floem::reactive::{create_rw_signal, SignalGet};

    #[test]
    fn closing_an_overlay_hands_focus_back_to_the_editor() {
        let requests = editor_refocus_requests();
        let open = create_rw_signal(false);
        focus_while_open(ViewId::new(), move || open.get());
        let before = requests.get_untracked();

        open.set(true);
        assert_eq!(
            requests.get_untracked(),
            before,
            "opening asks nothing of the editor"
        );
        open.set(false);
        assert_eq!(
            requests.get_untracked(),
            before + 1,
            "once the overlay is gone the editor must be asked to take focus"
        );
    }
}
