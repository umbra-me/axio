//! Reading a prompt from somewhere other than an argument.

use super::*;

/// How long to wait for supplementary stdin before giving up on it.
///
/// Only applies when `-p` was given, so stdin is optional. Override with
/// `AXIO_STDIN_WAIT_MS` if a slow producer ever needs longer.
pub(crate) fn stdin_wait() -> std::time::Duration {
    let ms = std::env::var("AXIO_STDIN_WAIT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(500);
    std::time::Duration::from_millis(ms)
}

/// Read stdin, without ever hanging forever on a pipe that has nothing to say.
///
/// The distinction is what stdin *means* for this invocation:
///
/// * No `-p`: stdin **is** the prompt (`echo hi | axio`). Blocking until EOF is
///   correct — there is nothing else to run.
/// * With `-p`: stdin is supplementary (`cat f | axio -p "review this"`). An
///   inherited-but-idle stdin — a supervisor, a background job, a harness that
///   holds the pipe open and never writes — would otherwise block the process
///   forever with no output at all. So it is read on a side thread and given a
///   bounded wait.
pub(crate) fn read_stdin(stdin_is_tty: bool, have_prompt: bool) -> Option<String> {
    if stdin_is_tty {
        return None;
    }

    if !have_prompt {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).ok();
        return Some(buf);
    }

    let (tx, rx) = std::sync::mpsc::channel();
    // Detached deliberately: if it is still blocked at exit the process is
    // going away anyway, and there is no portable way to cancel a blocking
    // read on stdin.
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    // A timeout means nothing was coming; proceed on the prompt alone.
    rx.recv_timeout(stdin_wait()).ok()
}
