//! Keyboard handling.

use std::cell::Cell;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use phazeai_core::tools::ToolApprovalMode;

use super::actions;
use super::chat::{ChatItem, ToolStatus};
use super::AppState;
use crate::commands::{self, matching_commands};

/// How long a first Ctrl+C on an idle, empty prompt stays "armed".
const QUIT_WINDOW: Duration = Duration::from_secs(2);

/// An Enter arriving this soon after the previous key is part of a paste, not
/// a keystroke, and inserts a newline instead of sending the prompt.
///
/// Windows consoles deliver pastes as a burst of plain key events (no
/// `Event::Paste`), so without this the first pasted line would be sent on
/// its own. Keyboard auto-repeat tops out around 30 Hz (≈33 ms apart), so
/// nothing typed by hand arrives within 8 ms of the key before it. Keys are
/// timed as they are handled, so a redraw that outlasts the gap between two
/// keystrokes could still misfire; the cost is a newline instead of a send,
/// and the next Enter sends.
const PASTE_BURST: Duration = Duration::from_millis(8);

thread_local! {
    /// When the previous key event was handled (keys arrive on one thread).
    static LAST_KEY_AT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Whether an Enter handled at `now` is part of a paste burst, given when the
/// previous key was handled.
fn enter_is_pasted(previous: Option<Instant>, now: Instant) -> bool {
    previous.is_some_and(|at| now.saturating_duration_since(at) < PASTE_BURST)
}

pub(super) fn handle_paste(state: &mut AppState, text: &str) {
    if state.pending_approval.is_some() || state.session_picker.is_some() {
        return;
    }
    state.input.insert_str(text);
    state.history.reset();
    state.palette_dismissed = false;
}

/// Commands matching the current input, when the palette should be shown.
pub(super) fn palette_matches(state: &AppState) -> Vec<&'static commands::CommandSpec> {
    if state.palette_dismissed || state.pending_approval.is_some() {
        return Vec::new();
    }
    matching_commands(state.input.text())
}

fn scroll_up(state: &mut AppState, n: usize) {
    state.follow = false;
    state.scroll = state.scroll.saturating_sub(n);
}

fn scroll_down(state: &mut AppState, n: usize) {
    // `follow` is re-enabled by the renderer when we reach the bottom.
    state.scroll = state.scroll.saturating_add(n);
}

