//! What the process leaves behind when it is told to stop.
//!
//! These tests deliver real signals to a real `axio` process that is really
//! running a child, because that is the only way the defect they cover is
//! visible. The in-process test in `axio-tools` cancels a token and checks for
//! orphans; it passes whether or not the signal path ever reaches the code that
//! cancels, which is exactly how `SIGTERM` came to orphan the whole tree while
//! the suite stayed green.

#![cfg(unix)]

mod support;

use std::process::Command;
use std::time::Duration;

use support::{spawn_axio, stub_provider, wait_for};

/// The pid of `parent`'s child process, if it has one.
///
/// By parentage rather than by a marker in the command: `/bin/sh -c "sleep 90"`
/// is a single simple command, so the shell `exec`s it and the process that
/// survives carries `sleep`'s argv, not the shell's. A marker written into the
/// command would be gone by the time `ps` could see it, and the test would look
/// like it was watching something when it was watching nothing.
fn child_of(parent: u32) -> Option<u32> {
    let out = Command::new("ps")
        .args(["-eo", "pid,ppid,args"])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid: u32 = fields.next()?.parse().ok()?;
            let ppid: u32 = fields.next()?.parse().ok()?;
            (ppid == parent).then_some(pid)
        })
        .next()
}

fn alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Deliver `signal` mid-command and report (exit code, whether the child
/// survived).
fn interrupt_with(signal: &str) -> (Option<i32>, bool) {
    let home = tempfile::tempdir().expect("a temp dir");
    let (base, _server) = stub_provider(
        "bash",
        serde_json::json!({"command": "sleep 90", "timeout_secs": 120}),
        "done",
    );
    let mut child = spawn_axio(&base, home.path(), &["--yes"]);
    let axio_pid = child.id();

    let grandchild = match wait_for(Duration::from_secs(30), || child_of(axio_pid)) {
        Some(pid) => pid,
        None => {
            let _ = child.kill();
            let out = child.wait_with_output().expect("output");
            panic!(
                "the {signal} run never started its command\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        }
    };

    let _ = Command::new("kill")
        .args([&format!("-{signal}"), &child.id().to_string()])
        .status();

    let status = wait_for(Duration::from_secs(20), || child.try_wait().ok().flatten())
        .unwrap_or_else(|| panic!("axio did not exit after {signal}"));

    // Give a would-be orphan every chance to be seen before deciding.
    std::thread::sleep(Duration::from_millis(500));
    let survived = alive(grandchild);
    if survived {
        let _ = Command::new("kill")
            .args(["-KILL", &grandchild.to_string()])
            .status();
    }
    (status.code(), survived)
}

#[test]
fn sigterm_takes_the_command_down_with_it() {
    // `process::exit` in the signal handler runs no destructor and polls no
    // task, so the tree kill never happens and a build keeps running with no
    // parent to reap it. SIGTERM is what every supervisor, container stop and
    // `timeout` sends.
    let (code, survived) = interrupt_with("TERM");
    assert!(!survived, "SIGTERM left the command running");
    assert_eq!(code, Some(143));
}

#[test]
fn sighup_takes_the_command_down_with_it() {
    // The same path, reached by closing a terminal or dropping an SSH session.
    let (code, survived) = interrupt_with("HUP");
    assert!(!survived, "SIGHUP left the command running");
    assert_eq!(code, Some(129));
}

#[test]
fn sigint_takes_the_command_down_with_it() {
    // The path that already worked; here so a change that fixes one and breaks
    // the other cannot pass.
    let (code, survived) = interrupt_with("INT");
    assert!(!survived, "SIGINT left the command running");
    assert_eq!(code, Some(130));
}
