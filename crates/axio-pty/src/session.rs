//! A hosted agent, and the terminal axio owns on its behalf.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::buffer::Ring;
use crate::harness::{Harness, child_env};

#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("{0} is not installed, or is not on PATH")]
    NotInstalled(&'static str),
    #[error("could not open a terminal: {0}")]
    Open(String),
    #[error("could not start {harness}: {message}")]
    Spawn {
        harness: &'static str,
        message: String,
    },
    #[error("that session is not running")]
    Gone,
    #[error("{0}")]
    Io(String),
}

/// What a hosted session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessStatus {
    Running,
    Exited(i32),
    /// It stopped in a way that produced no status — killed, or never started.
    Ended,
}

/// One hosted agent in one pseudo-terminal.
pub struct HarnessSession {
    pub harness: Harness,
    pub cwd: std::path::PathBuf,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Behind a lock because `MasterPty` is `Send` but not `Sync`, and a
    /// desktop surface shares its state across command threads — without this
    /// the whole application state stops being shareable for the sake of one
    /// resize call.
    ///
    /// `Option` so the destructor can take it out and drop it elsewhere. See
    /// the `Drop` impl: closing it can block forever, and it must not do that
    /// on whatever thread happened to release the session.
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    output: Arc<Mutex<Ring>>,
    /// Signalled whenever the ring advances.
    ///
    /// A notification, never the bytes. A reader still asks by cursor, which is
    /// what keeps a reload correct — the signal only removes the need to ask on
    /// a timer. Pushing the bytes themselves would mean anything not listening
    /// at that instant lost them, which is the failure the cursor exists for.
    wrote: Arc<tokio::sync::Notify>,
    status: Arc<Mutex<HarnessStatus>>,
    /// The direct child. On Windows this is `cmd.exe`, which is why killing it
    /// is not the same as killing what it started — see [`HarnessSession::kill`].
    pid: Option<u32>,
}