pub(super) fn handle_key(state: &mut AppState, key: KeyEvent) {
    let now = Instant::now();
    let previous_key_at = LAST_KEY_AT.replace(Some(now));
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    // Ctrl+C is handled the same everywhere: interrupt, else clear, else quit.
    if ctrl && matches!(key.code, KeyCode::Char('c')) {
        handle_ctrl_c(state);
        return;
    }
    state.quit_armed_at = None;

    if state.pending_approval.is_some() {
        handle_approval_key(state, key);
        return;
    }
    if state.session_picker.is_some() {
        handle_picker_key(state, key);
        return;
    }

    // Scrolling works in every mode.
    match key.code {
        KeyCode::PageUp => {
            let page = state.view_height.saturating_sub(2).max(1);
            scroll_up(state, page);
            return;
        }
        KeyCode::PageDown => {
            let page = state.view_height.saturating_sub(2).max(1);
            scroll_down(state, page);
            return;
        }
        KeyCode::Up if shift => {
            scroll_up(state, 1);
            return;
        }
        KeyCode::Down if shift => {
            scroll_down(state, 1);
            return;
        }
        KeyCode::Home if ctrl => {
            state.follow = false;
            state.scroll = 0;
            return;
        }
        KeyCode::End if ctrl => {
            state.follow = true;
            return;
        }
        _ => {}
    }

    // Command palette navigation.
    let matches = palette_matches(state);
    if !matches.is_empty() {
        let n = matches.len();
        match key.code {
            KeyCode::Up => {
                state.palette_index = (state.palette_index + n - 1) % n;
                return;
            }
            KeyCode::Down => {
                state.palette_index = (state.palette_index + 1) % n;
                return;
            }
            KeyCode::Tab => {
                let pick = matches[state.palette_index.min(n - 1)];
                state.input.set(format!("{} ", pick.name));
                state.palette_index = 0;
                return;
            }
            KeyCode::Esc => {
                state.palette_dismissed = true;
                return;
            }
            KeyCode::Enter if !shift && !alt => {
                let typed = state.input.text().to_string();
                let pick = matches[state.palette_index.min(n - 1)];
                if !pick.is_named(&typed) {
                    if pick.args.is_empty() {
                        state.input.set(pick.name);
                    } else {
                        state.input.set(format!("{} ", pick.name));
                        state.palette_index = 0;
                        return;
                    }
                }
                submit(state);
                return;
            }
            _ => {}
        }
    }

    match (key.code, ctrl, alt) {
        (KeyCode::Esc, _, _) => {
            if state.processing {
                state.cancel_run();
            }
        }

        // Ctrl+D: quit on an empty prompt, else delete forward.
        (KeyCode::Char('d'), true, _) => {
            if state.input.is_empty() {
                state.should_quit = true;
            } else {
                state.input.delete();
            }
        }

        // Newline: Shift+Enter (kitty protocol / Windows), Alt+Enter, Ctrl+J.
        (KeyCode::Enter, _, true) => newline(state),
        (KeyCode::Enter, _, _) if shift => newline(state),
        (KeyCode::Char('j'), true, _) => newline(state),
        (KeyCode::Enter, _, _) => {
            // A trailing backslash continues the prompt on a new line.
            let text = state.input.text();
            if enter_is_pasted(previous_key_at, now) {
                newline(state);
            } else if text.ends_with('\\') && state.input.cursor() == text.len() {
                state.input.backspace();
                newline(state);
            } else {
                submit(state);
            }
        }

        (KeyCode::Char('b'), true, _) => state.show_files = !state.show_files,
        (KeyCode::Char('o'), true, _) => actions::open_session_picker(state),
        (KeyCode::Char('l'), true, _) => {
            state.items.clear();
            state.scroll = 0;
            state.follow = true;
            state.needs_clear = true;
            state.notify(
                "Screen cleared (the model still remembers the conversation; /clear forgets it)",
            );
        }
        (KeyCode::Char('n'), true, _) => {
            actions::run_command(state, commands::CommandResult::NewConversation)
        }
        (KeyCode::Char('s'), true, _) => {
            actions::run_command(state, commands::CommandResult::SaveConversation)
        }
        (KeyCode::Char('t'), true, _) => {
            state.expand_tools = !state.expand_tools;
            state.notify(if state.expand_tools {
                "Tool output expanded"
            } else {
                "Tool output collapsed"
            });
        }
        (KeyCode::Char('e'), true, _) => actions::open_external_editor(state),
        (KeyCode::Char('v'), true, _) => actions::paste_clipboard(state),

        // Editing
        (KeyCode::Backspace, true, _) | (KeyCode::Backspace, _, true) => {
            state.input.delete_word_back()
        }
        (KeyCode::Char('w'), true, _) => state.input.delete_word_back(),
        (KeyCode::Char('h'), true, _) | (KeyCode::Backspace, _, _) => state.input.backspace(),
        (KeyCode::Delete, _, _) => state.input.delete(),
        (KeyCode::Char('u'), true, _) => state.input.kill_to_line_start(),
        (KeyCode::Char('k'), true, _) => state.input.kill_to_line_end(),
        (KeyCode::Char('a'), true, _) | (KeyCode::Home, _, _) => state.input.home(),
        (KeyCode::End, _, _) => state.input.end(),
        (KeyCode::Left, true, _) | (KeyCode::Left, _, true) | (KeyCode::Char('b'), _, true) => {
            state.input.word_left()
        }
        (KeyCode::Right, true, _) | (KeyCode::Right, _, true) | (KeyCode::Char('f'), _, true) => {
            state.input.word_right()
        }
        (KeyCode::Left, _, _) => state.input.left(),
        (KeyCode::Right, _, _) => state.input.right(),

        // Up/Down move between lines, then through history.
        (KeyCode::Up, _, _) => {
            if !state.input.up() {
                let current = state.input.text().to_string();
                if let Some(entry) = state.history.older(&current) {
                    let entry = entry.to_string();
                    state.input.set(entry);
                    state.palette_dismissed = true;
                }
            }
        }
        (KeyCode::Down, _, _) => {
            if !state.input.down() {
                if let Some(entry) = state.history.newer() {
                    state.input.set(entry);
                    state.palette_dismissed = true;
                }
            }
        }

        (KeyCode::Tab, _, _) => {
            if let Some(done) = commands::complete_command(state.input.text()) {
                state.input.set(done);
            }
        }

        // Plain characters. Ctrl+Alt+char is AltGr on Windows keyboard
        // layouts (e.g. `@`, `{` on German keyboards), so it inserts too.
        (KeyCode::Char(c), false, _) | (KeyCode::Char(c), true, true) => insert_char(state, c),
        _ => {}
    }
}

fn insert_char(state: &mut AppState, c: char) {
    state.input.insert_char(c);
    state.history.reset();
    state.palette_dismissed = false;
    state.palette_index = 0;
}

fn newline(state: &mut AppState) {
    state.input.insert_char('\n');
    state.palette_dismissed = true;
}

