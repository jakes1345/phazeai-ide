/// Global IDE keyboard shortcut commands.
///
/// This module provides a canonical enum of all globally-scoped keyboard
/// shortcuts and a pure matching function that maps a Floem `KeyEvent` to the
/// corresponding `GlobalShortcut` variant.  Both the root key handler in
/// `app.rs` and the terminal PTY handler in `terminal.rs` call
/// `match_global_shortcut` so that the mapping never drifts between the two
/// sites.
use floem::keyboard::{Key, Modifiers};

/// A global IDE action triggered by a keyboard shortcut.
///
/// These commands must be intercepted at every focusable leaf that uses
/// `on_event_stop` (e.g. the PTY terminal) so that they are not accidentally
/// swallowed before reaching the root handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalShortcut {
    /// Ctrl+B — toggle the left (explorer/sidebar) panel.
    ToggleLeftPanel,
    /// Ctrl+J — toggle the bottom (terminal/output) panel.
    ToggleBottomPanel,
    /// Ctrl+\ — toggle the right (chat/AI) panel.
    ToggleRightPanel,
    /// Ctrl+P — open/close the quick file-picker overlay.
    ToggleFilePicker,
    /// Ctrl+Shift+P — open/close the command palette overlay.
    ToggleCommandPalette,
}

/// Inspect a Floem `KeyEvent` and return the matching `GlobalShortcut`, if any.
///
/// This is a pure function with no side-effects.  Callers apply the returned
/// command to their local state.
pub fn match_global_shortcut(ke: &floem::keyboard::KeyEvent) -> Option<GlobalShortcut> {
    let ctrl = ke.modifiers.contains(Modifiers::CONTROL);
    let shift = ke.modifiers.contains(Modifiers::SHIFT);
    let alt = ke.modifiers.contains(Modifiers::ALT);

    // Only match Ctrl-based shortcuts; ignore pure Alt or bare keys.
    if !ctrl {
        return None;
    }

    // Named-key shortcuts (none currently for the global set, but guard anyway).
    if let Key::Named(_named) = &ke.key.logical_key {
        // No named-key global shortcuts at this time.
        return None;
    }

    if let Key::Character(ref ch) = ke.key.logical_key {
        let s = ch.as_str();

        // Ctrl+Shift+P — must be checked before Ctrl+P.
        if ctrl && shift && !alt && (s == "p" || s == "P") {
            return Some(GlobalShortcut::ToggleCommandPalette);
        }

        if ctrl && !shift && !alt {
            return match s {
                "b" | "B" => Some(GlobalShortcut::ToggleLeftPanel),
                "j" | "J" => Some(GlobalShortcut::ToggleBottomPanel),
                "\\" => Some(GlobalShortcut::ToggleRightPanel),
                "p" | "P" => Some(GlobalShortcut::ToggleFilePicker),
                _ => None,
            };
        }
    }

    None
}
