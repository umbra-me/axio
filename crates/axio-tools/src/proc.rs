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
const STRIP: &[&str] = &["ANTHROPIC_AUTH_TOKEN"];

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
    if STRIP.contains(&key) || STRIP_PREFIX.iter().any(|p| key.starts_with(p)) {
        return true;
    }
    // Derived, not listed. A hand-written list held only the Anthropic
    // variables, so `OLLAMA_API_KEY` — the credential for two of the three
    // provider names — was inherited verbatim by every command axio spawned.
    // Asking the auth module means adding a provider cannot leave its key
    // behind.
    axio_core::auth::PROVIDERS
        .iter()
        .filter_map(|p| axio_core::auth::env_var_for(p))
        .any(|var| var == key)
}

/// Configure a command for containment and cancellation.
///
/// On unix the child leads a new process group, so a kill reaches everything it
/// spawned. On Windows it gets a new process group, which keeps a console
/// Ctrl-C from reaching it behind our backs — cancellation is decided here, not
/// by whoever is holding the console. Killing the tree there is a separate
/// problem, and [`kill_tree`] is where it is solved.
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
///
/// Unix kills the process group, which the child leads. Windows has no such
/// thing to signal, so it asks `taskkill` to walk the tree — a job object would
/// be the tidier answer, but a job has to be assigned at spawn time to capture
/// anything, and by the time a kill is wanted the grandchildren already exist.
/// Only the unix half is covered by an automated orphan test; the Windows half
/// is verified by construction, and labelled as such in the gotchas.
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
    #[cfg(windows)]
    {
        if let Some(pid) = child.id() {
            // `/T` is the whole point: it walks the tree from this pid down, so
            // a shell's grandchildren go too. Failure is ignored deliberately —
            // the process may already be gone, and `child.kill()` below is the
            // backstop either way.
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }
    }
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_providers_credential_is_stripped() {
        // A hand-written list is how `OLLAMA_API_KEY` got left in: it protected
        // the provider the author had in mind and no other.
        for provider in axio_core::auth::PROVIDERS {
            if let Some(var) = axio_core::auth::env_var_for(provider) {
                assert!(
                    is_stripped(var),
                    "{var}, the credential for `{provider}`, reaches every child"
                );
            }
        }
    }

    #[test]
    fn the_credential_never_reaches_a_child() {
        assert!(is_stripped("ANTHROPIC_API_KEY"));
        assert!(is_stripped("ANTHROPIC_AUTH_TOKEN"));
        assert!(is_stripped("OLLAMA_API_KEY"));
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
