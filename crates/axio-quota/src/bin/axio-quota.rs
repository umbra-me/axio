//! `axio-quota` — the desktop app: tray icon, flyout and window.
//!
//! Built only with `--features app`. The work is in `axio_quota::app`.
//!
//! Not to be confused with `axio quota`, the subcommand, which prints the same numbers to
//! a terminal and shares nothing but the probe layer.

// No console behind the window in a release build; debug builds keep theirs for panics.
#![cfg_attr(
    all(feature = "app", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(feature = "app")]
fn main() -> std::process::ExitCode {
    match axio_quota::app::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("axio-quota: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "app"))]
fn main() -> std::process::ExitCode {
    eprintln!("axio-quota: needs --features app");
    std::process::ExitCode::FAILURE
}