impl HarnessSession {
    /// Start a harness in `cwd`.
    ///
    /// On Windows the command goes through `cmd.exe /d /s /c`, and that is not
    /// laziness: agent CLIs installed by npm are `.cmd` shims, which
    /// `CreateProcess` will not execute. `/d` skips AutoRun, so a registry key
    /// somebody set years ago does not run first.
    pub fn spawn(
        harness: Harness,
        cwd: &std::path::Path,
        args: &[String],
    ) -> Result<Self, PtyError> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: 32,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Open(e.to_string()))?;

        let mut command = command_for(harness);
        command.cwd(cwd);
        for arg in args {
            command.arg(arg);
        }
        for (key, value) in child_env() {
            command.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| PtyError::Spawn {
                harness: harness.executable(),
                message: e.to_string(),
            })?;
        // Dropped immediately: while this end of the pair is open the reader
        // never sees EOF, so a harness that exits leaves a thread waiting on a
        // terminal nobody is attached to.
        drop(pair.slave);

        let pid = child.process_id();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Io(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Io(e.to_string()))?;

        let output = Arc::new(Mutex::new(Ring::new()));
        let status = Arc::new(Mutex::new(HarnessStatus::Running));
        let wrote = Arc::new(tokio::sync::Notify::new());
        pump(reader, Arc::clone(&output), Arc::clone(&wrote));
        watch(child, Arc::clone(&status), Arc::clone(&wrote));

        Ok(Self {
            harness,
            cwd: cwd.to_path_buf(),
            writer: Arc::new(Mutex::new(writer)),
            master: Mutex::new(Some(pair.master)),
            output,
            wrote,
            status,
            pid,
        })
    }

    /// Resolves the next time this terminal writes anything, or ends.
    ///
    /// For a surface that would otherwise ask on a timer. `notify_waiters` only
    /// wakes whoever is already waiting, so a caller must hold this future
    /// across its read rather than creating one afterwards — otherwise output
    /// that lands in the gap wakes nobody. The cursor makes that survivable
    /// rather than fatal, which is exactly why the signal carries no bytes.
    pub fn wrote(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.wrote)
    }

    /// Everything written after `from`, and the cursor to ask with next time.
    pub fn read_from(&self, from: u64) -> (Vec<u8>, u64) {
        self.output
            .lock()
            .expect("the output lock is never held across a blocking call")
            .read_from(from)
    }

    pub fn status(&self) -> HarnessStatus {
        *self
            .status
            .lock()
            .expect("the status lock is never held across a blocking call")
    }

    /// Send keystrokes.
    ///
    /// Exactly the bytes given, and callers submitting a line should send the
    /// text and its carriage return as **two** writes. A provider that treats
    /// one combined chunk as a paste leaves the text sitting on its prompt
    /// without submitting it, which looks like the agent ignoring you.
    pub fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| PtyError::Io("the terminal writer is unavailable".to_owned()))?;
        writer
            .write_all(data)
            .map_err(|e| PtyError::Io(e.to_string()))?;
        writer.flush().map_err(|e| PtyError::Io(e.to_string()))
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), PtyError> {
        let guard = self
            .master
            .lock()
            .map_err(|_| PtyError::Io("the terminal is unavailable".to_owned()))?;
        guard
            .as_ref()
            .ok_or(PtyError::Gone)?
            .resize(PtySize {
                rows: rows.clamp(8, 200),
                cols: cols.clamp(20, 400),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Io(e.to_string()))
    }

    /// Stop it, and everything it started.
    ///
    /// Killing the direct child is not enough on either platform: on Windows it
    /// is `cmd.exe` and the agent is its child; on unix an agent that spawned a
    /// build leaves the build running. The same two answers `axio-tools` already
    /// uses — a process group signal, or `taskkill /T` — rather than a Job
    /// Object, which would mean unsafe FFI for a result this reaches without it.
    pub async fn kill(&self) -> Result<(), PtyError> {
        let Some(pid) = self.pid else {
            return Ok(());
        };
        // Off the async worker: it shells out and waits, which is exactly what
        // `spawn_blocking` exists for, and it shares one implementation with
        // the destructor so the two cannot drift.
        let _ = tokio::task::spawn_blocking(move || kill_tree_blocking(pid)).await;
        *self
            .status
            .lock()
            .expect("the status lock is never held across a blocking call") = HarnessStatus::Ended;
        Ok(())
    }
}

#[cfg(test)]
impl HarnessSession {
    /// Spawn an arbitrary program, for tests only.
    ///
    /// `#[cfg(test)]`, so it does not exist in a real build: the allowlist is
    /// the security property this crate has, and a public escape hatch would be
    /// the whole of it. What it buys is proving the pump, the ring and the kill
    /// actually work against a real pseudo-terminal rather than only compiling.
    pub(crate) fn spawn_raw(program: &str, args: &[&str]) -> Result<Self, PtyError> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Open(e.to_string()))?;
        let mut command = CommandBuilder::new(program);
        for arg in args {
            command.arg(arg);
        }
        command.cwd(std::env::temp_dir());
        for (key, value) in child_env() {
            command.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| PtyError::Spawn {
                harness: "test",
                message: e.to_string(),
            })?;
        drop(pair.slave);
        let pid = child.process_id();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Io(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Io(e.to_string()))?;
        let output = Arc::new(Mutex::new(Ring::new()));
        let status = Arc::new(Mutex::new(HarnessStatus::Running));
        let wrote = Arc::new(tokio::sync::Notify::new());
        pump(reader, Arc::clone(&output), Arc::clone(&wrote));
        watch(child, Arc::clone(&status), Arc::clone(&wrote));
        Ok(Self {
            harness: Harness::Axio,
            cwd: std::env::temp_dir(),
            writer: Arc::new(Mutex::new(writer)),
            master: Mutex::new(Some(pair.master)),
            output,
            wrote,
            status,
            pid,
        })
    }
}

