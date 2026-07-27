//! Handing a URL to whatever the desktop uses to open one.
//!
//! Here rather than beside the OAuth flow that needs it, because spawning a
//! process is this crate's job and putting one exception in the transport crate
//! is how that stops being true.
//!
//! Opening is best-effort by design. A machine reached over SSH, a container,
//! or a session with no desktop has nothing to open a URL with, and that is not
//! a failure of the sign-in — the URL still works, pasted into a browser
//! somewhere else. The caller is told whether it worked so it can print the URL
//! when it did not.

use std::process::{Command, Stdio};

/// Ask the desktop to open `url`. Returns whether the opener could be launched.
///
/// Launched, not succeeded: these commands hand off to the desktop and exit,
/// so a zero status means the request was accepted, never that a window
/// appeared. Waiting for more than that would hang on any machine where the
/// browser is the foreground process.
pub fn open(url: &str) -> bool {
    // A URL from an authorization flow is built by us, but this is a shell-
    // adjacent surface and one day something else will call it. Anything that
    // is not an http(s) URL is refused rather than passed to a shell.
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return false;
    }

    let mut command = opener(url);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    match command.spawn() {
        Ok(mut child) => {
            // Reaped so the process table does not keep a zombie for the rest
            // of the session; the status is not what is being asked about.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "windows")]
fn opener(url: &str) -> Command {
    // Through `cmd /c start`, whose first quoted argument is the window title
    // rather than the target — omitting the empty title makes a URL containing
    // a space open a window called that and nothing else. `start` is a shell
    // builtin, so there is no executable to call directly.
    let mut command = Command::new("cmd");
    command.args(["/c", "start", "", url]);
    command
}

#[cfg(target_os = "macos")]
fn opener(url: &str) -> Command {
    let mut command = Command::new("open");
    command.arg(url);
    command
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn opener(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard, not the opening: a caller one day passing something that is
    /// not a URL must not reach a shell with it.
    #[test]
    fn only_http_urls_are_opened() {
        assert!(!open("file:///etc/passwd"));
        assert!(!open("javascript:alert(1)"));
        assert!(!open("& calc.exe"));
        assert!(!open(""));
    }

    #[test]
    fn the_opener_names_the_platforms_own_command() {
        let command = opener("https://example.invalid/");
        let program = command.get_program().to_string_lossy().into_owned();
        let expected = if cfg!(target_os = "windows") {
            "cmd"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(program, expected);
    }

    /// On Windows the empty title is load-bearing: without it a URL containing
    /// a space is read as the window title and nothing opens.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_windows_opener_passes_an_empty_title_first() {
        let command = opener("https://example.invalid/a%20b");
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "/c");
        assert_eq!(args[1], "start");
        assert_eq!(args[2], "", "the empty title must come before the URL");
        assert_eq!(args[3], "https://example.invalid/a%20b");
    }
}
