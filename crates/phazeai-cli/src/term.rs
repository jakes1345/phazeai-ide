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
/// Whether the TUI currently owns the terminal.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Put the terminal into TUI mode.
pub fn enter() -> io::Result<()> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
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
    ACTIVE.store(true, Ordering::SeqCst);
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
    let _ = execute!(out, DisableBracketedPaste, LeaveAlternateScreen, Show);
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
        enter()?;
        Ok(Self)
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
