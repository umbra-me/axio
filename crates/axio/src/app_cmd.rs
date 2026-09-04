//! `axio app` — open the desktop surface.
//!
//! The window is not a mode of this binary. It is `axio-app`, a separate
//! executable: its webview runtime must never reach a `cargo install axio`,
//! and its crate already depends on this one for the wiring a session shares,
//! so linking the other way would be a cycle. This command therefore does the
//! one thing a command line is good at here — find the binary that ships
//! beside this one, start it, and leave.
//!
//! Beside this binary first, then `PATH`, because cargo puts both binaries in
//! the same directory — `target/release` from a build, the install bin
//! directory from an install — and because the same two-step search is how the
//! window finds the agents it hosts. It is not a fixed path: a fixed path
//! breaks the moment the two binaries were installed by different hands.
//!
//! Nothing is waited on. The window outlives this process by design, and the
//! single-instance plugin answers a second launch by bringing the first window
//! forward, so `axio app` is idempotent rather than a way to end up with two
//! supervisors over one index.
//!
//! Only something that could be run is tried: a directory or a stale,
//! non-executable file wearing the name must not end the search while a real
//! binary sits further down `PATH`. That is the same rule the harness lookup
//! applies, and for the same reason.

use std::io::ErrorKind;
// `process_group` lives in a trait on this platform only; a cfg'd import keeps
// the headless Windows build from meeting it.
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

/// The desktop binary's file name on this platform.
const APP_BINARY: &str = if cfg!(windows) {
    "axio-app.exe"
} else {
    "axio-app"
};

/// Find the desktop binary, start it, and say which one won.
pub(crate) fn app_command() -> u8 {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(APP_BINARY));
    }
    if let Ok(path) = std::env::var("PATH") {
        candidates.extend(
            std::env::split_paths(&path)
                .filter(|dir| !dir.as_os_str().is_empty())
                .map(|dir| dir.join(APP_BINARY)),
        );
    }

    for candidate in candidates.into_iter().filter(|c| is_executable(c)) {
        let mut cmd = Command::new(&candidate);
        // Its own process group, so a signal typed at this terminal belongs to
        // this terminal. On Windows nothing is needed: a GUI-subsystem binary
        // never attaches to the console that started it.
        #[cfg(unix)]
        cmd.process_group(0);
        match cmd.spawn() {
            // Started. Standard streams are inherited, so a window that fails
            // as it opens still explains itself on this terminal. The cost is
            // that whatever the window writes later lands here too, after the
            // prompt has come back; a release build writes nothing on the
            // happy path, and a debug build is meant to be watched.
            Ok(_) => return 0,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                eprintln!(
                    "axio: found {} but could not start it: {e}",
                    candidate.display()
                );
                return 1;
            }
        }
    }

    eprintln!(
        "axio: no {APP_BINARY} beside this binary or on PATH.\n\
         The desktop surface is its own binary. From the repository:\n\
         \x20   npm --prefix crates/axio-app/ui run build\n\
         \x20   cargo build --release -p axio-app --features app"
    );
    1
}

/// A regular file this process could run. On Windows an executable is what
/// `CreateProcess` accepts, and there is no mode bit to consult.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
