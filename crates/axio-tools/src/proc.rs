//! Spawning, containing and killing child processes.
//!
//! Two things here are not optional. A child gets its own process group, so
//! cancelling a tool kills the whole tree rather than orphaning a shell's
//! grandchildren. And the credential is stripped from the child environment —
//! a speed bump rather than a boundary, but a five-line one.

use std::process::Stdio;

use tokio::process::Command;

/// Variables never passed to a child.
///
/// This is not a security boundary: under `--yes` the agent has the shell, and
/// anything the shell can read it can read. It is a cheap way to stop a
/// model-authored `curl -d "$ANTHROPIC_API_KEY"` from working by accident.
const STRIP: &[&str] = &["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];

/// Prefixes stripped wholesale — axio's own configuration, including anything
/// added later, so a new variable is protected by default rather than by
/// remembering to add it here.
const STRIP_PREFIX: &[&str] = &["AXIO_"];

/// Build the child environment from the parent's, minus the credentials.
pub fn child_env() -> Vec<(String, String)> {
    filter_env(std::env::vars())
}

/// The filtering itself, over any iterator.
///
/// Separated from `child_env` so it can be tested without mutating the
/// process environment — a test that sets a real variable is a test that races
/// every other test in the binary.
pub fn filter_env(vars: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    vars.filter(|(k, _)| !is_stripped(k))
        .chain([
            // Tools that colour their output for a terminal produce escape
            // noise the model has to read past. Ask them not to.
            ("NO_COLOR".to_owned(), "1".to_owned()),
            ("TERM".to_owned(), "dumb".to_owned()),
        ])
        .collect()
}

pub fn is_stripped(key: &str) -> bool {
    STRIP.contains(&key) || STRIP_PREFIX.iter().any(|p| key.starts_with(p))
}

/// Configure a command for containment and cancellation.
///
/// On unix the child leads a new process group, so a kill reaches everything it
/// spawned. On Windows it gets a new process group *and* a job object, because
/// a process group alone does not stop grandchildren surviving.
///
/// Note what is deliberately absent on Windows: `CREATE_NO_WINDOW`. That flag
/// belongs to a GUI application spawning a console child. In a console
/// application it detaches the child from the console, which breaks anything
/// that checks whether it is attached to a terminal.
pub fn configure(cmd: &mut Command, cwd: &std::path::Path, env: &[(String, String)]) {
    cmd.current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    for (k, v) in env {
        cmd.env(k, v);
    }

    #[cfg(unix)]
    {
        // Its own process group, so killing the group reaches the whole tree.
        cmd.process_group(0);
    }

    #[cfg(windows)]
    {
        // `creation_flags` is INHERENT on tokio's Command. Importing
        // `std::os::windows::process::CommandExt` to reach it leaves an unused
        // import, which fails clippy at -D warnings on Windows and nowhere
        // else — so the mistake only ever shows up in CI.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
}

/// Kill a child and everything it spawned.
///
/// `Child::kill` alone signals one process. A shell that started a build leaves
/// the build running, which is exactly the orphan the acceptance test looks for.
pub async fn kill_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // Negative pid addresses the whole group. The child leads its own
            // group (see `configure`), so this cannot reach our own process.
            let pid = pid as i32;
            // SAFETY: `killpg` against a group this process created (see
            // `configure`, which puts the child in its own group). The only
            // effect is signal delivery; an invalid pid is an error return, not
            // undefined behaviour. It cannot reach our own group.
            #[allow(unsafe_code)]
            unsafe {
                libc::killpg(pid, libc::SIGTERM);
            }
            // A moment to shut down cleanly, then insist.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            #[allow(unsafe_code)]
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_never_reaches_a_child() {
        assert!(is_stripped("ANTHROPIC_API_KEY"));
        assert!(is_stripped("ANTHROPIC_AUTH_TOKEN"));
        assert!(!is_stripped("PATH"));
        assert!(!is_stripped("HOME"));
    }

    #[test]
    fn axio_configuration_is_stripped_by_prefix_not_by_list() {
        // A variable added later is protected without anyone remembering to
        // come back here.
        assert!(is_stripped("AXIO_STDIN_WAIT_MS"));
        assert!(is_stripped("AXIO_SOMETHING_INVENTED_LATER"));
    }

    #[test]
    fn the_built_environment_omits_the_credential() {
        let parent = [
            ("PATH", "/usr/bin"),
            ("ANTHROPIC_API_KEY", "sk-ant-should-not-appear"),
            ("AXIO_STDIN_WAIT_MS", "500"),
            ("HOME", "/home/user"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()));

        let env = filter_env(parent);
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

        assert!(
            !keys.contains(&"ANTHROPIC_API_KEY"),
            "the credential was passed to the child"
        );
        assert!(!keys.contains(&"AXIO_STDIN_WAIT_MS"));
        assert!(keys.contains(&"PATH"), "a child still needs a PATH");
        assert!(keys.contains(&"HOME"));
        assert!(
            env.iter().any(|(k, v)| k == "NO_COLOR" && v == "1"),
            "children should be asked not to emit colour"
        );
    }
}
