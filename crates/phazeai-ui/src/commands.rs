/// Global IDE keyboard shortcut commands.
///
/// This module provides a canonical enum of all globally-scoped keyboard
/// shortcuts plus the `GlobalCommandState` struct and `execute_command`
/// function that apply a command to the relevant reactive signals.
///
/// **Design contract**
/// - `match_global_shortcut` is a pure function — no side-effects.
/// - `execute_command` mutates only the signals stored in `GlobalCommandState`.
/// - Any site that uses `on_event_stop` and receives keyboard events (e.g. the
///   terminal canvas) must check `match_global_shortcut` first.  On a match it
///   calls `execute_command` and returns — this ensures the shortcut is handled
///   identically regardless of which widget holds focus.
/// - The root key handler in `app.rs` uses the same path so the logic never
///   drifts.
use std::path::PathBuf;
use std::rc::Rc;

use floem::keyboard::{Key, Modifiers};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate};

// ── IdeCommand enum ───────────────────────────────────────────────────────────

/// A global IDE action triggered by a keyboard shortcut.
///
/// Variants cover every shortcut that must work identically from any focused
/// widget (editor, terminal, explorer, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdeCommand {
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
    /// Ctrl+Shift+Z — toggle zen (distraction-free) mode.
    ToggleZenMode,
    /// Ctrl+Alt+\ — toggle the vertical split editor pane.
    ToggleSplitEditor,
}

/// Back-compat type alias — code that imported `GlobalShortcut` still compiles.
pub type GlobalShortcut = IdeCommand;

// ── match_global_shortcut ─────────────────────────────────────────────────────

/// Inspect a Floem `KeyEvent` and return the matching `IdeCommand`, if any.
///
/// This is a pure function with no side-effects.  Callers apply the returned
/// command to `GlobalCommandState` via `execute_command`.
pub fn match_global_shortcut(ke: &floem::keyboard::KeyEvent) -> Option<IdeCommand> {
    let ctrl = ke.modifiers.contains(Modifiers::CONTROL);
    let shift = ke.modifiers.contains(Modifiers::SHIFT);
    let alt = ke.modifiers.contains(Modifiers::ALT);

    if !ctrl {
        return None;
    }

    if let Key::Character(ref ch) = ke.key.logical_key {
        let s = ch.as_str();

        // Ctrl+Shift+P — must be checked before Ctrl+P.
        if shift && !alt && (s == "p" || s == "P") {
            return Some(IdeCommand::ToggleCommandPalette);
        }

        // Ctrl+Shift+Z — zen mode.
        if shift && !alt && (s == "z" || s == "Z") {
            return Some(IdeCommand::ToggleZenMode);
        }

        // Ctrl+Alt+\ — split editor (vertical pane).
        if alt && !shift && s == "\\" {
            return Some(IdeCommand::ToggleSplitEditor);
        }

        if !shift && !alt {
            return match s {
                "b" | "B" => Some(IdeCommand::ToggleLeftPanel),
                "j" | "J" => Some(IdeCommand::ToggleBottomPanel),
                "\\" => Some(IdeCommand::ToggleRightPanel),
                "p" | "P" => Some(IdeCommand::ToggleFilePicker),
                _ => None,
            };
        }
    }

    None
}

// ── GlobalCommandState ────────────────────────────────────────────────────────

/// Minimal reactive state needed to execute any `IdeCommand`.
///
/// Both `app.rs` (via `IdeState::as_global_command_state()`) and `terminal.rs`
/// use this to call `execute_command`.  This avoids circular imports because
/// `commands.rs` does not import `IdeState`.
///
/// All fields are `Copy` signals or `Clone` wrappers, so `GlobalCommandState`
/// itself is `Clone` — callers can clone and pass it into closures freely.
#[derive(Clone)]
pub struct GlobalCommandState {
    pub show_left_panel: RwSignal<bool>,
    /// Width of the left panel (260.0 open, 0.0 closed).  Updated together with
    /// `show_left_panel` so the panel collapses/expands correctly.
    pub left_panel_width: RwSignal<f64>,
    pub show_bottom_panel: RwSignal<bool>,
    pub show_right_panel: RwSignal<bool>,
    pub file_picker_open: RwSignal<bool>,
    /// Cleared when the picker is opened so stale queries don't show.
    pub file_picker_query: RwSignal<String>,
    pub command_palette_open: RwSignal<bool>,
    pub zen_mode: RwSignal<bool>,
    pub split_editor: RwSignal<bool>,
    /// Active file in the primary editor — used to seed the split pane on first open.
    pub primary_open_file: RwSignal<Option<PathBuf>>,
    /// Active file in the split editor pane.
    pub split_open_file: RwSignal<Option<PathBuf>>,
    /// Toast signal — Some(msg) while a toast is shown, cleared after 3 s.
    pub status_toast: RwSignal<Option<String>>,
    /// Called after any panel-visibility change so the caller can persist state
    /// to `session.toml`.  Pass `Rc::new(|| {})` when persistence is not needed
    /// (e.g. from the terminal handler).
    pub on_persist: Rc<dyn Fn()>,
}

// ── execute_command ───────────────────────────────────────────────────────────

/// Apply `cmd` to the provided reactive `state`.
///
/// This is the single authoritative implementation of every `IdeCommand`.
/// Both the root window key handler (`app.rs`) and any focusable leaf widget
/// that swallows key events (e.g. the PTY terminal) must call this function
/// after receiving a matching `IdeCommand` from `match_global_shortcut`.
pub fn execute_command(cmd: IdeCommand, state: &GlobalCommandState) {
    match cmd {
        IdeCommand::ToggleLeftPanel => {
            state.show_left_panel.update(|v| *v = !*v);
            let now_open = state.show_left_panel.get();
            let new_w = if now_open { 260.0 } else { 0.0 };
            state.left_panel_width.set(new_w);
            (state.on_persist)();
        }
        IdeCommand::ToggleBottomPanel => {
            state.show_bottom_panel.update(|v| *v = !*v);
            (state.on_persist)();
        }
        IdeCommand::ToggleRightPanel => {
            state.show_right_panel.update(|v| *v = !*v);
        }
        IdeCommand::ToggleFilePicker => {
            let was_open = state.file_picker_open.get();
            state.file_picker_open.set(!was_open);
            if !was_open {
                // Opening — clear stale query so results are fresh.
                state.file_picker_query.set(String::new());
            }
        }
        IdeCommand::ToggleCommandPalette => {
            state.command_palette_open.update(|v| *v = !*v);
        }
        IdeCommand::ToggleZenMode => {
            state.zen_mode.update(|v| *v = !*v);
            let on = state.zen_mode.get();
            let msg = if on {
                "Zen mode on — Ctrl+Shift+Z to exit"
            } else {
                "Zen mode off"
            };
            // Inline toast: set message; spawn thread to clear after 3 s.
            state.status_toast.set(Some(msg.to_string()));
            let toast_sig = state.status_toast;
            use floem::ext_event::create_ext_action;
            use floem::reactive::Scope;
            let dismiss = create_ext_action(Scope::current(), move |_: ()| {
                toast_sig.set(None);
            });
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                dismiss(());
            });
        }
        IdeCommand::ToggleSplitEditor => {
            let was_open = state.split_editor.get();
            state.split_editor.update(|v| *v = !*v);
            if !was_open && state.split_open_file.get().is_none() {
                // Seed the split pane with the currently active file.
                state.split_open_file.set(state.primary_open_file.get());
            }
        }
    }
}
