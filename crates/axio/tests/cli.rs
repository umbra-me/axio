//! Binary-level behaviour: what the process does before it ever needs a network.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn axio() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_axio"));
    // Every test here must be independent of the developer's own shell.
    c.env_remove("ANTHROPIC_API_KEY");
    c.env_remove("NO_COLOR");
    c
}

#[test]
fn version_carries_a_build_sha() {
    let out = axio().arg("--version").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
    assert!(
        text.contains('(') && text.contains(')'),
        "the build sha should be present: {text}"
    );
}

#[test]
fn no_prompt_and_no_input_explains_itself() {
    let out = axio().stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("axio -p"), "usage should name the flag: {err}");
    assert!(out.stdout.is_empty(), "usage never goes to stdout");
}

#[test]
fn a_missing_credential_names_the_exact_next_step() {
    let out = axio()
        .args(["-p", "hi"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("ANTHROPIC_API_KEY"),
        "the error must name the variable: {err}"
    );
    assert!(
        err.contains("export ANTHROPIC_API_KEY="),
        "and give the command to run: {err}"
    );
}

#[test]
fn doctor_reports_what_axio_can_see() {
    let out = axio()
        .arg("--doctor")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "credentials",
        "ANTHROPIC_API_KEY",
        "claude-opus-5",
        "xhigh",
        "assumed prices",
        "paths",
    ] {
        assert!(
            text.contains(expected),
            "doctor omitted {expected}:\n{text}"
        );
    }
}

#[test]
fn doctor_never_prints_the_key_itself() {
    let out = axio()
        .env("ANTHROPIC_API_KEY", "sk-ant-api03-SECRETVALUE123456")
        .arg("--doctor")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("set ("), "it should confirm the key is set");
    assert!(
        !text.contains("SECRETVALUE123456"),
        "doctor leaked the credential:\n{text}"
    );
}

/// Regression. With `-p`, stdin is supplementary — but an inherited pipe that
/// is held open and never written to made the process block on `read_to_string`
/// forever, producing no output at all. A silent infinite hang is the worst
/// failure mode a CLI has, and only a timed test catches it.
#[test]
fn a_prompt_flag_does_not_block_on_an_idle_stdin_pipe() {
    let started = Instant::now();
    let mut child = axio()
        .args(["-p", "hi"])
        .stdin(Stdio::piped()) // opened, never written, never closed
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _held_open = child.stdin.take();

    let deadline = Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            // It fails for want of a credential, which is the point: it got far
            // enough to try.
            assert_eq!(status.code(), Some(1));
            return;
        }
        if started.elapsed() > deadline {
            let _ = child.kill();
            panic!("axio -p hung on an idle stdin pipe for {deadline:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The counterpart: with no `-p`, stdin *is* the prompt, so it is read to EOF.
#[test]
fn piped_stdin_alone_is_read_as_the_prompt() {
    let mut child = axio()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"say hi").unwrap();

    let out = child.wait_with_output().unwrap();
    // Reached the credential check, so the prompt was accepted rather than
    // treated as "nothing to do" (which would be exit 2).
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("ANTHROPIC_API_KEY"));
}

#[test]
fn output_is_free_of_escape_bytes_when_stdout_is_not_a_terminal() {
    // The test harness gives us pipes, so this is the piped case by
    // construction. Nothing on either stream may carry an escape sequence.
    let out = axio()
        .arg("--doctor")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.stdout.contains(&0x1b), "stdout carried an escape byte");
    assert!(!out.stderr.contains(&0x1b), "stderr carried an escape byte");
}

/// Regression, and an M3 gate. With no terminal to ask and no `--yes`, an action
/// needing approval must be refused *promptly*. The failure mode being guarded
/// against is a hang: a process waiting forever for an answer nobody can give.
/// Only a timed assertion catches that, which is why this is a wall-clock test.
#[test]
fn without_a_terminal_and_without_yes_it_refuses_rather_than_waiting() {
    let started = Instant::now();
    let out = axio()
        .args(["-p", "write a file"])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "axio waited {:?} for an approval nobody could give",
        started.elapsed()
    );
    // It gets as far as the credential check, which is the point: it did not
    // block before reaching it.
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn the_yes_flag_announces_itself() {
    // An unattended, unsandboxed mode should never be silent about it.
    let out = axio()
        .args(["--yes", "-p", "hi"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--yes is on"), "{err}");
    assert!(err.contains("no sandbox"), "{err}");
    // Exactly once. Asserting only presence lets a duplicate survive, and a
    // duplicated warning reads as a bug in the thing doing the warning.
    assert_eq!(err.matches("--yes is on").count(), 1, "{err}");
}
