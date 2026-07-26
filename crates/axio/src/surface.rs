//! Which surface runs, and how the process stops.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tokio_util::sync::CancellationToken;

use crate::render::compose_prompt;

/// Exit codes for the signals we handle. These are the shell's convention
/// (128 + signal number), and scripts depend on them.
pub const EXIT_SIGINT: u8 = 130;
/// Unix only: Windows has no SIGTERM to receive, so naming one there would be
/// a constant that can never be returned.
#[cfg(unix)]
pub const EXIT_SIGTERM: u8 = 143;
#[cfg(unix)]
pub const EXIT_SIGHUP: u8 = 129;

/// A second interrupt inside this window stops waiting for a clean shutdown.
const IMPATIENT_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Surface {
    /// Run one turn and exit.
    OneShot(String),
    /// Interactive.
    Tui,
    /// Nothing to do; print usage.
    Nothing,
}

impl Surface {
    /// Decide from how the process was invoked.
    ///
    /// The order matters, and the stdout check is deliberately absent: a caller
    /// running `axio -p x | tee log` has a terminal stdin and a piped stdout,
    /// and still wants one shot. Only stdin decides whether a human is there to
    /// type.
    pub fn select(prompt: Option<&str>, piped_stdin: Option<&str>, stdin_is_tty: bool) -> Surface {
        match compose_prompt(prompt, piped_stdin) {
            Some(text) => Surface::OneShot(text),
            // No prompt and a human at the keyboard: interactive.
            None if stdin_is_tty => Surface::Tui,
            None => Surface::Nothing,
        }
    }
}

/// Watch for the signals that mean "stop", and report which one arrived.
///
/// The returned code is `0` until a signal lands. The first one cancels the
/// turn so it can flush a partial answer and record that it was cut short; a
/// second interrupt within two seconds gives up on that and exits immediately,
/// because a user pressing it twice has stopped caring about a clean shutdown.
pub fn spawn_signal_watcher(cancel: CancellationToken) -> Arc<AtomicU8> {
    let code = Arc::new(AtomicU8::new(0));
    let out = code.clone();

    tokio::spawn(async move {
        let mut first_interrupt: Option<std::time::Instant> = None;

        loop {
            let signal = next_signal().await;
            code.store(signal, Ordering::SeqCst);

            if signal == EXIT_SIGINT {
                if let Some(at) = first_interrupt
                    && at.elapsed() < IMPATIENT_WINDOW
                {
                    // Asked twice. Stop waiting.
                    std::process::exit(EXIT_SIGINT as i32);
                }
                first_interrupt = Some(std::time::Instant::now());
                cancel.cancel();
                continue;
            }

            // SIGTERM and SIGHUP are not a request for a second chance.
            cancel.cancel();
            std::process::exit(signal as i32);
        }
    });

    out
}

#[cfg(unix)]
async fn next_signal() -> u8 {
    use tokio::signal::unix::{SignalKind, signal};

    let mut term = signal(SignalKind::terminate()).expect("failed to listen for SIGTERM");
    let mut hup = signal(SignalKind::hangup()).expect("failed to listen for SIGHUP");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => EXIT_SIGINT,
        _ = term.recv() => EXIT_SIGTERM,
        _ = hup.recv() => EXIT_SIGHUP,
    }
}

#[cfg(not(unix))]
async fn next_signal() -> u8 {
    // Windows has no SIGTERM or SIGHUP to listen for; Ctrl-C is the whole
    // surface, and Ctrl-Break arrives as the same notification.
    let _ = tokio::signal::ctrl_c().await;
    EXIT_SIGINT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_flag_runs_one_shot_even_at_a_terminal() {
        assert_eq!(
            Surface::select(Some("hi"), None, true),
            Surface::OneShot("hi".into())
        );
    }

    #[test]
    fn piped_stdin_alone_runs_one_shot() {
        assert_eq!(
            Surface::select(None, Some("summarise this"), false),
            Surface::OneShot("summarise this".into())
        );
    }

    #[test]
    fn a_prompt_and_piped_stdin_are_combined_not_chosen_between() {
        match Surface::select(Some("review this"), Some("fn main() {}"), false) {
            Surface::OneShot(text) => {
                assert!(text.contains("review this"));
                assert!(text.contains("fn main() {}"));
                assert!(text.contains("<stdin>"));
            }
            other => panic!("expected one shot, got {other:?}"),
        }
    }

    #[test]
    fn a_bare_invocation_at_a_terminal_is_interactive() {
        assert_eq!(Surface::select(None, None, true), Surface::Tui);
    }

    #[test]
    fn a_bare_invocation_with_nothing_piped_in_has_nothing_to_do() {
        // Closed stdin, no prompt: printing usage beats hanging on a read.
        assert_eq!(Surface::select(None, Some(""), false), Surface::Nothing);
    }

    #[test]
    fn stdout_being_piped_does_not_launch_the_interactive_surface() {
        // `axio -p x | tee log` has a terminal stdin and a piped stdout. Only
        // stdin decides whether a human is there to type.
        assert_eq!(
            Surface::select(Some("x"), None, true),
            Surface::OneShot("x".into())
        );
    }

    #[test]
    fn signal_exit_codes_follow_the_shell_convention() {
        assert_eq!(EXIT_SIGINT, 130);
        #[cfg(unix)]
        {
            assert_eq!(EXIT_SIGTERM, 143);
            assert_eq!(EXIT_SIGHUP, 129);
        }
    }
}
