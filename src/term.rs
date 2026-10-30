//! Terminal setup and teardown: raw mode, alternate screen, bracketed paste,
//! optional kitty keyboard flags, and a panic hook that restores everything.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::{cursor, execute};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static KITTY: AtomicBool = AtomicBool::new(false);

/// Holds the terminal in editor mode; dropping it always restores the terminal.
pub struct TerminalGuard {
    _private: (),
}

impl TerminalGuard {
    /// Enters raw mode, the alternate screen and bracketed paste. Unless
    /// `no_kitty`, pushes `DISAMBIGUATE_ESCAPE_CODES` when the terminal supports it.
    pub fn enter(no_kitty: bool) -> io::Result<TerminalGuard> {
        enable_raw_mode()?;
        ACTIVE.store(true, Ordering::SeqCst);
        let guard = TerminalGuard { _private: () };
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
        if !no_kitty && terminal::supports_keyboard_enhancement().unwrap_or(false) {
            execute!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
            KITTY.store(true, Ordering::SeqCst);
        }
        Ok(guard)
    }

    /// Whether the kitty keyboard flags were pushed.
    pub fn kitty_enabled() -> bool {
        KITTY.load(Ordering::SeqCst)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Leaves everything `enter` set up. Idempotent, safe to call from a panic hook.
pub fn restore() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = io::stdout();
    if KITTY.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = out.flush();
    let _ = disable_raw_mode();
}

/// Restores the terminal before printing the panic.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        default(info);
    }));
}