fn handle_ctrl_c(state: &mut AppState) {
    if state.processing {
        state.cancel_run();
        return;
    }
    if state.session_picker.is_some() {
        state.session_picker = None;
        return;
    }
    if !state.input.is_empty() {
        state.input.clear();
        state.history.reset();
        return;
    }
    match state.quit_armed_at {
        Some(at) if at.elapsed() < QUIT_WINDOW => state.should_quit = true,
        _ => state.quit_armed_at = Some(Instant::now()),
    }
}

/// Submit the prompt: run a slash command or send it to the agent.
fn submit(state: &mut AppState) {
    let text = state.input.text().to_string();
    if text.trim().is_empty() {
        return;
    }
    state.input.clear();
    state.input_scroll = 0;
    state.palette_index = 0;
    state.palette_dismissed = false;
    if !commands::is_secret_input(&text) {
        state.history.push(&text);
    } else {
        state.history.reset();
    }

    if text.starts_with('/') {
        let result = commands::handle_command(text.trim_end());
        actions::run_command(state, result);
        if text.starts_with("/cd ") {
            // The header shows the directory and branch; keep them current.
            state.refresh_git();
        }
        return;
    }
    state.last_user_input = text.clone();
    let agent_input = super::apply_mode_prefix(&state.ai_mode, &text);
    state.send_prompt(text, agent_input);
}

fn handle_approval_key(state: &mut AppState, key: KeyEvent) {
    let Some(pending) = state.pending_approval.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            pending.scroll = pending.scroll.saturating_sub(1);
            return;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            pending.scroll = pending.scroll.saturating_add(1);
            return;
        }
        KeyCode::PageUp => {
            pending.scroll = pending.scroll.saturating_sub(10);
            return;
        }
        KeyCode::PageDown => {
            pending.scroll = pending.scroll.saturating_add(10);
            return;
        }
        _ => {}
    }

    let decision = match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Some((true, false)),
        KeyCode::Char('a') | KeyCode::Char('A') => Some((true, true)),
        KeyCode::Char('n') | KeyCode::Char('N') => Some((false, false)),
        KeyCode::Esc => {
            state.cancel_run();
            return;
        }
        _ => None,
    };
    let Some((approved, always)) = decision else {
        return;
    };
    let Some(pending) = state.pending_approval.take() else {
        return;
    };
    let req = pending.request;
    if always {
        if let Ok(mut p) = state.policy.lock() {
            p.allow(&req.name, &req.params);
        }
        let scope = crate::tool_view::ApprovalPolicy::describe(&req.name, &req.params);
        state.notify(format!("Allowed {scope} for this session"));
    }
    if approved {
        if let Some(ChatItem::Tool(card)) = state
            .items
            .iter_mut()
            .rev()
            .find(|i| matches!(i, ChatItem::Tool(c) if c.name == req.name && c.status == ToolStatus::Pending))
        {
            card.status = ToolStatus::Running;
        }
    }
    let _ = req.respond.send(approved);
}

fn handle_picker_key(state: &mut AppState, key: KeyEvent) {
    let Some(picker) = state.session_picker.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => state.session_picker = None,
        KeyCode::Up | KeyCode::Char('k') => {
            picker.index = picker.index.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if picker.index + 1 < picker.list.len() {
                picker.index += 1;
            }
        }
        KeyCode::Enter => {
            let id = picker.list.get(picker.index).map(|m| m.id.clone());
            state.session_picker = None;
            if let Some(id) = id {
                actions::run_command(state, commands::CommandResult::LoadConversation(id));
            }
        }
        _ => {}
    }
}

/// Label for the current approval mode (status bar).
pub(super) fn approval_label(mode: &ToolApprovalMode) -> &'static str {
    match mode {
        ToolApprovalMode::AutoApprove => "auto-approve",
        ToolApprovalMode::AlwaysAsk => "ask",
        ToolApprovalMode::AskOnce => "ask-once",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_within_a_paste_burst_inserts_a_newline() {
        let t0 = Instant::now();
        // Pasted lines arrive microseconds apart.
        assert!(enter_is_pasted(Some(t0), t0));
        assert!(enter_is_pasted(Some(t0), t0 + Duration::from_millis(2)));
        // Typing, even with auto-repeat, is much slower than the threshold.
        assert!(!enter_is_pasted(Some(t0), t0 + PASTE_BURST));
        assert!(!enter_is_pasted(Some(t0), t0 + Duration::from_millis(40)));
        assert!(!enter_is_pasted(Some(t0), t0 + Duration::from_secs(3)));
        // The first key ever is a keystroke.
        assert!(!enter_is_pasted(None, t0));
    }
}