/// Kill the tree, blocking, without a runtime.
///
/// Used by both [`HarnessSession::kill`] and `Drop`. The second is why it is
/// synchronous: a destructor cannot await, and it is the destructor that has to
/// win a race against a deadlock.
fn kill_tree_blocking(pid: u32) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Close the terminal somewhere nothing is waiting.
///
/// Closing a ConPTY blocks until its output pipe drains. The pump thread is
/// blocked in `read()` on that pipe, and that `read()` does not return until
/// the terminal closes. Each waits for the other and the thread stops — and
/// **the child exiting does not break it**, because the pty stays open after
/// its process is gone. A probe pinned this precisely: every step up to the
/// drop completed, and the drop never returned.
///
/// So two things happen here. The tree is killed, which is right regardless.
/// And the master is moved onto a detached thread to be dropped there, so if
/// the close does block it blocks something nobody is waiting on rather than
/// whatever thread happened to release the session — a webview command, or the
/// window's own teardown.
///
/// This is not only about tests. Dropping a session is what the application
/// does when somebody closes a terminal, and doing it inline would hang the
/// window on the click.
impl Drop for HarnessSession {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            kill_tree_blocking(pid);
        }
        let master = self.master.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(master) = master {
            std::thread::spawn(move || drop(master));
        }
    }
}

fn command_for(harness: Harness) -> CommandBuilder {
    let exe = harness.executable();
    #[cfg(windows)]
    {
        let mut command = CommandBuilder::new("cmd.exe");
        command.args(["/d", "/s", "/c", exe]);
        command
    }
    #[cfg(not(windows))]
    {
        CommandBuilder::new(exe)
    }
}

/// Drain the terminal into the ring.
///
/// A thread rather than a task: `Read` on a pty master blocks, and blocking a
/// tokio worker starves everything sharing it.
fn pump(
    mut reader: Box<dyn Read + Send>,
    output: Arc<Mutex<Ring>>,
    wrote: Arc<tokio::sync::Notify>,
) {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    {
                        let Ok(mut ring) = output.lock() else { break };
                        // Bytes, not text. Decoding here would corrupt any
                        // character unlucky enough to straddle this boundary.
                        ring.push(&buffer[..n]);
                    }
                    // After the lock is released: a waiter woken while this
                    // thread still held it would block on its own read.
                    wrote.notify_waiters();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
}

