//! Terminal setup / teardown.
//!
//! The terminal is always restored — on normal exit, on `?` error returns
//! (via `Drop`) and on panic (via a panic hook installed before raw mode is
//! enabled) — so a crash never leaves the user's shell in raw mode on the
//! alternate screen.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::{
    cursor::Show,
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};

/// Whether we pushed keyboard-enhancement flags (must be popped on exit).
static ENHANCED_KEYS: AtomicBool = AtomicBool::new(false);
/// Whether bracketed paste was enabled (legacy Windows consoles can't).
static BRACKETED_PASTE: AtomicBool = AtomicBool::new(false);
/// Whether the TUI currently owns the terminal.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Put the terminal into TUI mode. If a step after raw mode fails, raw mode
/// is undone again before the error is returned, so the shell stays usable.
pub fn enter() -> io::Result<()> {
    enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    match set_up_screen() {
        Ok(()) => Ok(()),
        Err(e) => {
            leave();
            Err(e)
        }
    }
}

/// Everything after raw mode: the alternate screen (required), then
/// bracketed paste and kitty keyboard flags where the terminal has them.
fn set_up_screen() -> io::Result<()> {
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    // Legacy Windows consoles don't support bracketed paste; pastes then
    // arrive as key events, which `keys.rs` handles by burst detection.
    if execute!(out, EnableBracketedPaste).is_ok() {
        BRACKETED_PASTE.store(true, Ordering::SeqCst);
    }
    // Lets terminals that support the kitty protocol report Shift+Enter
    // distinctly from Enter. Unsupported terminals are left untouched.
    if matches!(supports_keyboard_enhancement(), Ok(true))
        && execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        ENHANCED_KEYS.store(true, Ordering::SeqCst);
    }
    Ok(())
}

/// Restore the terminal. Safe to call more than once.
pub fn leave() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = io::stdout();
    if ENHANCED_KEYS.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    if BRACKETED_PASTE.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, DisableBracketedPaste);
    }
    let _ = execute!(out, LeaveAlternateScreen, Show);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

/// Install a panic hook that restores the terminal before the default hook
/// prints the panic message (otherwise the message is lost on the alternate
/// screen).
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let on_ui_thread = std::thread::current().name() == Some("main");
        if on_ui_thread || !ACTIVE.load(Ordering::SeqCst) {
            leave();
            default_hook(info);
        } else {
            // A background task panicked while the UI is still running: don't
            // tear down the screen or scribble over it; the UI notices the
            // dead worker and reports it. Keep the details in the log.
            tracing::error!("background task panicked: {info}");
        }
    }));
}

/// RAII guard: restores the terminal when dropped.
pub struct TerminalGuard;

impl TerminalGuard {
    pub fn new() -> io::Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        ACTIVE.store(true, Ordering::SeqCst);
        // From here on dropping the guard restores the terminal, so a failure
        // in the rest of the setup can't strand the shell in raw mode.
        let guard = Self;
        set_up_screen()?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        leave();
    }
}

/// Temporarily hand the terminal back (e.g. to run `$EDITOR`), then resume.
pub fn suspend<T>(f: impl FnOnce() -> T) -> io::Result<T> {
    leave();
    let result = f();
    enter()?;
    Ok(result)
}
