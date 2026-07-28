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
    // Not `cmd /c start`. `start` is a shell builtin, so reaching it means
    // going through cmd's parser — and cmd reads `&` as a command separator.
    // A URL is mostly `&`: the browser received everything up to the first one
    // and the authorization server refused it as missing half its parameters,
    // which is a failure with no visible connection to the opener at all.
    //
    // `rundll32 url.dll,FileProtocolHandler` hands the URL to the registered
    // protocol handler with no shell in between, so the argument arrives as
    // one argument whatever is in it.
    let mut command = Command::new("rundll32.exe");
    command.args(["url.dll,FileProtocolHandler", url]);
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
            "rundll32.exe"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(program, expected);
    }

    /// The regression, and it cost two rounds of browser screenshots to find:
    /// routed through `cmd /c start`, everything after the first `&` was a
    /// separate command, so the authorization server saw a URL with one
    /// parameter and refused it for reasons that named nothing to do with the
    /// opener. A URL must arrive as exactly one argument, ampersands included.
    #[test]
    fn the_whole_url_is_one_argument() {
        let url = "https://example.invalid/auth?response_type=code&scope=a%20b&state=xyz";
        let command = opener(url);
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert!(
            args.iter().any(|a| a == url),
            "the URL must survive whole: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.contains("cmd") || a == "/c"),
            "no shell may parse this: {args:?}"
        );
    }
}
