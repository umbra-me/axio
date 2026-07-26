//! What a whole turn tells the user, end to end against a stub provider.
//!
//! The defect these cover is not in any one component: policy refused the
//! write, the session recorded it, `--json` carried it — and the plain surface,
//! which is the only human surface, printed nothing. The model then said the
//! work was done, and the process exited 0. Every layer behaved and the user was
//! still misled, so the test has to be at the level of the process.

mod support;

use std::time::Duration;

use support::{spawn_axio, stub_provider, wait_for};

struct Run {
    code: Option<i32>,
    #[allow(dead_code)]
    stdout: String,
    stderr: String,
    home: tempfile::TempDir,
}

fn run(tool: &str, input: serde_json::Value, answer: &str, args: &[&str]) -> Run {
    let home = tempfile::tempdir().expect("a temp dir");
    let (base, _server) = stub_provider(tool, input, answer);
    let mut child = spawn_axio(&base, home.path(), args);

    let code = wait_for(Duration::from_secs(60), || child.try_wait().ok().flatten())
        .unwrap_or_else(|| {
            let _ = child.kill();
            panic!("the turn never finished")
        })
        .code();

    let mut out = Vec::new();
    let mut err = Vec::new();
    use std::io::Read;
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_end(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_end(&mut err);
    }
    Run {
        code,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        home,
    }
}

/// Regression, and the worst of the batch. Without `--yes` the write is
/// refused, the model reports success anyway, and the user sees confident prose,
/// an empty stderr and exit 0. Nothing on any stream distinguishes "done" from
/// "refused everything and made it up".
#[test]
fn a_refused_write_is_visible_and_changes_the_exit_code() {
    let r = run(
        "write",
        serde_json::json!({"path": "notes.md", "content": "- a bullet\n"}),
        "The bullet has been added to notes.md.",
        &[],
    );

    assert!(
        !r.home.path().join("notes.md").exists(),
        "a refused write must not reach the disk"
    );
    assert!(
        r.stderr.contains("[denied] write:notes.md"),
        "the refusal must be on stderr:\n{}",
        r.stderr
    );
    assert!(
        r.stderr.contains("refused"),
        "the turn must summarise that work did not happen:\n{}",
        r.stderr
    );
    assert_eq!(
        r.code,
        Some(5),
        "a script gating on the exit code must be able to tell"
    );
}

/// The counterpart: with `--yes` the write lands, and a turn that changes a file
/// without saying so is indistinguishable from a no-op.
#[test]
fn an_applied_write_is_reported_even_when_the_answer_is_empty() {
    let r = run(
        "write",
        serde_json::json!({"path": "notes.md", "content": "- a bullet\n"}),
        "",
        &["--yes"],
    );

    assert_eq!(
        std::fs::read_to_string(r.home.path().join("notes.md")).unwrap(),
        "- a bullet\n"
    );
    assert!(
        r.stderr.contains("[changed: notes.md]"),
        "the user must be told the file changed:\n{}",
        r.stderr
    );
    assert_eq!(r.code, Some(0));
}

/// Regression. `read .env` was refused by the built-in list and `bash cat .env`
/// printed the key — the list matches paths, and a shell command's subject is
/// only its program name. `--yes` is where nobody is watching.
#[test]
fn the_built_in_deny_list_survives_a_shell_command() {
    let home = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        home.path().join(".env"),
        "API_KEY=sk-live-CANARY-not-a-real-key\n",
    )
    .unwrap();

    let (base, _server) = stub_provider(
        "bash",
        serde_json::json!({"command": "cat .env", "timeout_secs": 30}),
        "reported",
    );
    let mut child = spawn_axio(&base, home.path(), &["--yes"]);
    let code = wait_for(Duration::from_secs(60), || child.try_wait().ok().flatten())
        .unwrap_or_else(|| {
            let _ = child.kill();
            panic!("the turn never finished")
        })
        .code();

    let mut err = Vec::new();
    use std::io::Read;
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_end(&mut err);
    }
    let stderr = String::from_utf8_lossy(&err);

    assert!(
        stderr.contains("[denied] bash:cat"),
        "`cat .env` must be refused even under --yes:\n{stderr}"
    );
    assert!(
        !stderr.contains("sk-live-CANARY"),
        "the credential reached the output:\n{stderr}"
    );
    assert_eq!(code, Some(5));

    // And it never reached the transcript either, which is what the model would
    // have read back on a resume.
    let sessions = home.path().join("state").join("sessions");
    let mut found = String::new();
    for entry in walk(&sessions) {
        found.push_str(&std::fs::read_to_string(entry).unwrap_or_default());
    }
    assert!(
        !found.contains("sk-live-CANARY"),
        "the credential was persisted to the session record"
    );
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