fn watch(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    status: Arc<Mutex<HarnessStatus>>,
    wrote: Arc<tokio::sync::Notify>,
) {
    std::thread::spawn(move || {
        let ended = match child.wait() {
            Ok(exit) => HarnessStatus::Exited(exit.exit_code() as i32),
            Err(_) => HarnessStatus::Ended,
        };
        if let Ok(mut slot) = status.lock() {
            // Only if nothing already decided. A kill sets `Ended` and then the
            // wait returns; overwriting would report an exit code for a process
            // somebody stopped.
            if *slot == HarnessStatus::Running {
                *slot = ended;
            }
        }
        // An exit is news too. Without this a surface waiting on output sits
        // there after the process is gone, showing it as running.
        wrote.notify_waiters();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo(text: &str) -> Result<HarnessSession, PtyError> {
        if cfg!(windows) {
            HarnessSession::spawn_raw("cmd.exe", &["/d", "/s", "/c", &format!("echo {text}")])
        } else {
            HarnessSession::spawn_raw("sh", &["-c", &format!("echo {text}")])
        }
    }

    /// Wait for a spawned harness to actually produce something, standing in
    /// for the one thing a terminal emulator does that this test otherwise
    /// would not.
    ///
    /// ConPTY opens by asking where the cursor is — `ESC [ 6 n` — and does not
    /// let the child's output through until something answers. A real terminal
    /// answers; xterm.js in the application answers; a test reading bytes into
    /// a buffer does not, and the whole stream stalls behind the question. So
    /// this replies `ESC [ 1 ; 1 R` once, which is exactly what a terminal at
    /// the origin would say.
    async fn settle(session: &HarnessSession, needle: &str) {
        let mut answered = false;
        // Sixty seconds, for something that takes about one. These spawn real
        // processes beside every other test in the workspace, and a budget
        // tuned to an idle machine is a budget that fails on a busy one.
        for _ in 0..2400 {
            let (seen, _) = session.read_from(0);
            if !answered && seen.windows(4).any(|w| w == b"[6n") {
                let _ = session.write(b"[1;1R");
                answered = true;
            }
            // Wait for the thing actually being looked for, not for "some
            // bytes". ConPTY's own opening handshake is about forty of them, so
            // a length threshold is satisfied before the child has written
            // anything and the test then asserts against a buffer holding only
            // terminal setup.
            if String::from_utf8_lossy(&seen).contains(needle) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    /// The whole machine, against a real pseudo-terminal: spawn, pump into the
    /// ring, notice the exit.
    ///
    /// These four hung for a while, and what they were catching was real: a
    /// ConPTY close waits for its output pipe to drain, the pump thread is
    /// blocked reading that pipe, and the child exiting does not break the
    /// cycle because the terminal outlives its process. Dropping a session
    /// would have hung the application on the click that closed a terminal.
    #[tokio::test]
    async fn output_reaches_the_ring_and_the_exit_is_noticed() {
        let session = echo("axio-pty-lives").expect("a terminal");
        settle(&session, "axio-pty-lives").await;

        let (bytes, cursor) = session.read_from(0);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("axio-pty-lives"), "got: {text:?}");
        assert!(cursor > 0);
        assert!(
            matches!(
                session.status(),
                HarnessStatus::Exited(_) | HarnessStatus::Ended
            ),
            "a finished process must stop reporting as running"
        );
    }

    /// The reload case. A reader that already saw everything is told nothing
    /// new, rather than handed the stream a second time.
    #[tokio::test]
    async fn a_cursor_is_not_a_replay() {
        let session = echo("once").expect("a terminal");
        settle(&session, "once").await;
        let (_, cursor) = session.read_from(0);
        assert!(session.read_from(cursor).0.is_empty());
    }

    /// Killing something already gone is the normal end of a session, not an
    /// error somebody has to handle.
    #[tokio::test]
    async fn killing_a_finished_session_is_not_an_error() {
        let session = echo("bye").expect("a terminal");
        settle(&session, "bye").await;
        session.kill().await.expect("kill is idempotent");
        assert_eq!(session.status(), HarnessStatus::Ended);
    }

    /// The signal a surface waits on instead of asking on a timer.
    ///
    /// Registered before the read, which is the discipline `Notify` demands:
    /// created afterwards, output landing in the gap would wake nobody.
    #[tokio::test]
    async fn a_write_wakes_whoever_is_waiting() {
        let session = echo("woke").expect("a terminal");
        let wrote = session.wrote();

        let woken = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                let waiting = wrote.notified();
                // The cursor query has to be answered or nothing ever flows;
                // this stands in for the terminal that would answer it.
                let (seen, _) = session.read_from(0);
                if seen.windows(4).any(|w| w == b"[6n") {
                    let _ = session.write(b"[1;1R");
                }
                if String::from_utf8_lossy(&seen).contains("woke") {
                    return true;
                }
                // A timeout here is a reason to look again, not a verdict.
                // Spawning a real terminal alongside the rest of the workspace
                // can take longer than any interval worth choosing, and treating
                // "nothing yet" as "never" is how a suite acquires a test that
                // fails once a fortnight for no reason anybody can reproduce.
                // Only the outer budget ends this.
                let _ = tokio::time::timeout(std::time::Duration::from_millis(500), waiting).await;
            }
        })
        .await
        .expect("the signal must arrive rather than hang");

        assert!(
            woken,
            "a write has to wake a waiter, or a surface polls forever"
        );
    }

    #[tokio::test]
    async fn a_terminal_can_be_resized_after_it_starts() {
        let session = echo("size").expect("a terminal");
        session.resize(40, 100).expect("resize");
    }
}
