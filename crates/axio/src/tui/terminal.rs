//! Getting the terminal into raw mode and, more importantly, out of it.
//!
//! Split from `mod` because that file reached the width limit. It belongs
//! together anyway: every path that leaves — a clean exit, an error, a panic —
//! has to undo exactly what starting up did, and having the two in one file is
//! how they stay in step.

use super::*;

/// Restore the terminal even when the process is dying badly.
///
/// A panic inside a raw-mode program leaves the user with no echo and no line
/// discipline — they have to type `reset` blind. The hook runs before the
/// default one so the message is readable when it arrives.
pub(super) fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        previous(info);
    }));
}

pub(super) fn restore_terminal() -> std::io::Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    let mut out = std::io::stdout();
    crossterm::execute!(
        out,
        crossterm::event::DisableBracketedPaste,
        crossterm::cursor::Show
    )?;
    out.flush()
}
